const { config } = require('../config');
const { attachToLms, getCurrentQuestionDom, goToNextQuestion } = require('../browser');
const { createRegistry } = require('../questionTypes/registry');
const audioMultipleChoice = require('../questionTypes/audioMultipleChoice');
const readingComprehension = require('../questionTypes/readingComprehension');
const { processQuestion } = require('../answerLoop');
const logger = require('../logger');

// This script attaches to an already-running Chromium-based browser (Chrome,
// Brave, Edge) launched with --remote-debugging-port=9222 (so your logged-in
// session/cookies are reused instead of requiring a fresh manual login) and
// answers every question in whichever Beelingua exercise tab is currently
// open, in place. Run via `node src/cli.js exercise`, which handles the
// browser launch + login wait for you, or invoke this file directly if the
// browser is already up.

// Every question here is solved by trying each option until Check reports
// correct: listening questions can't be heard, and A-D with in-place Check
// feedback is cheaper and more reliable to exhaust than to ask an LLM about.
function tryEachOption(options) {
  let attempt = 0;
  return async () => {
    const option = options[attempt % options.length];
    attempt += 1;
    return { answer: option.letter };
  };
}

// Some native exercises (verified live: "Multiple Choice Grammar Activity",
// a 15-question numbered-pill layout — 1..15 across the top, no per-question
// "Next" button at all) require an explicit final Submit once every question
// is answered, gated behind an "Are you sure?" Yes/No confirmation, before
// the attempt is actually recorded — without this, the questions can all be
// answered correctly and the exercise still shows as unfinished. Submit ->
// Yes lands on a result screen ("Excellent! You Passed! Your Score: 100")
// with its own "Next" button that returns straight to the unit's activity
// list (verified live: URL dropped back to the plain session URL, progress
// bar advanced) — clicked here too so the activity is fully closed out
// rather than left sitting on the result screen. No-op when Submit isn't
// present, since the reading/audio flow's own "no further Next button" is
// already a complete, correct finish on its own.
async function submitIfPresent(driver) {
  const submitted = await driver.executeScript(`
    const buttons = Array.from(document.querySelectorAll('button'));
    const btn = buttons.find((b) => b.textContent.trim() === 'Submit');
    if (btn && !btn.disabled) { btn.click(); return true; }
    return false;
  `);
  if (!submitted) return { submitted: false };

  await driver.sleep(1000 + Math.random() * 500);

  await driver.executeScript(`
    const buttons = Array.from(document.querySelectorAll('button'));
    const btn = buttons.find((b) => b.textContent.trim() === 'Yes');
    if (btn && !btn.disabled) { btn.click(); return true; }
    return false;
  `);
  await driver.sleep(1500);

  // The result screen ("Excellent! You Passed! Your Score: 100") takes a
  // beat longer than a fixed sleep to render (score/confetti animation) — a
  // single immediate click attempt right after can miss the "Next" button
  // entirely. Read the score and click Next in the same executeScript call
  // each poll so the score text is captured from the same render pass that
  // triggers the click, instead of racing a separate read against Next
  // navigating away.
  const deadline = Date.now() + 8000;
  let score;
  while (Date.now() < deadline) {
    const result = await driver.executeScript(`
      const scoreMatch = document.body.textContent.match(/Your Score:\\s*(\\d+)/);
      const buttons = Array.from(document.querySelectorAll('button'));
      const btn = buttons.find((b) => b.textContent.trim() === 'Next');
      const clicked = Boolean(btn && !btn.disabled);
      if (clicked) btn.click();
      return { score: scoreMatch ? Number(scoreMatch[1]) : null, clicked };
    `);
    if (result.score !== null && score === undefined) score = result.score;
    if (result.clicked) break;
    await driver.sleep(300);
  }
  await driver.sleep(1500);

  return { submitted: true, score };
}

// The same numbered-pill layout Submit needs also has no "Next" button to
// advance between questions — moving on means clicking the next question's
// own pill (1..15 across the top) instead.
async function clickPillNumber(driver, number) {
  return driver.executeScript(
    `
    const target = String(arguments[0]);
    const buttons = Array.from(document.querySelectorAll('button'));
    const btn = buttons.find((b) => b.textContent.trim() === target);
    if (btn && !btn.disabled) { btn.click(); return true; }
    return false;
  `,
    number
  );
}

// existingDriver lets an orchestrator (runners/unit.js) drive one continuous
// browser session across several activities instead of each script
// re-attaching its own driver; standalone invocation (via cli.js or `node
// src/runners/exercise.js` directly) still attaches its own as before.
async function main(existingDriver) {
  const driver = existingDriver || (await attachToLms());
  const registry = createRegistry();
  registry.register(audioMultipleChoice);
  registry.register(readingComprehension);

  let questionNum = 0;
  for (;;) {
    questionNum += 1;
    // Native MUI exercises never render an iframe, so the default 30s
    // iframe-wait (meant for LTI activities) would otherwise be burned in
    // full on every single question before falling through.
    const dom = await getCurrentQuestionDom(driver, { iframeWaitMs: 300 });
    const handler = registry.findHandler(dom);

    if (!handler) {
      logger.saveUnhandled(config.paths.unhandledLogDir, `run-exercise-q${questionNum}`, { html: dom.outerHTML });
      logger.warn('Unhandled question type — stopping', { questionNum });
      return { status: 'unhandled', questionNum };
    }

    const questionData = handler.parse(dom);

    const result = await processQuestion({
      driver,
      dom,
      registry,
      retryLimit: questionData.options.length - 1,
      answerQuestionFn: tryEachOption(questionData.options),
    });

    logger.info('Question processed', { questionNum, type: handler.name, ...result });

    let advanced = await goToNextQuestion(driver);
    if (!advanced) {
      advanced = await clickPillNumber(driver, questionNum + 1);
    }
    if (!advanced) {
      const submitResult = await submitIfPresent(driver);
      logger.info(submitResult.submitted ? 'Submitted the exercise' : 'No further "Next" button or pill — exercise complete', { questionNum });
      return { status: 'complete', questionNum, score: submitResult.score };
    }
  }
}

if (require.main === module) {
  main().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}

module.exports = { run: main, submitIfPresent };
