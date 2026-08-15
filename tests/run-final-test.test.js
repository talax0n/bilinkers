const assert = require('node:assert/strict');
const { test } = require('node:test');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { formatQuestionRecord, buildMarkdown, scrapeFinalTest, answerWithRetry, combineReview, loadPreviousAnswers, testIdFromUrl } = require('../scripts/run-final-test');

test('formatQuestionRecord builds an mcq record from readingComprehension-shaped data', () => {
  const record = formatQuestionRecord({
    number: 3,
    handlerName: 'readingComprehension',
    questionData: { text: 'Which sentence uses the past perfect correctly?', options: [{ letter: 'A', text: 'She go.' }, { letter: 'B', text: 'She had gone.' }] },
    answer: 'B',
    confidence: 0.9,
    agreements: 2,
  });
  assert.deepEqual(record, {
    number: 3,
    type: 'mcq',
    text: 'Which sentence uses the past perfect correctly?',
    options: [{ letter: 'A', text: 'She go.' }, { letter: 'B', text: 'She had gone.' }],
    answer: 'B',
    confidence: 0.9,
    agreements: 2,
  });
});

test('formatQuestionRecord builds a listening record for audioMultipleChoice, no answer field needed', () => {
  const record = formatQuestionRecord({ number: 7, handlerName: 'audioMultipleChoice', questionData: null, answer: null });
  assert.deepEqual(record, { number: 7, type: 'listening' });
});

test('formatQuestionRecord builds an unhandled record when no handler matched', () => {
  const record = formatQuestionRecord({ number: 12, handlerName: null, questionData: null, answer: null });
  assert.deepEqual(record, { number: 12, type: 'unhandled' });
});

test('buildMarkdown renders an mcq question with its options and answer', () => {
  const md = buildMarkdown([
    { number: 3, type: 'mcq', text: 'Which sentence uses the past perfect correctly?', options: [{ letter: 'A', text: 'She go.' }, { letter: 'B', text: 'She had gone.' }], answer: 'B', confidence: 0.95 },
  ]);
  assert.equal(
    md,
    '## Q3\nWhich sentence uses the past perfect correctly?\nA. She go.\nB. She had gone.\n**Jawaban: B** (confidence 0.95)\n'
  );
});

test('buildMarkdown renders a listening question as a skip line', () => {
  const md = buildMarkdown([{ number: 7, type: 'listening' }]);
  assert.equal(md, '## Q7 (listening — skipped, jawab manual)\n');
});

test('buildMarkdown renders an unhandled question as a manual-review line', () => {
  const md = buildMarkdown([{ number: 12, type: 'unhandled' }]);
  assert.equal(md, '## Q12 (unhandled question type — jawab manual)\n');
});

test('buildMarkdown joins multiple questions with a blank line between them', () => {
  const md = buildMarkdown([
    { number: 1, type: 'mcq', text: 'Q one?', options: [{ letter: 'A', text: 'x' }], answer: 'A', confidence: 0.8 },
    { number: 2, type: 'listening' },
  ]);
  assert.equal(md, '## Q1\nQ one?\nA. x\n**Jawaban: A** (confidence 0.80)\n\n## Q2 (listening — skipped, jawab manual)\n');
});

function makeDom(handlerName, questionData) {
  return { handlerName, questionData };
}

function makeFakeRegistry() {
  return () => ({
    register() {},
    findHandler(dom) {
      if (!dom.handlerName) return null;
      return { name: dom.handlerName, parse: () => dom.questionData };
    },
  });
}

test('answerWithRetry returns the LLM answer on the first success', async () => {
  const answerQuestionFn = async () => ({ answer: 'C', confidence: 0.95 });
  const result = await answerWithRetry({}, 'model', { text: 'q', options: [] }, { answerQuestionFn });
  assert.deepEqual(result, { answer: 'C', confidence: 0.95 });
});

