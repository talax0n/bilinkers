const { Builder } = require('selenium-webdriver');
const chrome = require('selenium-webdriver/chrome');
const { config } = require('../src/config');
const { getCurrentQuestionDom, goToNextQuestion } = require('../src/browser');
const { createRegistry } = require('../src/questionTypes/registry');
const audioMultipleChoice = require('../src/questionTypes/audioMultipleChoice');
const readingComprehension = require('../src/questionTypes/readingComprehension');
const { processQuestion } = require('../src/runner');
const { createClient, answerQuestion } = require('../src/llm');
const logger = require('../src/logger');

// This script attaches to an already-running Brave instance launched with
// --remote-debugging-port=9222 (so your logged-in session/cookies are reused
// instead of requiring a fresh manual login) and answers every question in
// whichever Beelingua exercise tab is currently open, in place.
const CHROMEDRIVER_PATH = '/Users/theo/.cache/selenium/chromedriver/mac-arm64/150.0.7871.124/chromedriver';

// audioMultipleChoice can't be answered intelligently (the bot can't hear the
// audio), so it cycles every option until Check reports correct instead of
// trusting an LLM guess.
const BLAST_TYPES = new Set(['audioMultipleChoice']);

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

function blastAnswerFn(options) {
  let attempt = 0;
  return async () => {
    const option = options[attempt % options.length];
    attempt += 1;
    return { answer: option.letter };
  };
}

async function main() {
  const driver = await attachToBrave();
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
      logger.saveUnhandled(config.paths.unhandledLogDir, `run-exercise-q${questionNum}`, { html: dom.outerHTML });
      logger.warn('Unhandled question type — stopping', { questionNum });
      break;
    }

    const questionData = handler.parse(dom);
    const isBlastType = BLAST_TYPES.has(handler.name);

    const result = await processQuestion({
      driver,
      dom,
      registry,
      llmClient,
      model: config.openai.model,
      instruction: 'Determine the single correct option letter for this question, using the passage/question text and options provided. Respond as JSON: { "answer": "<letter>" }.',
      retryLimit: isBlastType ? questionData.options.length - 1 : config.retry.maxAnswerRetries,
      answerQuestionFn: isBlastType ? blastAnswerFn(questionData.options) : answerQuestion,
    });

    logger.info('Question processed', { questionNum, type: handler.name, ...result });

    const advanced = await goToNextQuestion(driver);
    if (!advanced) {
      logger.info('No further "Next" button — exercise complete', { questionNum });
      break;
    }
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
