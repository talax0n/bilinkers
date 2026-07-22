const { Builder } = require('selenium-webdriver');
const chrome = require('selenium-webdriver/chrome');
const { resolveChromedriverPath } = require('../src/chromedriver');
const { config } = require('../src/config');
const { getCurrentQuestionDom } = require('../src/browser');
const { createRegistry } = require('../src/questionTypes/registry');
const audioMultipleChoice = require('../src/questionTypes/audioMultipleChoice');
const readingComprehension = require('../src/questionTypes/readingComprehension');
const { callWithRetry } = require('../src/runner');
const { createClient, answerQuestion } = require('../src/llm');
const logger = require('../src/logger');

// This script attaches to an already-running Chromium-based browser (Chrome,
// Brave, Edge) launched with --remote-debugging-port=9222, and drives a
// checkpoint (see the "ENG-B1.2 ... Checkpoint 1" gate screenshot: Total
// Question 30, Passing Score 100, Maximum Attempt Unlimited) end to end:
// click the gate's Start/Continue button, answer every question, submit,
// check the resulting score, and — since the platform allows unlimited
// attempts — retry the whole checkpoint from the gate if the score falls
// short of 100, up to a safety cap. Run via `node scripts/cli.js
// checkpoint`, or invoke this file directly if the browser is already up
// and sitting on a checkpoint's gate page.
// CHROMEDRIVER_PATH overrides auto-detection below (matches chromedriver to
// whatever's actually listening on the debug port) — only needed if that
// fails for your setup.
const CHROMEDRIVER_PATH = process.env.CHROMEDRIVER_PATH;

// A safety cap, not the platform's actual "Unlimited" policy — without one,
// a structural problem (not just an unlucky guess) would retry forever.
const MAX_ATTEMPTS = 5;

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

// The gate button's text isn't fixed: "Start Attempt 1" on a fresh
// checkpoint, "Start Attempt N" on a later one, or "Continue" if an attempt
// is already in progress (verified live — screenshot showed a Continue
// button distinct from Start Attempt N) — matched by prefix/exact text
// rather than one literal string.
async function clickGateButton(driver, timeoutMs = 8000) {
  await driver.switchTo().defaultContent();
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const clicked = await driver.executeScript(`
      const buttons = Array.from(document.querySelectorAll('button'));
      const btn = buttons.find((b) => {
        const text = b.textContent.trim();
        return text.startsWith('Start Attempt') || text === 'Continue';
      });
      if (btn && !btn.disabled) { btn.click(); return true; }
      return false;
    `);
    if (clicked) return true;
    await driver.sleep(300);
  }
  return false;
}

// A checkpoint's per-question nav is "Save & Next" (verified live — not
// "Next", which goToNextQuestion() looks for, and not a bare pill click,
// which navigates without persisting the just-picked option since pills are
// for jumping/review, not saving) — except the very last question, whose
// button reads bare "Save" (there's nothing left to advance to). Clicking
// that final "Save" is also what triggers grading — checkpoints have no
// separate Submit/Yes/Next confirm step the way run-exercise.js's native
// exercises do (verified live: no "Submit" button ever appears here at all).
async function clickSaveAndNext(driver) {
  await driver.switchTo().defaultContent();
  return driver.executeScript(`
    const buttons = Array.from(document.querySelectorAll('button'));
    const btn = buttons.find((b) => {
      const text = b.textContent.trim();
      return text === 'Save & Next' || text === 'Save';
    });
    if (btn && !btn.disabled) { btn.click(); return true; }
    return false;
  `);
}

// After the last question's bare "Save" click, the app auto-navigates to
// .../result/version/{id}/review, e.g.
// ".../BlCheckpoint/{contentId}/result/version/{versionId}/review" — the
// review page's own text reads "...Attempted1Score77Completion Date...",
// so the score sits between the literal words "Score" and "Completion
// Date" with no separator (verified live: a checkpoint that missed 7 of 30
// questions read "Score77").
async function waitForScore(driver, timeoutMs = 10000) {
  await driver.switchTo().defaultContent();
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const score = await driver.executeScript(`
      const match = document.body.textContent.match(/Score(\\d+)Completion Date/);
      return match ? Number(match[1]) : null;
    `);
    if (score !== null) return score;
    await driver.sleep(300);
  }
  return undefined;
}