test('answerWithRetry retries up to maxAttempts then returns NEEDS REVIEW', async () => {
  let calls = 0;
  const answerQuestionFn = async () => { calls += 1; throw new Error('boom'); };
  const result = await answerWithRetry({}, 'model', { text: 'q', options: [] }, { answerQuestionFn, maxAttempts: 3, sleepFn: async () => {} });
  assert.deepEqual(result, { answer: 'NEEDS REVIEW', confidence: 0 });
  assert.equal(calls, 3);
});

test('answerWithRetry recovers if a later attempt succeeds', async () => {
  let calls = 0;
  const answerQuestionFn = async () => {
    calls += 1;
    if (calls < 2) throw new Error('boom');
    return { answer: 'D', confidence: 0.8 };
  };
  const result = await answerWithRetry({}, 'model', { text: 'q', options: [] }, { answerQuestionFn, maxAttempts: 3, sleepFn: async () => {} });
  assert.deepEqual(result, { answer: 'D', confidence: 0.8 });
  assert.equal(calls, 2);
});

test('answerWithRetry defaults missing confidence to 0', async () => {
  const answerQuestionFn = async () => ({ answer: 'A' });
  const result = await answerWithRetry({}, 'model', { text: 'q', options: [] }, { answerQuestionFn });
  assert.deepEqual(result, { answer: 'A', confidence: 0 });
});

test('scrapeFinalTest walks every pill, answers mcq questions, skips listening, records unhandled', async () => {
  const pillDoms = {
    1: makeDom('readingComprehension', { text: 'Q1?', options: [{ letter: 'A', text: 'x' }] }),
    2: makeDom('audioMultipleChoice', { instruction: 'listen', questionText: 'Q2?', options: [] }),
    3: makeDom(null, null),
  };
  const driver = {};
  const result = await scrapeFinalTest(driver, {
    readPillsFn: async () => [{ n: 1 }, { n: 2 }, { n: 3 }],
    clickPillFn: async () => true,
    ensurePillsExpandedFn: async () => true,
    getCurrentQuestionDomFn: async () => {},
    createRegistryFn: makeFakeRegistry(),
    answerQuestionFn: async () => ({ answer: 'A', confidence: 0.95 }),
    pillDomLookupFn: (n) => pillDoms[n],
  });
  assert.equal(result.status, 'complete');
  assert.deepEqual(result.records, [
    { number: 1, type: 'mcq', text: 'Q1?', options: [{ letter: 'A', text: 'x' }], answer: 'A', confidence: 0.95, agreements: 1 },
    { number: 2, type: 'listening' },
    { number: 3, type: 'unhandled' },
  ]);
});

test('scrapeFinalTest review pass re-derives and boosts confidence on agreement', async () => {
  const pillDoms = {
    1: makeDom('readingComprehension', { text: 'Q1?', options: [{ letter: 'A', text: 'x' }, { letter: 'B', text: 'y' }] }),
  };
  const instructions = [];
  const driver = {};
  const result = await scrapeFinalTest(driver, {
    readPillsFn: async () => [{ n: 1 }],
    clickPillFn: async () => true,
    ensurePillsExpandedFn: async () => true,
    getCurrentQuestionDomFn: async () => {},
    createRegistryFn: makeFakeRegistry(),
    answerQuestionFn: async (client, model, instruction) => {
      instructions.push(instruction);
      return { answer: 'A', confidence: 0.7 };
    },
    pillDomLookupFn: (n) => pillDoms[n],
    previousAnswers: [{ number: 1, type: 'mcq', answer: 'A', confidence: 0.7 }],
  });
  assert.equal(result.status, 'complete');
  // Review instruction embedded the previous answer; agreement lifts confidence to min(0.99, 0.7+0.2).
  assert.equal(instructions[0].includes('"A"'), true);
  assert.equal(result.records[0].answer, 'A');
  assert.ok(Math.abs(result.records[0].confidence - 0.9) < 1e-9);
  assert.equal(result.records[0].agreements, 2);
});

