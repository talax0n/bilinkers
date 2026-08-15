const { Builder } = require('selenium-webdriver');
const chrome = require('selenium-webdriver/chrome');
const fs = require('node:fs');
const { resolveChromedriverPath } = require('../src/chromedriver');
const { config } = require('../src/config');
const { getCurrentQuestionDom } = require('../src/browser');
const { createRegistry } = require('../src/questionTypes/registry');
const audioMultipleChoice = require('../src/questionTypes/audioMultipleChoice');
const readingComprehension = require('../src/questionTypes/readingComprehension');
const { createClient, answerQuestion } = require('../src/llm');
const { readPills, clickPill, ensurePillsExpanded, clickSaveButton, waitForPillSaved } = require('./run-checkpoint');
const logger = require('../src/logger');

// This script attaches to an already-running Chromium-based browser sitting
// on an OPEN Final Test attempt (50 questions, 1 attempt, same pill-nav
// native quiz DOM as a checkpoint) and reads every question into a Markdown
// answer key. By default it never clicks an option, Save, or Submit — a Final
// Test has exactly one attempt, so there is no room to brute-force like
// run-checkpoint.js does. Listening questions (audioMultipleChoice) are
// skipped since the bot can't hear the audio. Run via
// `node scripts/cli.js final-test`, or invoke this file directly if the
// browser is already up and sitting on an open Final Test attempt.
//
// OPT-IN LISTENING GUESSES: set FINAL_TEST_GUESS_LISTENING=1 — each listening
// question gets a random option letter clicked and Saved instead of skipped.
// A blank is worth 0 points; a random guess is ~25% per question, which is
// strictly better when you only need to clear the passing score (66). The
// guess is still written to the answer key so you can review it before
// submitting.
//
// OPT-IN auto-answering: set FINAL_TEST_AUTO_ANSWER=1 in .env (and optionally
// FINAL_TEST_CONFIDENCE_THRESHOLD, default 0.9). The LLM is asked for a
// self-assessed confidence alongside each answer, and only answers scoring
// at/above the threshold are auto-clicked and Saved on the spot; the rest
// still land in the answer key for manual review. Still one attempt — an
// overconfident guess cannot be undone.
//
// MULTI-PASS REVIEW: every run writes its answers (answer, confidence,
// question text, options) to final-test-answers.json in addition to the
// markdown key. On a later run over the SAME final test, those previous
// answers are loaded and each question is re-derived INDEPENDENTLY — the LLM
// is told what the previous pass suggested but explicitly told not to trust
// it. Agreement between passes boosts confidence (auto-clickable); a
// disagreement caps it low so that question stays manual. A third+ pass
// compounds on the second, so confidence only climbs for answers that keep
// agreeing pass after pass, and an overconfident wrong guess from a single
// pass never clears the threshold on its own. FINAL_TEST_REVIEWS_REQUIRED
// (default 1) adds a hard gate: an answer needs that many consecutive
// agreeing passes before it can auto-click at all.
//
// STALE-GUARD: the json also stores the attempt URL's path (testId) it was
// scraped from. If you run the script while a leftover json from a DIFFERENT
// final test exists, that old data is detected as stale and IGNORED (fresh
// pass, no review hints, no bogus confidence boost) — the file is then
// overwritten with the new test's own data. You don't need to delete it
// yourself.
const CHROMEDRIVER_PATH = process.env.CHROMEDRIVER_PATH;
const OUTPUT_PATH = './final-test-answers.md';
const ANSWERS_PATH = './final-test-answers.json';

// The attempt URL's path (sans the #/question-N hash, which changes every
// pill) identifies which final test the answers were scraped from. It's
// written alongside the answers so a later run can tell "same test, review
// it" from "different test — the old json is stale, ignore it".
function testIdFromUrl(url) {
  return (url || '').split('#')[0];
}

