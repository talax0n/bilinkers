const assert = require('node:assert/strict');
const { test } = require('node:test');
const { run: runCheckpoint, waitForScore, ensurePillsExpanded, answerAllQuestions } = require('../scripts/run-checkpoint');

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

test('keeps retrying past the old 5-attempt cap until it passes', async () => {
  const driver = makeDriver();
  let calls = 0;
  const runExerciseFn = async () => {
    calls += 1;
    return calls < 8 ? { status: 'complete', questionNum: 30, score: 90 } : { status: 'complete', questionNum: 30, score: 100 };
  };
  const result = await runCheckpoint(driver, { runExerciseFn });
  assert.deepEqual(result, { status: 'passed', attempt: 8, score: 100 });
  assert.equal(calls, 8);
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

test('ensurePillsExpanded clicks the checkpoint chevron and waits for more pills', async () => {
  let pillCount = 15;
  let clicks = 0;
  const driver = {
    switchTo() { return { defaultContent: async () => {} }; },
    async sleep() {},
    async executeScript(code) {
      if (code.includes("path.startsWith('M16.59 8.59L12 13.17')")) {
        clicks += 1;
        pillCount = 30;
        return true;
      }
      return Array.from({ length: pillCount }, (_, index) => ({ n: index + 1 }));
    },
  };

  assert.equal(await ensurePillsExpanded(driver), true);
  assert.equal(clicks, 1);
});

test('answerAllQuestions uses only pill navigation and grades the final question by leaving its pill before submit', async () => {
  const visits = [];
  const submissions = [];
  const answeredPills = [];
  let activePill = null;
  const driver = {
    switchTo() { return { defaultContent: async () => {} }; },
    async executeScript(code) {
      if (code.includes("some((b) => !b.disabled)")) return activePill !== 2;
      if (code.includes('primary-light-shade-color')) return [];
      if (code.includes('Save & Next')) throw new Error('save button must never be queried');
      if (code.includes("textContent.trim() === 'Save'")) throw new Error('save button must never be queried');
      return false;
    },
  };

  const result = await answerAllQuestions(driver, 'D', {
    getCurrentQuestionDomFn: async () => ({ kind: 'fake-dom' }),
    ensurePillsExpandedFn: async () => true,
    readPillsFn: async () => [{ n: 1 }, { n: 2 }, { n: 3 }],
    clickPillFn: async (_driver, n) => {
      visits.push(n);
      activePill = n;
      return true;
    },
    waitForPillActiveFn: async (_driver, n) => activePill === n,
    answerEveryGroupFn: async (_driver, letter) => {
      answeredPills.push({ pill: activePill, letter });
      return 1;
    },
    clickSubmitFn: async () => {
      submissions.push([...visits]);
      return true;
    },
    createRegistryFn: () => ({
      register() {},
      findHandler() { return { name: 'fake-handler' }; },
    }),
    logger: { info() {}, warn() {} },
  });

  assert.deepEqual(answeredPills, [
    { pill: 1, letter: 'D' },
    { pill: 3, letter: 'D' },
  ]);
  assert.deepEqual(visits, [1, 2, 3, 1]);
  assert.deepEqual(submissions, [[1, 2, 3, 1]]);
  assert.deepEqual(result, { status: 'answered', questionNum: 2 });
});

test('answerAllQuestions stops after bounded unanswered completion passes without submitting', async () => {
  const visits = [];
  const submissions = [];
  let activePill = null;
  let unansweredChecks = 0;
  const warnings = [];
  const driver = {
    switchTo() { return { defaultContent: async () => {} }; },
    async executeScript(code) {
      if (code.includes("some((b) => !b.disabled)")) return true;
      if (code.includes('primary-light-shade-color')) {
        unansweredChecks += 1;
        return unansweredChecks === 1 ? [2, 3] : [2, 3];
      }
      return false;
    },
  };

  const result = await answerAllQuestions(driver, 'C', {
    getCurrentQuestionDomFn: async () => ({ kind: 'fake-dom' }),
    ensurePillsExpandedFn: async () => true,
    readPillsFn: async () => [{ n: 1 }, { n: 2 }, { n: 3 }],
    clickPillFn: async (_driver, n) => {
      visits.push(n);
      activePill = n;
      return true;
    },
    waitForPillActiveFn: async (_driver, n) => activePill === n,
    answerEveryGroupFn: async () => 1,
    clickSubmitFn: async () => {
      submissions.push('submit');
      return true;
    },
    createRegistryFn: () => ({
      register() {},
      findHandler() { return { name: 'fake-handler' }; },
    }),
    logger: {
      info() {},
      warn(message, data) { warnings.push({ message, data }); },
    },
    maxCompletionPasses: 3,
  });

  assert.deepEqual(result, { status: 'unhandled', questionNum: 6 });
  assert.deepEqual(submissions, []);
  assert.equal(unansweredChecks, 2);
  assert.deepEqual(visits, [1, 2, 3, 1, 2, 3]);
  assert.equal(warnings.some(({ message, data }) => message.includes('made no progress') && Array.isArray(data.unanswered)), true);
});