test('scrapeFinalTest review pass caps confidence on disagreement', async () => {
  const pillDoms = {
    1: makeDom('readingComprehension', { text: 'Q1?', options: [{ letter: 'A', text: 'x' }, { letter: 'B', text: 'y' }] }),
  };
  const driver = {};
  const result = await scrapeFinalTest(driver, {
    readPillsFn: async () => [{ n: 1 }],
    clickPillFn: async () => true,
    ensurePillsExpandedFn: async () => true,
    getCurrentQuestionDomFn: async () => {},
    createRegistryFn: makeFakeRegistry(),
    answerQuestionFn: async () => ({ answer: 'B', confidence: 0.9 }),
    pillDomLookupFn: (n) => pillDoms[n],
    previousAnswers: [{ number: 1, type: 'mcq', answer: 'A', confidence: 0.8 }],
  });
  assert.equal(result.status, 'complete');
  assert.equal(result.records[0].answer, 'B');
  // min(0.9, 0.8) * 0.5 = 0.4 — never auto-clicks.
  assert.equal(result.records[0].confidence, 0.4);
});

test('combineReview returns the fresh answer when there is no previous record', () => {
  const combined = combineReview(null, { answer: 'C', confidence: 0.6 });
  assert.deepEqual(combined, { answer: 'C', confidence: 0.6, agreements: 1 });
});

test('combineReview keeps the previous answer when the review call failed', () => {
  const combined = combineReview({ answer: 'B', confidence: 0.9 }, { answer: 'NEEDS REVIEW', confidence: 0 });
  assert.deepEqual(combined, { answer: 'B', confidence: 0.9 });
});

test('testIdFromUrl strips the #/question-N hash so the same test always matches', () => {
  assert.equal(testIdFromUrl('https://lms.binus.ac.id/exam/123#/question-4'), 'https://lms.binus.ac.id/exam/123');
  assert.equal(testIdFromUrl('https://lms.binus.ac.id/exam/123'), 'https://lms.binus.ac.id/exam/123');
});

test('loadPreviousAnswers flags a different test as stale instead of reviewing', () => {
  const file = path.join(os.tmpdir(), `final-test-stale-${Date.now()}.json`);
  fs.writeFileSync(file, JSON.stringify({ testId: 'https://lms.binus.ac.id/exam/OLD', answers: [{ number: 1, type: 'mcq', answer: 'A' }] }));
  const result = loadPreviousAnswers('https://lms.binus.ac.id/exam/NEW', file);
  fs.unlinkSync(file);
  assert.equal(result.stale, true);
  assert.deepEqual(result.answers, []);
  assert.equal(result.storedTestId, 'https://lms.binus.ac.id/exam/OLD');
});

test('loadPreviousAnswers returns answers for the same test', () => {
  const file = path.join(os.tmpdir(), `final-test-same-${Date.now()}.json`);
  fs.writeFileSync(file, JSON.stringify({ testId: 'https://lms.binus.ac.id/exam/SAME', answers: [{ number: 1, type: 'mcq', answer: 'A' }] }));
  const result = loadPreviousAnswers('https://lms.binus.ac.id/exam/SAME', file);
  fs.unlinkSync(file);
  assert.equal(result.stale, false);
  assert.equal(result.answers.length, 1);
});

test('scrapeFinalTest auto-clicks and saves a confident answer when autoAnswer enabled', async () => {
  const pillDoms = {
    1: makeDom('readingComprehension', { text: 'Q1?', options: [{ letter: 'A', text: 'x' }] }),
    2: makeDom('readingComprehension', { text: 'Q2?', options: [{ letter: 'B', text: 'y' }] }),
  };
  const clicked = [];
  const saved = [];
  const driver = {};
  const result = await scrapeFinalTest(driver, {
    readPillsFn: async () => [{ n: 1 }, { n: 2 }],
    clickPillFn: async () => true,
    ensurePillsExpandedFn: async () => true,
    getCurrentQuestionDomFn: async () => {},
    createRegistryFn: makeFakeRegistry(),
    answerQuestionFn: async (client, model, instruction, qd) => ({ answer: qd.options[0].letter, confidence: 0.95 }),
    pillDomLookupFn: (n) => pillDoms[n],
    autoAnswer: true,
    confidenceThreshold: 0.9,
    clickOptionFn: async (driver, letter) => { clicked.push(letter); return true; },
    saveAndWaitFn: async (driver, n) => { saved.push(n); return true; },
  });
  assert.equal(result.status, 'complete');
  assert.deepEqual(clicked, ['A', 'B']);
  assert.deepEqual(saved, [1, 2]);
  assert.equal(result.records[0].autoAnswered, true);
});