function loadPreviousAnswers(testId, filePath = ANSWERS_PATH) {
  try {
    const parsed = JSON.parse(fs.readFileSync(filePath, 'utf8'));
    const answers = Array.isArray(parsed.answers) ? parsed.answers : [];
    if (testId && parsed.testId && parsed.testId !== testId) {
      return { answers: [], stale: true, storedTestId: parsed.testId };
    }
    return { answers, stale: false, storedTestId: parsed.testId || null };
  } catch {
    return { answers: [], stale: false, storedTestId: null };
  }
}

// Merges a fresh, independent pass with the previous pass's record. The whole
// point of the review loop is that a SINGLE self-assessed confidence is
// unreliable — an overconfident first guess must not commit the one attempt.
// Two passes agreeing is the verification signal: confidence is lifted well
// above what either pass claimed alone. Two passes disagreeing is the
// opposite signal: the new answer is recorded (the reviewer may have caught a
// real mistake) but its confidence is crushed, so it can never auto-click.
//
// Also tracks `agreements` — how many consecutive passes have agreed on the
// current answer (1 for a brand-new answer, +1 for each later pass that
// confirms it, reset to 1 when the answer changes). FINAL_TEST_REVIEWS_REQUIRED
// turns that into a hard gate: auto-click needs BOTH confidence >= threshold
// AND agreements >= reviewsRequired, so a high-confidence-but-only-once answer
// still never clicks until enough independent passes have ratified it.
function combineReview(previous, current) {
  if (!previous || previous.answer === 'NEEDS REVIEW') return { ...current, agreements: 1 };
  if (current.answer === 'NEEDS REVIEW') return previous;
  if (previous.answer === current.answer) {
    return {
      answer: current.answer,
      confidence: Math.min(0.99, Math.max(previous.confidence || 0, current.confidence || 0) + 0.2),
      agreements: (previous.agreements || 1) + 1,
    };
  }
  return { answer: current.answer, confidence: Math.min(current.confidence || 0, previous.confidence || 0) * 0.5, agreements: 1 };
}

function formatQuestionRecord({ number, handlerName, questionData, answer, confidence, agreements, autoAnswered = false }) {
  if (handlerName === 'audioMultipleChoice') {
    const record = { number, type: 'listening' };
    if (answer) record.answer = answer;
    if (autoAnswered) record.autoAnswered = true;
    return record;
  }
  if (!handlerName) return { number, type: 'unhandled' };
  const record = { number, type: 'mcq', text: questionData.text, options: questionData.options, answer, confidence, agreements };
  if (autoAnswered) record.autoAnswered = true;
  return record;
}

function buildMarkdown(records) {
  return records
    .map((record) => {
      if (record.type === 'listening') {
        if (record.answer) return `## Q${record.number} (listening — guessed: ${record.answer})\n`;
        return `## Q${record.number} (listening — skipped, jawab manual)\n`;
      }
      if (record.type === 'unhandled') return `## Q${record.number} (unhandled question type — jawab manual)\n`;
      const optionLines = record.options.map((opt) => `${opt.letter}. ${opt.text}`).join('\n');
      const conf = record.confidence !== undefined ? ` (confidence ${record.confidence.toFixed(2)})` : '';
      const answered = record.autoAnswered ? '**Jawaban: {AUTO-CLICKED}**' : `**Jawaban: ${record.answer}**`;
      return `## Q${record.number}\n${record.text}\n${optionLines}\n${answered}${conf}\n`;
    })
    .join('\n');
}

async function attachToBrave() {
  const options = new chrome.Options();
  options.debuggerAddress('localhost:9222');
  const chromedriverPath = CHROMEDRIVER_PATH || (await resolveChromedriverPath());
  const builder = new Builder().forBrowser('chrome').setChromeOptions(options).setChromeService(new chrome.ServiceBuilder(chromedriverPath));
  const driver = await builder.build();

  const handles = await driver.getAllWindowHandles();
  for (const handle of handles) {
    await driver.switchTo().window(handle);
    const url = await driver.getCurrentUrl();
    if (url.includes('lms.binus.ac.id')) return driver;
  }
  throw new Error('No lms.binus.ac.id tab found among open Brave windows.');
}

