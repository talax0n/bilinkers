const fs = require('node:fs');
const { config } = require('../config');
const { attachToLms, getCurrentQuestionDom } = require('../browser');
const { createRegistry } = require('../questionTypes/registry');
const audioMultipleChoice = require('../questionTypes/audioMultipleChoice');
const readingComprehension = require('../questionTypes/readingComprehension');
const { createClient, answerQuestion } = require('../llm');
const { readPills, clickPill, ensurePillsExpanded, clickSaveButton, waitForPillSaved } = require('./checkpoint');
const logger = require('../logger');

// Site behavior this relies on: docs/platform-notes.md#final-test

// Read-only by default. A Final Test has one attempt.
const OUTPUT_PATH = './final-test-answers.md';
const ANSWERS_PATH = './final-test-answers.json';

// The path without the #/question-N hash identifies the test.
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

// Agreeing passes raise confidence and count agreements. A disagreement halves the lower confidence.
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

const ANSWER_INSTRUCTION =
  'Determine the single correct option letter for this question, using the passage/question text and options provided. ' +
  'Respond as JSON: { "answer": "<letter>", "confidence": <0.0 to 1.0> }.';

// The review prompt stays neutral. combineReview does the weighting.
function reviewInstruction(previous) {
  return (
    'A previous review pass suggested answer ' +
    `"${previous.answer}". Re-derive the answer yourself from the question ` +
    'and options; do not assume the previous answer is correct. If your own ' +
    'reasoning reaches the same letter, keep it; otherwise give the letter you ' +
    'now believe is correct. Report your confidence in your OWN judgment alone.'
  );
}

// One answer per question. Retries cover transient call failures only.
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

// Read-only unless autoAnswer is on. pillDomLookupFn lets tests fake per-pill DOM.
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
    // A dropped pill click would re-read the same question, and the pill nav re-collapses,
    // so re-expand and retry.
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

    // Trust the DOM once two reads changePollMs apart match. outerHTML noise makes
    // comparing to the previous question useless.
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
  const driver = existingDriver || (await attachToLms());
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
