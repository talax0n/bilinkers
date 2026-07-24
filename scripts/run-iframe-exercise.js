const { Builder, By } = require('selenium-webdriver');
const chrome = require('selenium-webdriver/chrome');
const { config } = require('../src/config');
const { getCurrentQuestionDom } = require('../src/browser');
const { createRegistry } = require('../src/questionTypes/registry');
const fillInBlank = require('../src/questionTypes/fillInBlank');
const errorAnalysis = require('../src/questionTypes/errorAnalysis');
const vocabularyIntro = require('../src/questionTypes/vocabularyIntro');
const quizMatching = require('../src/questionTypes/quizMatching');
const { processQuestion } = require('../src/runner');
const { createClient, answerQuestion } = require('../src/llm');
const { resolveChromedriverPath } = require('../src/chromedriver');
const logger = require('../src/logger');

// errorAnalysis has no entry here — it's answered by brute force
// (createBruteForceAnswerer below), not an LLM call, so no format note is
// ever read for it.
const FORMAT_NOTES = {
  fillInBlank: 'If multiple blanks, return {"answers": [...]} in order; if one blank, return {"answer": "..."}.',
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

// vocabularyIntro pages aren't graded and have nothing to answer, so there's
// no need to burn an LLM call on them — a no-op stands in for answerQuestion,
// and the handler's own answer() already clicks "#/n" to move on, so the
// generic advance() step (which only knows about .quiz-next-btn / #/question-N)
// is skipped for this type instead of running and wrongly declaring the
// activity complete. quizMatching is also LLM-free (its answer is derived
// deterministically from the page's own data-drag/data-drag-target indexes,
// not guessed), but it isn't self-advancing — it uses the normal
// .quiz-next-btn Continue flow once Check passes, same as fillInBlank/errorAnalysis.
const SKIP_LLM_TYPES = new Set(['quizMatching']);
const SELF_ADVANCING_TYPES = new Set();
// Non-graded presentation slides — walked forward, never counted or graded
// as questions (see the main loop).
const PRESENTATION_TYPES = new Set(['vocabularyIntro']);
const noopAnswer = async () => ({});

// errorAnalysis is answered by brute force instead of an LLM call: try A,
// then B, then C, ... in order, one per retry, until Check passes. Simpler
// and more reliable than reasoning about grammar through a model — there
// are at most a handful of candidates, so exhausting them is cheap, and
// runner.js's own retry loop (one attempt per call here) already drives
// the "try next, check, repeat" sequence. A fresh instance per question
// keeps the attempt counter from leaking across questions.
function createBruteForceAnswerer() {
  let attemptIndex = 0;
  return async (client, model, instruction, questionData) => {
    const candidate = questionData.candidates[attemptIndex];
    attemptIndex += 1;
    if (!candidate) {
      throw new Error('errorAnalysis brute force: exhausted all candidate letters without a correct answer');
    }
    return { answer: candidate.letter };
  };
}
const BRUTE_FORCE_TYPES = new Set(['errorAnalysis']);

// existingDriver lets an orchestrator (run-unit.js) drive one continuous
// browser session across several activities instead of each script
// re-attaching its own driver; standalone invocation (via cli.js or `node
// scripts/run-iframe-exercise.js` directly) still attaches its own as before.
async function main(existingDriver) {
  const driver = existingDriver || (await attachToBrave());
  const registry = createRegistry();
  registry.register(fillInBlank);
  registry.register(errorAnalysis);
  registry.register(quizMatching);
  registry.register(vocabularyIntro);

  const llmClient = createClient(config);

  let questionNum = 0;
  let slideNum = 0;
  for (;;) {
    slideNum += 1;
    const dom = await getCurrentQuestionDom(driver);

    // The activity's final slide ("You can now close this activity and
    // continue to the next one") has no #/n link and no quiz markers, so it
    // never matches a registered type — that's expected, not an error.
    if (dom.outerHTML.includes('You can now close this activity')) {
      logger.info('Reached activity closing screen — done', { questions: questionNum });
      return { status: 'complete', questionNum };
    }

    const handler = registry.findHandler(dom);

    if (!handler) {
      logger.saveUnhandled(config.paths.unhandledLogDir, `run-iframe-exercise-slide${slideNum}`, { html: dom.outerHTML });
      logger.warn('Unhandled question type — stopping', { slideNum });
      return { status: 'unhandled', slideNum };
    }

    // Presentation slides (a vocabulary unit's word / meaning-and-example
    // cards) sit between the real questions in the same iframe SPA. They're
    // not graded and have nothing to answer — just walk them forward via
    // their own #/n link, without counting or grading them as questions
    // (otherwise a vocab unit logs dozens of "Question processed" lines for
    // slides that were never questions).
    if (PRESENTATION_TYPES.has(handler.name)) {
      await handler.answer(dom.driver);
      await dom.driver.sleep(300 + Math.random() * 300);
      continue;
    }

    questionNum += 1;

    // Reading/thinking delay before answering, so submissions don't land
    // suspiciously instantly after the question loads.
    await dom.driver.sleep(500 + Math.random() * 500);

    const questionData = handler.parse(dom);
    const skipLlm = SKIP_LLM_TYPES.has(handler.name);
    const bruteForce = BRUTE_FORCE_TYPES.has(handler.name);

    const result = await processQuestion({
      driver: dom.driver,
      dom,
      registry,
      llmClient,
      model: config.openai.model,
      instruction: buildInstruction(handler, questionData),
      retryLimit: skipLlm ? 0 : MAX_ANSWER_RETRIES,
      answerQuestionFn: bruteForce ? createBruteForceAnswerer() : skipLlm ? noopAnswer : answerQuestion,
    });

    logger.info('Question processed', { questionNum, type: handler.name, ...result });

    if (result.status === 'incorrect') {
      logger.warn('Still incorrect after all retries — stopping instead of advancing', { questionNum });
      return { status: 'incorrect', questionNum };
    }

    if (SELF_ADVANCING_TYPES.has(handler.name)) {
      continue;
    }

    const advanced = await advance(dom.driver);
    if (!advanced) {
      logger.info('Could not advance (no Continue link, no next question hash) — activity complete', { questionNum });
      return { status: 'complete', questionNum };
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
