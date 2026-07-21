const { Builder, By } = require('selenium-webdriver');
const chrome = require('selenium-webdriver/chrome');
const { config } = require('../src/config');
const { getCurrentQuestionDom } = require('../src/browser');
const { createRegistry } = require('../src/questionTypes/registry');
const fillInBlank = require('../src/questionTypes/fillInBlank');
const errorAnalysis = require('../src/questionTypes/errorAnalysis');
const { processQuestion } = require('../src/runner');
const { createClient, answerQuestion } = require('../src/llm');
const logger = require('../src/logger');

const INSTRUCTIONS = {
  fillInBlank:
    'Fill in the blank(s) in the sentence with the correct word/phrase. If multiple blanks, return {"answers": [...]} in order; if one blank, return {"answer": "..."}.',
  errorAnalysis:
    'The sentence has 4 candidate words, each labeled with a letter (see questionData.candidates). Exactly one of them is grammatically incorrect. Return the letter of the incorrect word as {"answer": "<letter>"}.',
};

// This script attaches to an already-running Brave instance launched with
// --remote-debugging-port=9222 and answers every question inside the
// currently open Beelingua LTI-embedded activity (Bits player), advancing
// via the iframe's own "Continue" link. Unlike run-exercise.js (the native
// MUI app), this activity's Check-once-per-load UI means a wrong answer
// cannot be retried in place, so retryLimit is 0 here.
const CHROMEDRIVER_PATH = '/Users/theo/.cache/selenium/chromedriver/mac-arm64/150.0.7871.124/chromedriver';

async function attachToBrave() {
  const options = new chrome.Options();
  options.debuggerAddress('localhost:9222');
  const service = new chrome.ServiceBuilder(CHROMEDRIVER_PATH);
  const driver = await new Builder().forBrowser('chrome').setChromeOptions(options).setChromeService(service).build();

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

    const result = await processQuestion({
      driver: dom.driver,
      dom,
      registry,
      llmClient,
      model: config.openai.model,
      instruction: INSTRUCTIONS[handler.name] || '',
      retryLimit: 0,
      answerQuestionFn: answerQuestion,
    });

    logger.info('Question processed', { questionNum, type: handler.name, ...result });

    const advanced = await advance(dom.driver);
    if (!advanced) {
      logger.info('Could not advance (no Continue link, no next question hash) — activity complete', { questionNum });
      break;
    }
    await dom.driver.sleep(500);
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