const ANSWER_INSTRUCTION =
  'Determine the single correct option letter for this question, using the passage/question text and options provided. ' +
  'Respond as JSON: { "answer": "<letter>", "confidence": <0.0 to 1.0> }.';

// When a previous pass's answer exists, the model re-derives the question from
// scratch rather than just ratifying what it saw — it's told the earlier
// suggestion only so it can actively disagree with it, and told its confidence
// must reflect its own fresh judgment, not agreement with that earlier answer.
// combineReview() then applies the real weighting (two agreeing passes lift
// confidence, a disagreement caps it), so the prompt staying neutral on the
// previous answer is what keeps the review genuinely independent.
function reviewInstruction(previous) {
  return (
    'A previous review pass suggested answer ' +
    `"${previous.answer}". Re-derive the answer yourself from the question ` +
    'and options; do not assume the previous answer is correct. If your own ' +
    'reasoning reaches the same letter, keep it; otherwise give the letter you ' +
    'now believe is correct. Report your confidence in your OWN judgment alone.'
  );
}

// A single LLM guess per question — there's no per-question feedback to loop
// on (unlike run-exercise.js's retry-with-feedback) and only one real
// attempt exists, so retries here are purely for transient call failures
// (matches src/runner.js's callWithRetry), not for trying a different
// answer. Exhausting retries marks the question for manual review instead of
// throwing, so one bad question can't abort the other 49. Also asks for a
// self-assessed confidence (0..1) — an answer at/above confidenceThreshold is
// safe to auto-click, anything below goes to the manual answer key.
// When previousAnswer is supplied (a prior run's record for this question),
// the model is asked to review rather than answer fresh.
async function answerWithRetry(llmClient, model, questionData, { answerQuestionFn = answerQuestion, maxAttempts = 3, sleepFn = (ms) => new Promise((r) => setTimeout(r, ms)), previousAnswer = null } = {}) {
  const instruction = previousAnswer
    ? `${ANSWER_INSTRUCTION} ${reviewInstruction(previousAnswer)}`
    : ANSWER_INSTRUCTION;
  let lastErr;
  for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
    try {
      const result = await answerQuestionFn(llmClient, model, instruction, questionData, null);
      const confidence = Number(result.confidence);
      return { answer: result.answer, confidence: Number.isFinite(confidence) ? confidence : 0 };
    } catch (err) {
      lastErr = err;
      if (attempt < maxAttempts) await sleepFn(1000);
    }
  }
  logger.warn('Final test: LLM failed for a question after retries — marking for manual review', { error: lastErr.message });
  return { answer: 'NEEDS REVIEW', confidence: 0 };
}

// Clicks the option button whose text starts with the given letter (the
// final test's MCQ rows are `button.bl-w-full.justify-content-start`, same
// shape as a checkpoint's), without any Check/Save — the caller decides.
async function clickOptionByLetter(driver, letter) {
  await driver.switchTo().defaultContent();
  return driver.executeScript(
    `
    const letter = arguments[0];
    const buttons = [...document.querySelectorAll('button.bl-w-full.justify-content-start')]
      .filter((b) => b.offsetParent !== null);
    const target = buttons.find((b) => b.textContent.trim().startsWith(letter));
    if (target && !target.disabled) { target.click(); return true; }
    return false;
  `,
    letter
  );
}

async function saveAndWait(driver, n) {
  const saved = await clickSaveButton(driver);
  if (!saved) return false;
  return waitForPillSaved(driver, n);
}

