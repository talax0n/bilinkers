const assert = require('node:assert/strict');
const { test } = require('node:test');
const { run: runCheckpoint, waitForScore, ensurePillsExpanded, waitForPillSaved, answerAllQuestions, clickSubmit } = require('../scripts/run-checkpoint');

function makeDriver({ gateClickable = true } = {}) {
  const urls = ['https://lms.binus.ac.id/checkpoint-gate'];
  return {
    switchTo() { return { defaultContent: async () => {} }; },
    async sleep() {},
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

test('waitForPillSaved polls until the pill drops its unanswered marker', async () => {
  let calls = 0;
  const driver = {
    async sleep() {},
    async executeScript() {
      calls += 1;
      return calls >= 3; // unanswered (false) for the first two polls, then saved
    },
  };

  assert.equal(await waitForPillSaved(driver, 5), true);
  assert.equal(calls, 3);
});

test('waitForPillSaved resolves false if the pill never clears within the timeout', async () => {
  const driver = {
    async sleep() {},
    async executeScript() { return false; },
  };

  assert.equal(await waitForPillSaved(driver, 5, 250), false);
});

test('answerAllQuestions skips a locked pill without clicking it or waiting for it to go active', async () => {
  const visits = [];
  const waitedFor = [];
  const answeredPills = [];
  let activePill = null;
  const driver = {
    switchTo() { return { defaultContent: async () => {} }; },
    async sleep() {},
    async executeScript(code) {
      if (code.includes("filter((b) => !b.disabled).length")) return 6;
      if (code.includes('primary-light-shade-color')) return [];
      return false;
    },
  };

  const result = await answerAllQuestions(driver, 'A', {
    getCurrentQuestionDomFn: async () => ({ kind: 'fake-dom' }),
    ensurePillsExpandedFn: async () => true,
    readPillsFn: async () => [{ n: 1 }, { n: 2 }, { n: 3 }],
    isPillLockedFn: async (_driver, n) => n === 2, // pill 2 is already correct from a prior attempt
    clickPillFn: async (_driver, n) => {
      visits.push(n);
      activePill = n;
      return true;
    },
    waitForPillActiveFn: async (_driver, n) => {
      waitedFor.push(n);
      return activePill === n;
    },
    answerEveryGroupFn: async (_driver, letter) => {
      answeredPills.push({ pill: activePill, letter });
      return 1;
    },
    clickSaveButtonFn: async () => true,
    waitForPillSavedFn: async () => true,
    clickSubmitFn: async () => true,
    createRegistryFn: () => ({
      register() {},
      findHandler() { return { name: 'fake-handler' }; },
    }),
    logger: { info() {}, warn() {} },
  });

  assert.deepEqual(visits, [1, 3]);
  assert.deepEqual(waitedFor, [1, 3]);
  assert.deepEqual(answeredPills, [
    { pill: 1, letter: 'A' },
    { pill: 3, letter: 'A' },
  ]);
  assert.deepEqual(result, { status: 'answered', questionNum: 2 });
});

test('answerAllQuestions retries a pill click once before treating it as a stall', async () => {
  const clickAttempts = [];
  let activePill = null;
  let pill2ActiveChecks = 0;
  const driver = {
    switchTo() { return { defaultContent: async () => {} }; },
    async sleep() {},
    async executeScript(code) {
      if (code.includes('primary-light-shade-color')) return [];
      return true; // "enabled" check
    },
  };

  const result = await answerAllQuestions(driver, 'A', {
    getCurrentQuestionDomFn: async () => ({ kind: 'fake-dom' }),
    ensurePillsExpandedFn: async () => true,
    readPillsFn: async () => [{ n: 1 }, { n: 2 }],
    isPillLockedFn: async () => false,
    clickPillFn: async (_driver, n) => {
      clickAttempts.push(n);
      activePill = n;
      return true;
    },
    waitForPillActiveFn: async (_driver, n) => {
      if (n === 2) {
        pill2ActiveChecks += 1;
        if (pill2ActiveChecks === 1) return false; // first click's wait times out
      }
      return activePill === n;
    },
    answerEveryGroupFn: async () => 1,
    clickSaveButtonFn: async () => true,
    waitForPillSavedFn: async () => true,
    clickSubmitFn: async () => true,
    createRegistryFn: () => ({
      register() {},
      findHandler() { return { name: 'fake-handler' }; },
    }),
    logger: { info() {}, warn() {} },
  });

  assert.deepEqual(clickAttempts, [1, 2, 2]); // pill 2 clicked twice: first miss, then the retry
  assert.deepEqual(result, { status: 'answered', questionNum: 2 });
});

test('answerAllQuestions navigates by pill, saves each answer, and submits without leaving the final pill', async () => {
  const visits = [];
  const submissions = [];
  const answeredPills = [];
  const savedPills = [];
  let activePill = null;
  const driver = {
    switchTo() { return { defaultContent: async () => {} }; },
    async sleep() {},
    async executeScript(code) {
      if (code.includes("filter((b) => !b.disabled).length")) return activePill !== 2 ? 6 : 0;
      if (code.includes('primary-light-shade-color')) return [];
      if (code.includes("t === 'Save'")) { savedPills.push(activePill); return true; }
      return false;
    },
  };

  const result = await answerAllQuestions(driver, 'A', {
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
    waitForPillSavedFn: async () => true,
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
    { pill: 1, letter: 'A' },
    { pill: 3, letter: 'A' },
  ]);
  assert.deepEqual(savedPills, [1, 3]);
  assert.deepEqual(visits, [1, 2, 3]);
  assert.deepEqual(submissions, [[1, 2, 3]]);
  assert.deepEqual(result, { status: 'answered', questionNum: 2 });
});

test('answerAllQuestions uses the same shared letter for every pill in the attempt', async () => {
  const lettersUsed = [];
  let activePill = null;
  const driver = {
    switchTo() { return { defaultContent: async () => {} }; },
    async sleep() {},
    async executeScript(code) {
      if (code.includes("filter((b) => !b.disabled).length")) return 6;
      if (code.includes('primary-light-shade-color')) return [];
      return false;
    },
  };

  await answerAllQuestions(driver, 'C', {
    getCurrentQuestionDomFn: async () => ({ kind: 'fake-dom' }),
    ensurePillsExpandedFn: async () => true,
    readPillsFn: async () => [{ n: 1 }, { n: 2 }, { n: 3 }, { n: 4 }, { n: 5 }, { n: 6 }],
    clickPillFn: async (_driver, n) => { activePill = n; return true; },
    waitForPillActiveFn: async (_driver, n) => activePill === n,
    answerEveryGroupFn: async (_driver, letter) => {
      lettersUsed.push({ pill: activePill, letter });
      return 1;
    },
    waitForPillSavedFn: async () => true,
    clickSubmitFn: async () => true,
    createRegistryFn: () => ({
      register() {},
      findHandler() { return { name: 'fake-handler' }; },
    }),
    logger: { info() {}, warn() {} },
  });

  assert.equal(lettersUsed.length, 6);
  assert.ok(lettersUsed.every(({ letter }) => letter === 'C'));
});

test('answerAllQuestions applies the next shared letter on a fresh attempt, skipping pills already locked', async () => {
  const lettersUsed = [];
  let activePill = null;
  let pill1Locked = false;
  const driver = {
    switchTo() { return { defaultContent: async () => {} }; },
    async sleep() {},
    async executeScript(code) {
      if (code.includes("filter((b) => !b.disabled).length")) return 6;
      if (code.includes('primary-light-shade-color')) return [];
      return false;
    },
  };

  const baseOptions = () => ({
    getCurrentQuestionDomFn: async () => ({ kind: 'fake-dom' }),
    ensurePillsExpandedFn: async () => true,
    readPillsFn: async () => [{ n: 1 }, { n: 3 }],
    isPillLockedFn: async (_driver, n) => n === 1 && pill1Locked,
    clickPillFn: async (_driver, n) => { activePill = n; return true; },
    waitForPillActiveFn: async () => true,
    answerEveryGroupFn: async (_driver, letter) => { lettersUsed.push(letter); return 1; },
    clickSaveButtonFn: async () => true,
    waitForPillSavedFn: async () => true,
    clickSubmitFn: async () => true,
    createRegistryFn: () => ({
      register() {},
      findHandler() { return { name: 'fake-handler' }; },
    }),
    logger: { info() {}, warn() {} },
  });

  await answerAllQuestions(driver, 'A', baseOptions());
  assert.deepEqual(lettersUsed, ['A', 'A']);

  pill1Locked = true; // pill 1's 'A' was graded correct — frozen
  lettersUsed.length = 0;
  await answerAllQuestions(driver, 'B', baseOptions());
  assert.deepEqual(lettersUsed, ['B']); // only pill 3 gets the new letter
});

test('answerAllQuestions submits whatever is answered when unanswered pills make no progress across passes', async () => {
  const visits = [];
  const submissions = [];
  let activePill = null;
  let unansweredChecks = 0;
  const infos = [];
  const driver = {
    switchTo() { return { defaultContent: async () => {} }; },
    async sleep() {},
    async executeScript(code) {
      if (code.includes("filter((b) => !b.disabled).length")) return 6;
      if (code.includes('primary-light-shade-color')) {
        unansweredChecks += 1;
        return unansweredChecks === 1 ? [2, 3] : [2, 3];
      }
      return false;
    },
  };

  const result = await answerAllQuestions(driver, 'A', {
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
    waitForPillSavedFn: async () => true,
    clickSubmitFn: async () => {
      submissions.push('submit');
      return true;
    },
    createRegistryFn: () => ({
      register() {},
      findHandler() { return { name: 'fake-handler' }; },
    }),
    logger: {
      info(message, data) { infos.push({ message, data }); },
      warn() {},
    },
    maxCompletionPasses: 3,
  });

  // Not 'unhandled' — a letter that stops resolving new pills is expected
  // brute-force behaviour, not a structural gap, so the attempt still
  // submits and the outer retry loop gets a real score to react to.
  assert.deepEqual(result, { status: 'answered', questionNum: 6 });
  assert.deepEqual(submissions, ['submit']);
  assert.equal(unansweredChecks, 2);
  assert.deepEqual(visits, [1, 2, 3, 1, 2, 3]);
  assert.equal(infos.some(({ message, data }) => message.includes('made no progress') && Array.isArray(data.unanswered)), true);
});

// The checkpoint brute-forces ACROSS attempts with a shared letter, so a
// question's correctness only becomes knowable next attempt via isPillLockedFn
// (correct pills lock and are skipped). This verifies the shared-letter model:
// every unlocked pill gets the same letter, locked pills are skipped, and the
// same letter is reused across completion passes within one attempt (those
// passes retry saves, they don't advance letters).
test('answerAllQuestions reuses the same letter across completion passes within one attempt', async () => {
  const lettersUsed = [];
  let activePill = null;
  let unanswered = [2];
  const driver = {
    switchTo() { return { defaultContent: async () => {} }; },
    async sleep() {},
    async executeScript(code) {
      if (code.includes("filter((b) => !b.disabled).length")) return 6;
      if (code.includes('primary-light-shade-color')) return unanswered;
      return false;
    },
  };

  await answerAllQuestions(driver, 'E', {
    getCurrentQuestionDomFn: async () => ({ kind: 'fake-dom' }),
    ensurePillsExpandedFn: async () => true,
    readPillsFn: async () => [{ n: 1 }, { n: 2 }],
    isPillLockedFn: async () => false,
    clickPillFn: async (_driver, n) => { activePill = n; return true; },
    waitForPillActiveFn: async () => true,
    answerEveryGroupFn: async (_driver, letter) => { lettersUsed.push(letter); return 1; },
    clickSaveButtonFn: async () => true,
    waitForPillSavedFn: async () => true,
    clickSubmitFn: async () => true,
    createRegistryFn: () => ({
      register() {},
      findHandler() { return { name: 'fake-handler' }; },
    }),
    logger: { info() {}, warn() {} },
    maxCompletionPasses: 2,
  });

  assert.ok(lettersUsed.length > 0);
  assert.ok(lettersUsed.every((letter) => letter === 'E'));
});

test('main advances the shared letter on each retry until the checkpoint passes', async () => {
  const driver = makeDriver();
  const letters = [];
  let call = 0;
  const runExerciseFn = async (_driver, letter) => {
    letters.push(letter);
    call += 1;
    return call === 1 ? { status: 'complete', questionNum: 30, score: 80 } : { status: 'complete', questionNum: 30, score: 100 };
  };
  const result = await runCheckpoint(driver, { runExerciseFn });
  assert.deepEqual(result, { status: 'passed', attempt: 2, score: 100 });
  assert.deepEqual(letters, ['A', 'B']); // attempt 1 all A, attempt 2 all B
});

test('clickSubmit retries the Submit click when the first one never opens the confirm', async () => {
  let submitClicks = 0;
  let confirmVisible = false;
  const driver = {
    switchTo() { return { defaultContent: async () => {} }; },
    async sleep(ms) {
      if (ms === 300) confirmVisible = true; // the backoff before a retry submit is where the page finally catches up
    },
    async executeScript(code) {
      if (code.includes("=== 'Submit'")) {
        submitClicks += 1;
        return true;
      }
      if (code.includes("=== 'Yes'")) return confirmVisible;
      return false;
    },
  };
  assert.equal(await clickSubmit(driver, { maxSubmitClicks: 3, confirmWaitMs: 200 }), true);
  assert.equal(submitClicks, 2);
});

test('clickSubmit returns false when the Submit button is never rendered', async () => {
  const driver = {
    switchTo() { return { defaultContent: async () => {} }; },
    async sleep() {},
    async executeScript() { return null; }, // no button in the DOM at all
  };
  assert.equal(await clickSubmit(driver, { maxSubmitClicks: 3, enabledWaitMs: 200, confirmWaitMs: 200 }), false);
});

test('clickSubmit waits for a transiently-disabled Submit button to enable, then submits', async () => {
  let disabled = true;
  let submitClicks = 0;
  let confirmVisible = false;
  const driver = {
    switchTo() { return { defaultContent: async () => {} }; },
    async sleep(ms) {
      if (ms === 200 && disabled) disabled = false; // save settles, button enables
    },
    async executeScript(code) {
      if (code.includes("=== 'Submit'")) {
        if (disabled) return false; // found but disabled
        submitClicks += 1;
        confirmVisible = true; // a landed Submit click opens the confirm right away
        return true;
      }
      if (code.includes("=== 'Yes'")) return confirmVisible;
      return false;
    },
  };
  assert.equal(await clickSubmit(driver, { maxSubmitClicks: 3, enabledWaitMs: 1000, confirmWaitMs: 200 }), true);
  assert.equal(submitClicks, 1);
});

test('clickSubmit returns false when the Submit button stays disabled the whole window', async () => {
  const warns = [];
  const driver = {
    switchTo() { return { defaultContent: async () => {} }; },
    async sleep() {},
    async executeScript(code) {
      if (code.includes("=== 'Submit'")) return false; // found but permanently disabled
      return false;
    },
  };
  assert.equal(await clickSubmit(driver, { maxSubmitClicks: 3, enabledWaitMs: 300, confirmWaitMs: 100, logger: { info() {}, warn(m) { warns.push(m); } } }), false);
  assert.equal(warns.some((m) => m.includes('stayed disabled')), true);
});

test('clickSubmit returns false when the confirm never appears across all submit retries', async () => {
  let submitClicks = 0;
  const driver = {
    switchTo() { return { defaultContent: async () => {} }; },
    async sleep() {},
    async executeScript(code) {
      if (code.includes("=== 'Submit'")) { submitClicks += 1; return true; }
      return false;
    },
  };
  assert.equal(await clickSubmit(driver, { maxSubmitClicks: 2, confirmWaitMs: 100 }), false);
  assert.equal(submitClicks, 2);
});

test('answerAllQuestions reports submit-failed when the submit click never lands', async () => {
  const infos = [];
  const driver = {
    switchTo() { return { defaultContent: async () => {} }; },
    async sleep() {},
    async executeScript(code) {
      if (code.includes('primary-light-shade-color')) return []; // nothing left unanswered -> single pass
      if (code.includes("filter((b) => !b.disabled).length")) return 1;
      return false;
    },
  };

  const result = await answerAllQuestions(driver, 'A', {
    getCurrentQuestionDomFn: async () => ({ kind: 'fake-dom' }),
    ensurePillsExpandedFn: async () => true,
    readPillsFn: async () => [{ n: 1 }],
    isPillLockedFn: async () => false,
    clickPillFn: async () => true,
    waitForPillActiveFn: async () => true,
    answerEveryGroupFn: async () => 1,
    clickSaveButtonFn: async () => true,
    waitForPillSavedFn: async () => true,
    clickSubmitFn: async () => false,
    createRegistryFn: () => ({
      register() {},
      findHandler() { return { name: 'fake-handler' }; },
    }),
    logger: { info() {}, warn(message, data) { infos.push({ message, data }); } },
  });

  assert.deepEqual(result, { status: 'submit-failed', questionNum: 1 });
  assert.equal(infos.some(({ message }) => message.includes('submit did not go through')), true);
});

test('runCheckpoint retries a submit-failed attempt, then passes once submit lands', async () => {
  const driver = makeDriver();
  let call = 0;
  const runExerciseFn = async () => {
    call += 1;
    if (call === 1) return { status: 'submit-failed', questionNum: 30 };
    return { status: 'complete', questionNum: 30, score: 100 };
  };
  const result = await runCheckpoint(driver, { runExerciseFn });
  assert.deepEqual(result, { status: 'passed', attempt: 2, score: 100 });
});

test('runCheckpoint stops after 3 consecutive submit failures', async () => {
  const driver = makeDriver();
  const runExerciseFn = async () => ({ status: 'submit-failed', questionNum: 30 });
  const result = await runCheckpoint(driver, { runExerciseFn });
  assert.deepEqual(result, { status: 'submit-failed', attempt: 3 });
});

test('readPills and clickPill are exported for reuse by other scripts', () => {
  const mod = require('../scripts/run-checkpoint');
  assert.equal(typeof mod.readPills, 'function');
  assert.equal(typeof mod.clickPill, 'function');
});