test('scrapeFinalTest leaves answers below confidence threshold unclicked', async () => {
  const pillDoms = {
    1: makeDom('readingComprehension', { text: 'Q1?', options: [{ letter: 'A', text: 'x' }] }),
  };
  let clicked = 0;
  const driver = {};
  const result = await scrapeFinalTest(driver, {
    readPillsFn: async () => [{ n: 1 }],
    clickPillFn: async () => true,
    ensurePillsExpandedFn: async () => true,
    getCurrentQuestionDomFn: async () => {},
    createRegistryFn: makeFakeRegistry(),
    answerQuestionFn: async () => ({ answer: 'A', confidence: 0.5 }),
    pillDomLookupFn: (n) => pillDoms[n],
    autoAnswer: true,
    confidenceThreshold: 0.9,
    clickOptionFn: async () => { clicked += 1; return true; },
    saveAndWaitFn: async () => true,
  });
  assert.equal(result.status, 'complete');
  assert.equal(clicked, 0);
  assert.equal(result.records[0].autoAnswered, undefined);
});

test('scrapeFinalTest never auto-clicks listening questions', async () => {
  const pillDoms = {
    1: makeDom('audioMultipleChoice', { instruction: 'listen', questionText: 'Q1?', options: [] }),
  };
  let clicked = 0;
  const driver = {};
  const result = await scrapeFinalTest(driver, {
    readPillsFn: async () => [{ n: 1 }],
    clickPillFn: async () => true,
    ensurePillsExpandedFn: async () => true,
    getCurrentQuestionDomFn: async () => {},
    createRegistryFn: makeFakeRegistry(),
    answerQuestionFn: async () => ({ answer: 'A', confidence: 1 }),
    pillDomLookupFn: (n) => pillDoms[n],
    autoAnswer: true,
    clickOptionFn: async () => { clicked += 1; return true; },
    saveAndWaitFn: async () => true,
  });
  assert.equal(result.status, 'complete');
  assert.equal(clicked, 0);
  assert.deepEqual(result.records, [{ number: 1, type: 'listening' }]);
});

test('scrapeFinalTest guesses and saves a random listening answer when guessListening enabled', async () => {
  const pillDoms = {
    1: makeDom('audioMultipleChoice', { instruction: 'listen', questionText: 'Q1?', options: [{ letter: 'A', text: 'x' }, { letter: 'B', text: 'y' }, { letter: 'C', text: 'z' }] }),
  };
  const clicked = [];
  const saved = [];
  const driver = {};
  const result = await scrapeFinalTest(driver, {
    readPillsFn: async () => [{ n: 1 }],
    clickPillFn: async () => true,
    ensurePillsExpandedFn: async () => true,
    getCurrentQuestionDomFn: async () => {},
    createRegistryFn: makeFakeRegistry(),
    answerQuestionFn: async () => { throw new Error('LLM should not be called for listening guesses'); },
    pillDomLookupFn: (n) => pillDoms[n],
    guessListening: true,
    randomLetterFn: (letters) => letters[1],
    clickOptionFn: async (driver, letter) => { clicked.push(letter); return true; },
    saveAndWaitFn: async (driver, n) => { saved.push(n); return true; },
  });
  assert.equal(result.status, 'complete');
  assert.deepEqual(clicked, ['B']);
  assert.deepEqual(saved, [1]);
  assert.deepEqual(result.records, [{ number: 1, type: 'listening', answer: 'B', autoAnswered: true }]);
});

