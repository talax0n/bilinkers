const assert = require('node:assert/strict');
const { test } = require('node:test');
const { processQuestion } = require('../src/runner');
const { createRegistry } = require('../src/questionTypes/registry');

test('processQuestion returns unhandled when no type module matches', async () => {
  const registry = createRegistry();
  registry.register({ name: 'mcq', detect: () => false });
  const result = await processQuestion({
    driver: { sleep: async () => {} },
    dom: {},
    registry,
    llmClient: {},
    model: 'gpt-4o',
    instruction: 'instr',
    retryLimit: 1,
  });
  assert.deepEqual(result, { status: 'unhandled', attempts: 0 });
});

test('processQuestion succeeds on first attempt when correct', async () => {
  const registry = createRegistry();
  const answerCalls = [];
  registry.register({
    name: 'mcq',
    detect: () => true,
    parse: () => ({ text: 'q' }),
    answer: async (driver, llmResult) => { answerCalls.push(llmResult); },
    checkResult: async () => 'correct',
  });

  const answerQuestionFn = async () => ({ answer: 'A' });

  const result = await processQuestion({
    driver: { sleep: async () => {} },
    dom: {},
    registry,
    llmClient: {},
    model: 'gpt-4o',
    instruction: 'instr',
    retryLimit: 1,
    answerQuestionFn,
  });

  assert.deepEqual(result, { status: 'correct', attempts: 1 });
  assert.equal(answerCalls.length, 1);
});

test('processQuestion retries once with feedback then gives up', async () => {
  const registry = createRegistry();
  const feedbacks = [];
  registry.register({
    name: 'mcq',
    detect: () => true,
    parse: () => ({ text: 'q' }),
    answer: async () => {},
    checkResult: async () => 'incorrect',
  });

  const answerQuestionFn = async (client, model, instruction, questionData, feedback) => {
    feedbacks.push(feedback);
    return { answer: 'A' };
  };

  const result = await processQuestion({
    driver: { sleep: async () => {} },
    dom: {},
    registry,
    llmClient: {},
    model: 'gpt-4o',
    instruction: 'instr',
    retryLimit: 1,
    answerQuestionFn,
  });

  assert.deepEqual(result, { status: 'incorrect', attempts: 2 });
  assert.deepEqual(feedbacks, [
    null,
    'previous answer was wrong, try again. Already tried and confirmed wrong: A — do not repeat these.',
  ]);
});

test('processQuestion surfaces a hint and lists prior wrong guesses so retries do not repeat them', async () => {
  const registry = createRegistry();
  const feedbacks = [];
  const answersTried = [];
  registry.register({
    name: 'tfng',
    detect: () => true,
    parse: () => ({ text: 'q' }),
    answer: async (driver, llmResult) => { answersTried.push(llmResult.answer); },
    checkResult: async () => ({ outcome: 'incorrect', hint: 'Reread paragraph 3.' }),
  });

  let call = 0;
  const answerQuestionFn = async (client, model, instruction, questionData, feedback) => {
    feedbacks.push(feedback);
    call += 1;
    return { answer: call === 1 ? 'A' : 'B' };
  };

  const result = await processQuestion({
    driver: { sleep: async () => {} },
    dom: {},
    registry,
    llmClient: {},
    model: 'gpt-4o',
    instruction: 'instr',
    retryLimit: 1,
    answerQuestionFn,
  });

  assert.deepEqual(result, { status: 'incorrect', attempts: 2 });
  assert.deepEqual(answersTried, ['A', 'B']);
  assert.deepEqual(feedbacks, [
    null,
    'previous answer was wrong. Hint: Reread paragraph 3. Already tried and confirmed wrong: A — do not repeat these.',
  ]);
});

test('processQuestion treats a throwing handler as incorrect and still respects retryLimit', async () => {
  const registry = createRegistry();
  registry.register({
    name: 'mcq',
    detect: () => true,
    parse: () => ({ text: 'q' }),
    answer: async () => { throw new Error('stale element'); },
    checkResult: async () => 'correct',
  });

  const answerQuestionFn = async () => ({ answer: 'A' });

  const result = await processQuestion({
    driver: { sleep: async () => {} },
    dom: {},
    registry,
    llmClient: {},
    model: 'gpt-4o',
    instruction: 'instr',
    retryLimit: 1,
    answerQuestionFn,
  });

  assert.deepEqual(result, { status: 'incorrect', attempts: 2 });
});

test('processQuestion treats an unexpected checkResult value as incorrect', async () => {
  const registry = createRegistry();
  registry.register({
    name: 'mcq',
    detect: () => true,
    parse: () => ({ text: 'q' }),
    answer: async () => {},
    checkResult: async () => 'not-a-real-status',
  });

  const answerQuestionFn = async () => ({ answer: 'A' });

  const result = await processQuestion({
    driver: { sleep: async () => {} },
    dom: {},
    registry,
    llmClient: {},
    model: 'gpt-4o',
    instruction: 'instr',
    retryLimit: 0,
    answerQuestionFn,
  });

  assert.deepEqual(result, { status: 'incorrect', attempts: 1 });
});
