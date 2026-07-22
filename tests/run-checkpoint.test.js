const assert = require('node:assert/strict');
const { test } = require('node:test');
const { run: runCheckpoint, waitForScore } = require('../scripts/run-checkpoint');

function makeDriver({ gateClickable = true } = {}) {
  const urls = ['https://lms.binus.ac.id/checkpoint-gate'];
  return {
    switchTo() { return { defaultContent: async () => {} }; },
    async getCurrentUrl() { return urls[urls.length - 1]; },
    async get(url) { urls.push(url); },
    async sleep() {},
    async executeScript(code) {
      if (code.includes('Start Attempt') && !gateClickable) return false;
      if (code.includes('Start Attempt')) return true;
      return false;
    },
  };
}

test('passes on the first attempt when the score is 100', async () => {
  const driver = makeDriver();
  const runExerciseFn = async () => ({ status: 'complete', questionNum: 30, score: 100 });
  const result = await runCheckpoint(driver, { runExerciseFn });
  assert.deepEqual(result, { status: 'passed', attempt: 1, score: 100 });
});

test('retries from the gate when the score is below 100, then passes', async () => {
  const driver = makeDriver();
  let call = 0;
  const runExerciseFn = async () => {
    call += 1;
    return call === 1 ? { status: 'complete', questionNum: 30, score: 83 } : { status: 'complete', questionNum: 30, score: 100 };
  };
  const result = await runCheckpoint(driver, { runExerciseFn });
  assert.deepEqual(result, { status: 'passed', attempt: 2, score: 100 });
});

test('stops immediately on an unhandled question type, without retrying', async () => {
  const driver = makeDriver();
  let calls = 0;
  const runExerciseFn = async () => {
    calls += 1;
    return { status: 'unhandled', questionNum: 7 };
  };
  const result = await runCheckpoint(driver, { runExerciseFn });
  assert.deepEqual(result, { status: 'unhandled', attempt: 1, questionNum: 7 });
  assert.equal(calls, 1);
});

test('gives up after 5 attempts without reaching 100', async () => {
  const driver = makeDriver();
  let calls = 0;
  const runExerciseFn = async () => {
    calls += 1;
    return { status: 'complete', questionNum: 30, score: 90 };
  };
  const result = await runCheckpoint(driver, { runExerciseFn });
  assert.deepEqual(result, { status: 'exhausted', attempts: 5 });
  assert.equal(calls, 5);
});

// clickGateButton polls a real 8s wall-clock deadline (matches
// run-unit.js's clickStartGate — a fixed delay would be too short after a
// real route change), which outlives bun's default 5s per-test timeout;
// this test alone needs a longer explicit timeout, not a production change.
test('stops when the gate button is never found', { timeout: 10000 }, async () => {
  const driver = makeDriver({ gateClickable: false });
  const runExerciseFn = async () => { throw new Error('should not be called'); };
  const result = await runCheckpoint(driver, { runExerciseFn });
  assert.deepEqual(result, { status: 'no-gate', attempt: 1 });
});

// The post-submit result page's body text reads "...Your Score:83Correct25
// Incorrect5No Answer0..." — verified live on a real checkpoint that missed
// 5 of 30 questions.
test('waitForScore parses the score out of the post-submit result page', async () => {
  const driver = {
    switchTo() { return { defaultContent: async () => {} }; },
    async sleep() {},
    async executeScript() {
      return 77;
    },
  };
  const score = await waitForScore(driver);
  assert.equal(score, 77);
});

test('waitForScore resolves undefined if the score never appears within the timeout', async () => {
  const driver = {
    switchTo() { return { defaultContent: async () => {} }; },
    async sleep() {},
    async executeScript() {
      return null;
    },
  };
  const score = await waitForScore(driver, 50);
  assert.equal(score, undefined);
});