test('scrapeFinalTest listening guess skipped when click fails', async () => {
  const pillDoms = {
    1: makeDom('audioMultipleChoice', { instruction: 'listen', questionText: 'Q1?', options: [{ letter: 'A', text: 'x' }] }),
  };
  const driver = {};
  const result = await scrapeFinalTest(driver, {
    readPillsFn: async () => [{ n: 1 }],
    clickPillFn: async () => true,
    ensurePillsExpandedFn: async () => true,
    getCurrentQuestionDomFn: async () => {},
    createRegistryFn: makeFakeRegistry(),
    answerQuestionFn: async () => { throw new Error('should not be called'); },
    pillDomLookupFn: (n) => pillDoms[n],
    guessListening: true,
    randomLetterFn: (letters) => letters[0],
    clickOptionFn: async () => false,
    saveAndWaitFn: async () => true,
  });
  assert.equal(result.status, 'complete');
  assert.deepEqual(result.records, [{ number: 1, type: 'listening' }]);
});

test('scrapeFinalTest auto-answer honors reviewsRequired — no click below the pass threshold', async () => {
  const pillDoms = {
    1: makeDom('readingComprehension', { text: 'Q1?', options: [{ letter: 'A', text: 'x' }, { letter: 'B', text: 'y' }] }),
  };
  let clicked = 0;
  const driver = {};
  // High confidence AND one agreeing pass exists (agreements would be 2),
  // but reviewsRequired=3 demands three — no click.
  const result = await scrapeFinalTest(driver, {
    readPillsFn: async () => [{ n: 1 }],
    clickPillFn: async () => true,
    ensurePillsExpandedFn: async () => true,
    getCurrentQuestionDomFn: async () => {},
    createRegistryFn: makeFakeRegistry(),
    answerQuestionFn: async () => ({ answer: 'A', confidence: 0.95 }),
    pillDomLookupFn: (n) => pillDoms[n],
    previousAnswers: [{ number: 1, type: 'mcq', answer: 'A', confidence: 0.7, agreements: 1 }],
    autoAnswer: true,
    confidenceThreshold: 0.9,
    reviewsRequired: 3,
    clickOptionFn: async () => { clicked += 1; return true; },
    saveAndWaitFn: async () => true,
  });
  assert.equal(result.status, 'complete');
  assert.equal(clicked, 0);
  assert.equal(result.records[0].agreements, 2);
});

test('scrapeFinalTest auto-answer clicks when agreements clear reviewsRequired', async () => {
  const pillDoms = {
    1: makeDom('readingComprehension', { text: 'Q1?', options: [{ letter: 'A', text: 'x' }, { letter: 'B', text: 'y' }] }),
  };
  let clicked = 0;
  const driver = {};
  const result = await scrapeFinalTest(driver, {
    readPillsFn: async () => [{ n: 1 }],
    clickPillFn: async () => true,
    ensurePillsExpandedFn: async () => true,
    getCurrentQuestionDomFn: async () => {},
    createRegistryFn: makeFakeRegistry(),
    answerQuestionFn: async () => ({ answer: 'A', confidence: 0.95 }),
    pillDomLookupFn: (n) => pillDoms[n],
    previousAnswers: [{ number: 1, type: 'mcq', answer: 'A', confidence: 0.7, agreements: 2 }],
    autoAnswer: true,
    confidenceThreshold: 0.9,
    reviewsRequired: 3,
    clickOptionFn: async () => { clicked += 1; return true; },
    saveAndWaitFn: async () => true,
  });
  assert.equal(result.status, 'complete');
  assert.equal(clicked, 1);
  assert.equal(result.records[0].agreements, 3);
});

test('scrapeFinalTest returns no-pills status without calling the LLM when readPills is empty', async () => {
  let llmCalled = false;
  const driver = {};
  const result = await scrapeFinalTest(driver, {
    readPillsFn: async () => [],
    clickPillFn: async () => true,
    ensurePillsExpandedFn: async () => true,
    getCurrentQuestionDomFn: async () => {},
    createRegistryFn: makeFakeRegistry(),
    answerQuestionFn: async () => { llmCalled = true; return { answer: 'A' }; },
    pillDomLookupFn: () => null,
  });
  assert.deepEqual(result, { status: 'no-pills' });
  assert.equal(llmCalled, false);
});
