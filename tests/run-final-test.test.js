const assert = require('node:assert/strict');
const { test } = require('node:test');
const { formatQuestionRecord, buildMarkdown, scrapeFinalTest, answerWithRetry } = require('../scripts/run-final-test');

test('formatQuestionRecord builds an mcq record from readingComprehension-shaped data', () => {
  const record = formatQuestionRecord({
    number: 3,
    handlerName: 'readingComprehension',
    questionData: { text: 'Which sentence uses the past perfect correctly?', options: [{ letter: 'A', text: 'She go.' }, { letter: 'B', text: 'She had gone.' }] },
    answer: 'B',
  });
  assert.deepEqual(record, {
    number: 3,
    type: 'mcq',
    text: 'Which sentence uses the past perfect correctly?',
    options: [{ letter: 'A', text: 'She go.' }, { letter: 'B', text: 'She had gone.' }],
    answer: 'B',
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
    { number: 3, type: 'mcq', text: 'Which sentence uses the past perfect correctly?', options: [{ letter: 'A', text: 'She go.' }, { letter: 'B', text: 'She had gone.' }], answer: 'B' },
  ]);
  assert.equal(
    md,
    '## Q3\nWhich sentence uses the past perfect correctly?\nA. She go.\nB. She had gone.\n**Jawaban: B**\n'
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
    { number: 1, type: 'mcq', text: 'Q one?', options: [{ letter: 'A', text: 'x' }], answer: 'A' },
    { number: 2, type: 'listening' },
  ]);
  assert.equal(md, '## Q1\nQ one?\nA. x\n**Jawaban: A**\n\n## Q2 (listening — skipped, jawab manual)\n');
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
  const answerQuestionFn = async () => ({ answer: 'C' });
  const result = await answerWithRetry({}, 'model', { text: 'q', options: [] }, { answerQuestionFn });
  assert.equal(result, 'C');
});

test('answerWithRetry retries up to maxAttempts then returns NEEDS REVIEW', async () => {
  let calls = 0;
  const answerQuestionFn = async () => { calls += 1; throw new Error('boom'); };
  const result = await answerWithRetry({}, 'model', { text: 'q', options: [] }, { answerQuestionFn, maxAttempts: 3, sleepFn: async () => {} });
  assert.equal(result, 'NEEDS REVIEW');
  assert.equal(calls, 3);
});

test('answerWithRetry recovers if a later attempt succeeds', async () => {
  let calls = 0;
  const answerQuestionFn = async () => {
    calls += 1;
    if (calls < 2) throw new Error('boom');
    return { answer: 'D' };
  };
  const result = await answerWithRetry({}, 'model', { text: 'q', options: [] }, { answerQuestionFn, maxAttempts: 3, sleepFn: async () => {} });
  assert.equal(result, 'D');
  assert.equal(calls, 2);
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
    answerQuestionFn: async () => ({ answer: 'A' }),
    pillDomLookupFn: (n) => pillDoms[n],
  });
  assert.equal(result.status, 'complete');
  assert.deepEqual(result.records, [
    { number: 1, type: 'mcq', text: 'Q1?', options: [{ letter: 'A', text: 'x' }], answer: 'A' },
    { number: 2, type: 'listening' },
    { number: 3, type: 'unhandled' },
  ]);
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
