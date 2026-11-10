const { By } = require('selenium-webdriver');
const { config } = require('../config');
const { attachToLms, getCurrentQuestionDom } = require('../browser');
const { createRegistry } = require('../questionTypes/registry');
const fillInBlank = require('../questionTypes/fillInBlank');
const errorAnalysis = require('../questionTypes/errorAnalysis');
const vocabularyIntro = require('../questionTypes/vocabularyIntro');
const quizMatching = require('../questionTypes/quizMatching');
const { processQuestion } = require('../answerLoop');
const { createClient, answerQuestion } = require('../llm');
const logger = require('../logger');

// Site behavior this relies on: docs/platform-notes.md#lti-iframe-player

const FORMAT_NOTES = {
  // The on-screen worked example answers in a full sentence, and the model copies that shape.
  fillInBlank:
    'Return ONLY the word(s) that fill the blank(s) — never the surrounding sentence, even though the worked example above shows the answer inside a full sentence. If multiple blanks, return {"answers": [...]} in order (each entry just the words for that one blank); if one blank, return {"answer": "..."} with just those words.',
};

// Wording and the graded rule vary per question, so the on-screen instruction is used verbatim.
function buildInstruction(handler, questionData) {
  const onScreenInstruction = questionData.instructionText || 'Answer the question as shown.';
  return `${onScreenInstruction} ${FORMAT_NOTES[handler.name] || ''}`;
}

async function clickContinue(driver) {
  return driver.executeScript(`
    const link = document.querySelector('.quiz-next-btn');
    if (link && getComputedStyle(link).display !== 'none') { link.click(); return true; }
    return false;
  `);
}

async function readActivityState(driver) {
  return driver.executeScript(`
    const text = document.querySelector('#content')?.innerText || '';
    const question = text.match(/Question\\s+\\d+\\s+of\\s+\\d+/i)?.[0] || null;
    return {
      url: window.location.href,
      question,
      // The "Question N of M" counter is itself sometimes stale — live-verified
      // on a fillInBlank activity where clicking Continue after a correct
      // answer advanced the hash *and* the actual question sentence
      // underneath, but left this counter frozen on the old number forever,
      // making 'question' alone a false negative (advance() would wait the
      // full timeout every time). The full #content text changes whenever
      // either the counter or the underlying question content changes, so
      // it's used as the real signal; 'question' is kept only for logging.
      content: text,
      closing: text.includes('You can now close this activity'),
    };
  `);
}

async function waitForActivityChange(driver, previous, timeoutMs = 60000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const current = await readActivityState(driver);
    if (current.closing || current.content !== previous.content) return current;
    await driver.sleep(200);
  }
  return null;
}

async function advance(driver, previousState, timeoutMs = 60000) {
  const advanced = await clickContinue(driver);
  if (advanced) return Boolean(await waitForActivityChange(driver, previousState, timeoutMs));
  return false;
}

// errorAnalysis brute force. Try letter subsets by size (A, B, ..., A+B, ...), one per retry.
function getLetterCombinations(letters) {
  const combinations = [];

  function build(startIndex, subsetSize, current) {
    if (current.length === subsetSize) {
      combinations.push([...current]);
      return;
    }

    for (let i = startIndex; i <= letters.length - (subsetSize - current.length); i += 1) {
      current.push(letters[i]);
      build(i + 1, subsetSize, current);
      current.pop();
    }
  }

  for (let subsetSize = 1; subsetSize <= letters.length; subsetSize += 1) {
    build(0, subsetSize, []);
  }

  return combinations;
}

function createBruteForceAnswerer() {
  let attemptIndex = 0;
  let combinations = null;

  return async (client, model, instruction, questionData) => {
    if (!combinations) {
      combinations = getLetterCombinations(questionData.candidates.map((candidate) => candidate.letter));
    }

    const candidateLetters = combinations[attemptIndex];
    attemptIndex += 1;

    if (!candidateLetters) {
      throw new Error('errorAnalysis brute force: exhausted all candidate-letter combinations without a correct answer');
    }

    return { answers: candidateLetters };
  };
}

// registry.findHandler() returns the first match, so order matters.
// The strategy belongs to this runner. checkpoint.js and final-test.js solve the same pages differently.
const QUESTION_TYPES = [
  { type: fillInBlank, solve: 'llm' },
  { type: errorAnalysis, solve: 'letter-combinations' },
  // Answer derived from the page's own data-drag/data-drag-target indexes.
  { type: quizMatching, solve: 'from-page' },
  // Ungraded slides. answer() clicks #/n itself, so the generic advance() is skipped.
  { type: vocabularyIntro, solve: 'click-through' },
];
const SOLVE_BY_NAME = new Map(QUESTION_TYPES.map(({ type, solve }) => [type.name, solve]));

function answererFor(solve) {
  if (solve === 'llm') return answerQuestion;
  if (solve === 'letter-combinations') return createBruteForceAnswerer();
  return async () => ({});
}

async function main(existingDriver) {
  const driver = existingDriver || (await attachToLms());
  const registry = createRegistry();
  QUESTION_TYPES.forEach(({ type }) => registry.register(type));

  const llmClient = createClient(config);

  let questionNum = 0;
  let slideNum = 0;
  for (;;) {
    slideNum += 1;
    const dom = await getCurrentQuestionDom(driver);
    const activityState = await readActivityState(dom.driver);

    // The closing slide matches no handler. That is the normal end.
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

    const solve = SOLVE_BY_NAME.get(handler.name);
    // Vocabulary slides are walked without counting them as questions.
    if (solve === 'click-through') {
      await handler.answer(dom.driver);
      await dom.driver.sleep(100 + Math.random() * 100);
      continue;
    }

    questionNum += 1;

    // Short reading delay so submissions don't land instantly.
    await dom.driver.sleep(100 + Math.random() * 100);

    const questionData = handler.parse(dom);

    const result = await processQuestion({
      driver: dom.driver,
      dom,
      registry,
      llmClient,
      model: config.openai.model,
      instruction: buildInstruction(handler, questionData),
      retryLimit: solve === 'from-page' ? 0 : config.retry.maxAnswerRetries,
      answerQuestionFn: answererFor(solve),
    });

    logger.info('Question processed', { questionNum, type: handler.name, ...result });

    if (result.status === 'incorrect') {
      logger.warn('Still incorrect after all retries — stopping instead of advancing', { questionNum });
      return { status: 'incorrect', questionNum };
    }

    const advanced = await advance(dom.driver, activityState);
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

module.exports = { run: main, readActivityState, waitForActivityChange, advance, createBruteForceAnswerer, QUESTION_TYPES };