// Walks every pill 1..N (N read live, not hardcoded), parsing and guessing
// each question. Read-only unless autoAnswer is enabled: only pill navigation
// touches the DOM by default, no option/Save/Submit click ever happens.
// With autoAnswer=true and an answer whose confidence clears
// confidenceThreshold, the option is clicked and the answer saved on the spot
// (still one attempt — an overconfident guess can't be undone). Everything
// below threshold stays in the markdown key for manual answering.
// When previousAnswers (a prior run's records) is supplied, each question is
// re-derived independently and combined via combineReview — see the
// MULTI-PASS REVIEW note at the top of this file.
// pillDomLookupFn/getCurrentQuestionDomFn
// are separated (rather than one combined call) so tests can fake per-pill
// DOM content without a real driver.
async function scrapeFinalTest(
  driver,
  {
    llmClient,
    model,
    readPillsFn = readPills,
    clickPillFn = clickPill,
    ensurePillsExpandedFn = ensurePillsExpanded,
    getCurrentQuestionDomFn = getCurrentQuestionDom,
    createRegistryFn = createRegistry,
    answerQuestionFn = answerQuestion,
    pillDomLookupFn = null,
    logger: injectedLogger = logger,
    changeWaitMs = 5000,
    changePollMs = 300,
    sleepFn = (ms) => new Promise((r) => setTimeout(r, ms)),
    autoAnswer = false,
    confidenceThreshold = 0.9,
    reviewsRequired = 1,
    clickOptionFn = clickOptionByLetter,
    saveAndWaitFn = saveAndWait,
    previousAnswers = [],
    guessListening = false,
    randomLetterFn = (letters) => letters[Math.floor(Math.random() * letters.length)],
  } = {}
) {
  const registry = createRegistryFn();
  registry.register(audioMultipleChoice);
  registry.register(readingComprehension);

  const previousByNumber = new Map(previousAnswers.filter((r) => r.type === 'mcq').map((r) => [r.number, r]));

  await ensurePillsExpandedFn(driver);
  const pills = await readPillsFn(driver);
  if (pills.length === 0) {
    injectedLogger.warn('Final test: no nav pills found — stopping', {});
    return { status: 'no-pills' };
  }

  const records = [];
  for (const { n } of pills) {
    // clickPillFn's return value used to be discarded — a failed click
    // (verified live: the whole run silently stalled on one pill's content
    // for every subsequent question, with no error, because a later click
    // never registered) left every following iteration re-reading that same
    // stale page instead of the actual next question. Retry a few times and
    // bail loudly instead of scraping the wrong question under a new number.
    //
    // The pill nav also re-collapses to its default ~12-pill view after
    // landing on some questions (live-verified after Q11/Q12) — expanding
    // once before the loop starts isn't enough, so re-expand on every
    // retry before clicking again.
    let clicked = false;
    for (let attempt = 1; attempt <= 3 && !clicked; attempt += 1) {
      if (!pillDomLookupFn) await ensurePillsExpandedFn(driver);
      clicked = pillDomLookupFn ? true : await clickPillFn(driver, n);
      if (!clicked) await sleepFn(changePollMs);
    }
    if (!clicked) {
      injectedLogger.warn('Final test: could not click pill — stopping', { pill: n });
      return { status: 'stuck', pill: n, records };
    }

    let dom = pillDomLookupFn ? pillDomLookupFn(n) : await getCurrentQuestionDomFn(driver, { iframeWaitMs: 600 });

    // Comparing each read against the *previous question's* DOM (an earlier
    // version of this fix) doesn't work — outerHTML carries enough
    // incidental noise (ids, timestamps) that back-to-back reads of the
    // very same still-loading question already differ from the prior
    // question's snapshot, so that comparison was satisfied immediately
    // without ever actually waiting. What's live-verified instead: a
    // question that's still mid-render answers two reads spaced apart with
    // different content each time (it's actively changing), while a
    // settled one answers identically — so require two consecutive reads,
    // changePollMs apart, to agree before trusting the DOM.
    if (!pillDomLookupFn) {
      const deadline = Date.now() + changeWaitMs;
      let previous = dom.outerHTML;
      await sleepFn(changePollMs);
      dom = await getCurrentQuestionDomFn(driver, { iframeWaitMs: 600 });
      while (dom.outerHTML !== previous && Date.now() < deadline) {
        previous = dom.outerHTML;
        await sleepFn(changePollMs);
        dom = await getCurrentQuestionDomFn(driver, { iframeWaitMs: 600 });
      }
    }

    const handler = registry.findHandler(dom);

    if (!handler) {
      injectedLogger.warn('Final test: unhandled question type', { pill: n });
      records.push(formatQuestionRecord({ number: n, handlerName: null, questionData: null, answer: null }));
      continue;
    }

    if (handler.name === 'audioMultipleChoice') {
      if (guessListening) {
        const questionData = handler.parse(dom);
        const letters = questionData.options.map((o) => o.letter);
        const guess = randomLetterFn(letters);
        const clicked = await clickOptionFn(driver, guess);
        if (clicked) await saveAndWaitFn(driver, n);
        injectedLogger.info('Final test: listening guessed', { pill: n, guess, clicked });
        records.push(formatQuestionRecord({ number: n, handlerName: handler.name, questionData, answer: clicked ? guess : null, autoAnswered: clicked }));
        continue;
      }
      injectedLogger.info('Final test: skipping listening question', { pill: n });
      records.push(formatQuestionRecord({ number: n, handlerName: handler.name, questionData: null, answer: null }));
      continue;
    }

    const questionData = handler.parse(dom);
    const previous = previousByNumber.get(n) || null;
    const fresh = await answerWithRetry(llmClient, model, questionData, { answerQuestionFn, previousAnswer: previous });
    const { answer, confidence, agreements } = combineReview(previous, fresh);
    const reviewed = Boolean(previous);

    let autoAnswered = false;
    if (autoAnswer && answer !== 'NEEDS REVIEW' && confidence >= confidenceThreshold && agreements >= reviewsRequired) {
      const clicked = await clickOptionFn(driver, answer);
      if (clicked) {
        await saveAndWaitFn(driver, n);
        autoAnswered = true;
        injectedLogger.info('Final test: auto-answered', { pill: n, answer, confidence, agreements, reviewed });
      }
    }
    injectedLogger.info('Final test: question answered', { pill: n, answer, confidence, agreements, reviewed, autoAnswered });
    records.push(formatQuestionRecord({ number: n, handlerName: handler.name, questionData, answer, confidence, agreements, autoAnswered }));
  }

  return { status: 'complete', records };
}