// Checkpoints never show a per-question "Correct!"/"Incorrect!" heading —
// verified live: run-exercise.js's checkResult path (waitForCheckFeedback)
// timed out on every single attempt here, since there's no Check
// button/feedback in a checkpoint at all, only a final score after
// submitting everything. So there's no live signal to retry a question
// against mid-checkpoint — each question gets exactly one LLM guess,
// selected via the same handler.answer() every other flow uses (its "click
// Check if present" step simply no-ops here), then advanced with "Save &
// Next" instead of run-exercise.js's Next/pill-number fallback.
async function answerAllQuestions(driver) {
  const registry = createRegistry();
  registry.register(audioMultipleChoice);
  registry.register(readingComprehension);
  const llmClient = createClient(config);

  let questionNum = 0;
  for (;;) {
    questionNum += 1;
    const dom = await getCurrentQuestionDom(driver);
    const handler = registry.findHandler(dom);

    if (!handler) {
      logger.saveUnhandled(config.paths.unhandledLogDir, `run-checkpoint-q${questionNum}`, { html: dom.outerHTML });
      logger.warn('Unhandled question type in checkpoint — stopping', { questionNum });
      return { status: 'unhandled', questionNum };
    }

    const questionData = handler.parse(dom);
    const llmResult = await callWithRetry(
      answerQuestion,
      [
        llmClient,
        config.openai.model,
        'Determine the single correct option letter for this question, using the passage/question text and options provided. Respond as JSON: { "answer": "<letter>" }.',
        questionData,
        null,
      ],
      driver
    );
    await handler.answer(driver, llmResult);
    logger.info('Checkpoint question answered', { questionNum, type: handler.name, answer: llmResult.answer });

    const advanced = await clickSaveAndNext(driver);
    if (!advanced) {
      // Genuinely nothing left to click at all — treat as done rather than
      // loop forever.
      logger.info('No further Save/Save & Next button — stopping', { questionNum });
      return { status: 'answered', questionNum };
    }

    // "Save & Next" (a normal question) stays on the same quiz URL and has
    // a next question to parse; the last question's bare "Save" redirects
    // straight to the result/review URL instead — that's the real signal
    // this was the final question, not just another absent-button case.
    await driver.sleep(500);
    const url = await driver.getCurrentUrl();
    if (/\/result\//.test(url) || /\/review/.test(url)) {
      logger.info('Reached the result/review page — all questions answered', { questionNum });
      return { status: 'answered', questionNum };
    }
  }
}

async function runCheckpointAttempt(driver) {
  const answerResult = await answerAllQuestions(driver);
  if (answerResult.status === 'unhandled') {
    return { status: 'unhandled', questionNum: answerResult.questionNum };
  }

  const score = await waitForScore(driver);
  logger.info('Checkpoint attempt graded', { questionNum: answerResult.questionNum, score });
  return { status: 'complete', questionNum: answerResult.questionNum, score };
}

async function main(existingDriver, { runExerciseFn = runCheckpointAttempt } = {}) {
  const driver = existingDriver || (await attachToBrave());
  const gateUrl = await driver.getCurrentUrl();

  for (let attempt = 1; attempt <= MAX_ATTEMPTS; attempt += 1) {
    logger.info('Starting checkpoint attempt', { attempt });

    const clicked = await clickGateButton(driver);
    if (!clicked) {
      logger.warn('Could not find a Start Attempt/Continue button on the gate page — stopping', { attempt });
      return { status: 'no-gate', attempt };
    }
    await driver.sleep(1500);

    const result = await runExerciseFn(driver);
    logger.info('Checkpoint attempt finished', { attempt, ...result });

    if (result.status === 'unhandled') {
      // A structural gap, not a wrong guess — retrying hits the same
      // unhandled question type again, so stop right away instead of
      // burning attempts against the cap.
      logger.warn('Unhandled question type inside checkpoint — stopping', { attempt, questionNum: result.questionNum });
      return { status: 'unhandled', attempt, questionNum: result.questionNum };
    }

    if (result.score === 100) {
      logger.info('Checkpoint passed', { attempt, score: result.score });
      return { status: 'passed', attempt, score: result.score };
    }

    logger.warn('Checkpoint attempt did not reach a passing score — retrying', { attempt, score: result.score });
    await driver.switchTo().defaultContent();
    await driver.get(gateUrl);
    await driver.sleep(1500);
  }

  logger.warn('Checkpoint not passed after max attempts', { attempts: MAX_ATTEMPTS });
  return { status: 'exhausted', attempts: MAX_ATTEMPTS };
}

if (require.main === module) {
  main().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}

module.exports = { run: main, waitForScore };
