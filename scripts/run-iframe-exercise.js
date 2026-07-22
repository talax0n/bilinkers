const { Builder, By } = require('selenium-webdriver');
const chrome = require('selenium-webdriver/chrome');
const { config } = require('../src/config');
const { getCurrentQuestionDom } = require('../src/browser');
const { createRegistry } = require('../src/questionTypes/registry');
const fillInBlank = require('../src/questionTypes/fillInBlank');
const errorAnalysis = require('../src/questionTypes/errorAnalysis');
const { processQuestion } = require('../src/runner');
const { createClient, answerQuestion } = require('../src/llm');
const { resolveChromedriverPath } = require('../src/chromedriver');
const logger = require('../src/logger');

const FORMAT_NOTES = {
  fillInBlank: 'If multiple blanks, return {"answers": [...]} in order; if one blank, return {"answer": "..."}.',
  errorAnalysis:
    'The sentence has candidate words, each labeled with a letter (see questionData.candidates). ' +
    'There can be more than one matching letter. Return {"answer": "<letter>"} for a single answer, or {"answers": ["<letter>", ...]} if more than one applies.',
};

// Neither question type has a fixed task — the on-screen instruction (read
// straight off the page into questionData.instructionText by the parser,
// since wording and the graded tense/rule vary per question) is the only
// source of truth for what to do, used verbatim rather than guessed at.
function buildInstruction(handler, questionData) {
  const onScreenInstruction = questionData.instructionText || 'Answer the question as shown.';
  return `${onScreenInstruction} ${FORMAT_NOTES[handler.name] || ''}`;
}

// This script attaches to an already-running Chromium-based browser (Chrome,
// Brave, Edge) launched with --remote-debugging-port=9222 and answers every
// question inside the currently open Beelingua LTI-embedded activity (Bits
// player), advancing via the iframe's own "Continue" link. Unlike
// run-exercise.js (the native MUI app), this activity's Check-once-per-load
// UI means a wrong answer cannot be retried in place, so retryLimit is 0
// here. Run via `node scripts/cli.js iframe`, which handles the browser
// launch + login wait for you, or invoke this file directly if the browser
// is already up.
// CHROMEDRIVER_PATH overrides auto-detection below (matches chromedriver to
// whatever's actually listening on the debug port) — only needed if that
// fails for your setup.
const CHROMEDRIVER_PATH = process.env.CHROMEDRIVER_PATH;

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

async function clickContinue(driver) {
  return driver.executeScript(`
    const link = document.querySelector('.quiz-next-btn');
    if (link && getComputedStyle(link).display !== 'none') { link.click(); return true; }
    return false;
  `);
}

// A wrong answer locks this activity's Check-once UI: the "Continue" link
// stays hidden and re-checking doesn't re-evaluate. The only way forward is
// the SPA's own hash router, which accepts direct navigation regardless of
// what's visually unlocked.
async function goToNextQuestionByHash(driver) {
  const currentUrl = await driver.executeScript('return window.location.href');
  const match = currentUrl.match(/#\/question-(\d+)/);
  if (!match) return false;

  const next = parseInt(match[1], 10) + 1;
  await driver.executeScript(`window.location.hash = '#/question-${next}';`);
  await driver.sleep(1000);

  const newUrl = await driver.executeScript('return window.location.href');
  return newUrl.includes(`#/question-${next}`);
}

async function advance(driver) {
  const advanced = await clickContinue(driver);
  if (advanced) return true;
  return goToNextQuestionByHash(driver);
}

// Check re-evaluates on every click (verified live: cycling radio options and
// re-clicking Check flips .correct.timeout/.incorrect.timeout each time), so
// a wrong answer can be retried in place instead of restarting the activity.
const MAX_ANSWER_RETRIES = 10;

async function main() {
  const driver = await attachToBrave();
  const registry = createRegistry();
  registry.register(fillInBlank);
  registry.register(errorAnalysis);

  const llmClient = createClient(config);

  let questionNum = 0;
  for (;;) {
    questionNum += 1;
    const dom = await getCurrentQuestionDom(driver);
    const handler = registry.findHandler(dom);

    if (!handler) {
      logger.saveUnhandled(config.paths.unhandledLogDir, `run-iframe-exercise-q${questionNum}`, { html: dom.outerHTML });
      logger.warn('Unhandled question type — stopping', { questionNum });
      break;
    }

    // Reading/thinking delay before answering, so submissions don't land
    // suspiciously instantly after the question loads.
    await dom.driver.sleep(500 + Math.random() * 500);

    const questionData = handler.parse(dom);

    const result = await processQuestion({
      driver: dom.driver,
      dom,
      registry,
      llmClient,
      model: config.openai.model,
      instruction: buildInstruction(handler, questionData),
      retryLimit: MAX_ANSWER_RETRIES,
      answerQuestionFn: answerQuestion,
    });

    logger.info('Question processed', { questionNum, type: handler.name, ...result });

    if (result.status === 'incorrect') {
      logger.warn('Still incorrect after all retries — stopping instead of advancing', { questionNum });
      break;
    }

    const advanced = await advance(dom.driver);
    if (!advanced) {
      logger.info('Could not advance (no Continue link, no next question hash) — activity complete', { questionNum });
      break;
    }
    await dom.driver.sleep(1500 + Math.random() * 1500);
  }
}

if (require.main === module) {
  main().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}

module.exports = { run: main };
