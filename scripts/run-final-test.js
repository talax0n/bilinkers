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
const { readPills, clickPill, ensurePillsExpanded } = require('./run-checkpoint');
const logger = require('../src/logger');

// This script attaches to an already-running Chromium-based browser sitting
// on an OPEN Final Test attempt (50 questions, 1 attempt, same pill-nav
// native quiz DOM as a checkpoint) and reads every question into a Markdown
// answer key. It never clicks an option, Save, or Submit — a Final Test has
// exactly one attempt, so there is no room to brute-force like
// run-checkpoint.js does. Listening questions (audioMultipleChoice) are
// skipped since the bot can't hear the audio. Run via
// `node scripts/cli.js final-test`, or invoke this file directly if the
// browser is already up and sitting on an open Final Test attempt.
const CHROMEDRIVER_PATH = process.env.CHROMEDRIVER_PATH;
const OUTPUT_PATH = './final-test-answers.md';

function formatQuestionRecord({ number, handlerName, questionData, answer }) {
  if (handlerName === 'audioMultipleChoice') return { number, type: 'listening' };
  if (!handlerName) return { number, type: 'unhandled' };
  return { number, type: 'mcq', text: questionData.text, options: questionData.options, answer };
}

function buildMarkdown(records) {
  return records
    .map((record) => {
      if (record.type === 'listening') return `## Q${record.number} (listening — skipped, jawab manual)\n`;
      if (record.type === 'unhandled') return `## Q${record.number} (unhandled question type — jawab manual)\n`;
      const optionLines = record.options.map((opt) => `${opt.letter}. ${opt.text}`).join('\n');
      return `## Q${record.number}\n${record.text}\n${optionLines}\n**Jawaban: ${record.answer}**\n`;
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
  'Determine the single correct option letter for this question, using the passage/question text and options provided. Respond as JSON: { "answer": "<letter>" }.';

// A single LLM guess per question — there's no per-question feedback to loop
// on (unlike run-exercise.js's retry-with-feedback) and only one real
// attempt exists, so retries here are purely for transient call failures
// (matches src/runner.js's callWithRetry), not for trying a different
// answer. Exhausting retries marks the question for manual review instead of
// throwing, so one bad question can't abort the other 49.
async function answerWithRetry(llmClient, model, questionData, { answerQuestionFn = answerQuestion, maxAttempts = 3, sleepFn = (ms) => new Promise((r) => setTimeout(r, ms)) } = {}) {
  let lastErr;
  for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
    try {
      const result = await answerQuestionFn(llmClient, model, ANSWER_INSTRUCTION, questionData, null);
      return result.answer;
    } catch (err) {
      lastErr = err;
      if (attempt < maxAttempts) await sleepFn(1000);
    }
  }
  logger.warn('Final test: LLM failed for a question after retries — marking for manual review', { error: lastErr.message });
  return 'NEEDS REVIEW';
}

// Walks every pill 1..N (N read live, not hardcoded), parsing and guessing
// each question. Read-only: only pill navigation touches the DOM, no
// option/Save/Submit click ever happens. pillDomLookupFn/getCurrentQuestionDomFn
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
  } = {}
) {
  const registry = createRegistryFn();
  registry.register(audioMultipleChoice);
  registry.register(readingComprehension);

  await ensurePillsExpandedFn(driver);
  const pills = await readPillsFn(driver);
  if (pills.length === 0) {
    injectedLogger.warn('Final test: no nav pills found — stopping', {});
    return { status: 'no-pills' };
  }

  const records = [];
  for (const { n } of pills) {
    await clickPillFn(driver, n);
    const dom = pillDomLookupFn ? pillDomLookupFn(n) : await getCurrentQuestionDomFn(driver, { iframeWaitMs: 600 });
    const handler = registry.findHandler(dom);

    if (!handler) {
      injectedLogger.warn('Final test: unhandled question type', { pill: n });
      records.push(formatQuestionRecord({ number: n, handlerName: null, questionData: null, answer: null }));
      continue;
    }

    if (handler.name === 'audioMultipleChoice') {
      injectedLogger.info('Final test: skipping listening question', { pill: n });
      records.push(formatQuestionRecord({ number: n, handlerName: handler.name, questionData: null, answer: null }));
      continue;
    }

    const questionData = handler.parse(dom);
    const answer = await answerWithRetry(llmClient, model, questionData, { answerQuestionFn });
    injectedLogger.info('Final test: question answered', { pill: n, answer });
    records.push(formatQuestionRecord({ number: n, handlerName: handler.name, questionData, answer }));
  }

  return { status: 'complete', records };
}

async function main(existingDriver) {
  const driver = existingDriver || (await attachToBrave());
  const llmClient = createClient(config);

  const result = await scrapeFinalTest(driver, { llmClient, model: config.openai.model });
  if (result.status === 'no-pills') {
    logger.warn('Final test: nothing to scrape — is the browser on an open Final Test attempt?', {});
    return result;
  }

  const markdown = buildMarkdown(result.records);
  fs.writeFileSync(OUTPUT_PATH, markdown);
  logger.info('Final test: answer key written', { path: OUTPUT_PATH, questions: result.records.length });
  return result;
}

if (require.main === module) {
  main().catch((err) => {
    logger.error('Final test scraper failed', { error: err.message });
    process.exitCode = 1;
  });
}

module.exports = { run: main, formatQuestionRecord, buildMarkdown, answerWithRetry, scrapeFinalTest };
