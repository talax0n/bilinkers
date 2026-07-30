const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { test } = require('node:test');
const { waitForActivityChange, advance, createBruteForceAnswerer } = require('../scripts/run-iframe-exercise');

test('waitForActivityChange ignores URL-only changes until rendered content changes', async () => {
  const states = [
    { url: '#/question-15', question: 'Question 14 of 15', content: 'Question 14 of 15\nold sentence' },
    { url: '#/question-15', question: 'Question 15 of 15', content: 'Question 15 of 15\nnew sentence' },
  ];
  const driver = {
    async executeScript() { return { ...(states.shift() || states.at(-1)), closing: false }; },
    async sleep() {},
  };

  const result = await waitForActivityChange(
    driver,
    { url: '#/question-14', question: 'Question 14 of 15', content: 'Question 14 of 15\nold sentence', closing: false },
    1000
  );

  assert.equal(result.question, 'Question 15 of 15');
});

// Live-verified counterpart: a "Question N of M" counter label can itself
// stay frozen on the old number even once the hash *and* the actual question
// sentence underneath have both moved on — the full #content text (which
// includes that sentence) still changes in that case, so it's still treated
// as advanced rather than stuck.
test('waitForActivityChange treats it as advanced when content changes even if the counter label is stale', async () => {
  const driver = {
    async executeScript() {
      return {
        url: '#/question-4',
        question: 'Question 3 of 10', // stale label, never updates
        content: 'Question 3 of 10\nWalk straight, then ________ at the drug store.',
        closing: false,
      };
    },
    async sleep() {},
  };

  const result = await waitForActivityChange(
    driver,
    { url: '#/question-3', question: 'Question 3 of 10', content: 'Question 3 of 10\nSome earlier sentence.', closing: false },
    1000
  );

  assert.ok(result);
  assert.equal(result.question, 'Question 3 of 10');
});

test('advance returns false when a stale visible Continue never changes rendered content', async () => {
  const driver = {
    async executeScript(code) {
      if (code.includes("document.querySelector('.quiz-next-btn')")) return true;
      return { url: '#/question-15', question: 'Question 14 of 15', content: 'Question 14 of 15\nold sentence', closing: false };
    },
    async sleep() {},
  };

  assert.equal(
    await advance(
      driver,
      { url: '#/question-14', question: 'Question 14 of 15', content: 'Question 14 of 15\nold sentence', closing: false },
      10
    ),
    false
  );
});

test('iframe runner uses shared max answer retry config', () => {
  const source = fs.readFileSync(path.join(__dirname, '../scripts/run-iframe-exercise.js'), 'utf8');

  assert.match(source, /retryLimit: skipLlm \? 0 : config\.retry\.maxAnswerRetries/);
  assert.doesNotMatch(source, /MAX_ANSWER_RETRIES\s*=\s*10/);
});

test('createBruteForceAnswerer tries non-empty candidate-letter combinations in increasing subset size', async () => {
  const answerQuestion = createBruteForceAnswerer();
  const questionData = {
    candidates: [
      { letter: 'A' },
      { letter: 'B' },
      { letter: 'C' },
    ],
  };

  await assert.doesNotReject(async () => {
    assert.deepEqual((await answerQuestion(null, null, null, questionData)).answers, ['A']);
    assert.deepEqual((await answerQuestion(null, null, null, questionData)).answers, ['B']);
    assert.deepEqual((await answerQuestion(null, null, null, questionData)).answers, ['C']);
    assert.deepEqual((await answerQuestion(null, null, null, questionData)).answers, ['A', 'B']);
    assert.deepEqual((await answerQuestion(null, null, null, questionData)).answers, ['A', 'C']);
    assert.deepEqual((await answerQuestion(null, null, null, questionData)).answers, ['B', 'C']);
    assert.deepEqual((await answerQuestion(null, null, null, questionData)).answers, ['A', 'B', 'C']);
  });
});

test('createBruteForceAnswerer exhausts only after all candidate-letter combinations are tried', async () => {
  const answerQuestion = createBruteForceAnswerer();
  const questionData = {
    candidates: [
      { letter: 'A' },
      { letter: 'B' },
    ],
  };

  assert.deepEqual((await answerQuestion(null, null, null, questionData)).answers, ['A']);
  assert.deepEqual((await answerQuestion(null, null, null, questionData)).answers, ['B']);
  assert.deepEqual((await answerQuestion(null, null, null, questionData)).answers, ['A', 'B']);
  await assert.rejects(
    answerQuestion(null, null, null, questionData),
    /exhausted all candidate-letter combinations/
  );
});
