const { config } = require('../config');
const { attachToLms, getCurrentQuestionDom, goToNextQuestion } = require('../browser');
const { createRegistry } = require('../questionTypes/registry');
const audioMultipleChoice = require('../questionTypes/audioMultipleChoice');
const readingComprehension = require('../questionTypes/readingComprehension');
const { processQuestion } = require('../answerLoop');
const logger = require('../logger');

// Site behavior this relies on: docs/platform-notes.md#native-exercise

// No LLM. Listening can't be heard, and in-place Check makes trying each option cheap.
function tryEachOption(options) {
  let attempt = 0;
  return async () => {
    const option = options[attempt % options.length];
    attempt += 1;
    return { answer: option.letter };
  };
}

// Numbered-pill exercises record the attempt only after Submit, Yes, then Next on the result screen.
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

  // The result screen animates in. Read the score and click Next in the same script.
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

// Numbered-pill layouts have no Next button. Advance by clicking the next pill.
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

async function main(existingDriver) {
  const driver = existingDriver || (await attachToLms());
  const registry = createRegistry();
  registry.register(audioMultipleChoice);
  registry.register(readingComprehension);

  let questionNum = 0;
  for (;;) {
    questionNum += 1;
    // Native exercises have no iframe, so skip the long LTI iframe wait.
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