async function main(existingDriver) {
  const driver = existingDriver || (await attachToBrave());
  const llmClient = createClient(config);

  const autoAnswer = process.env.FINAL_TEST_AUTO_ANSWER === '1';
  const confidenceThreshold = Number(process.env.FINAL_TEST_CONFIDENCE_THRESHOLD || 0.9);
  const reviewsRequired = Number(process.env.FINAL_TEST_REVIEWS_REQUIRED || 1);
  const guessListening = process.env.FINAL_TEST_GUESS_LISTENING === '1';

  const testId = testIdFromUrl(await driver.getCurrentUrl());
  const previous = loadPreviousAnswers(testId);
  if (previous.stale) {
    logger.warn('Final test: previous answer key is for a DIFFERENT test — ignoring it and starting fresh', { storedTestId: previous.storedTestId, currentTestId: testId });
  } else if (previous.answers.length > 0) {
    logger.info('Final test: review pass — re-deriving previous answers', { previous: previous.answers.length, testId });
  }

  const result = await scrapeFinalTest(driver, {
    llmClient,
    model: config.openai.model,
    autoAnswer,
    confidenceThreshold,
    reviewsRequired,
    previousAnswers: previous.stale ? [] : previous.answers,
    guessListening,
  });
  if (result.status === 'no-pills') {
    logger.warn('Final test: nothing to scrape — is the browser on an open Final Test attempt?', {});
    return result;
  }

  const markdown = buildMarkdown(result.records);
  fs.writeFileSync(OUTPUT_PATH, markdown);
  fs.writeFileSync(ANSWERS_PATH, JSON.stringify({ testId, answers: result.records }, null, 2));
  logger.info('Final test: answer key written', { path: OUTPUT_PATH, questions: result.records.length });
  return result;
}

if (require.main === module) {
  main().catch((err) => {
    logger.error('Final test scraper failed', { error: err.message });
    process.exitCode = 1;
  });
}

module.exports = { run: main, formatQuestionRecord, buildMarkdown, answerWithRetry, scrapeFinalTest, clickOptionByLetter, saveAndWait, combineReview, loadPreviousAnswers, testIdFromUrl };
