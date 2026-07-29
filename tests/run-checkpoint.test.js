const assert = require('node:assert/strict');
const { test } = require('node:test');
const { run: runCheckpoint, waitForScore, ensurePillsExpanded, waitForPillSaved, answerAllQuestions } = require('../scripts/run-checkpoint');

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
    async executeScript(code) {
      if (code.includes("some((b) => !b.disabled)")) return true;
      if (code.includes('primary-light-shade-color')) return [];
      return false;
    },
  };

  const result = await answerAllQuestions(driver, new Map(), {
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
    { pill: 1, letter: 'B' },
    { pill: 3, letter: 'E' },
  ]);
  assert.deepEqual(result, { status: 'answered', questionNum: 2 });
});

test('answerAllQuestions retries a pill click once before treating it as a stall', async () => {
  const clickAttempts = [];
  let activePill = null;
  let pill2ActiveChecks = 0;
  const driver = {
    switchTo() { return { defaultContent: async () => {} }; },
    async executeScript(code) {
      if (code.includes('primary-light-shade-color')) return [];
      return true; // "enabled" check
    },
  };

  const result = await answerAllQuestions(driver, new Map(), {
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
    async executeScript(code) {
      if (code.includes("some((b) => !b.disabled)")) return activePill !== 2;
      if (code.includes('primary-light-shade-color')) return [];
      if (code.includes("t === 'Save'")) { savedPills.push(activePill); return true; }
      return false;
    },
  };

  const result = await answerAllQuestions(driver, new Map(), {
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
    { pill: 1, letter: 'B' },
    { pill: 3, letter: 'E' },
  ]);
  assert.deepEqual(savedPills, [1, 3]);
  assert.deepEqual(visits, [1, 2, 3]);
  assert.deepEqual(submissions, [[1, 2, 3]]);
  assert.deepEqual(result, { status: 'answered', questionNum: 2 });
});

test('answerAllQuestions submits whatever is answered when unanswered pills make no progress across passes', async () => {
  const visits = [];
  const submissions = [];
  let activePill = null;
  let unansweredChecks = 0;
  const infos = [];
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

  const result = await answerAllQuestions(driver, new Map(), {
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

// The actual bug this was built to fix: a shared letter for every pill each
// attempt can never solve pills whose correctness depends on not colliding
// with a cluster-mate (verified live: seven relative-pronoun blanks sharing
// one candidate pool plateaued at 60%/12-unresolved for 27 straight
// attempts under the old single-letter design). Each pill must track and
// advance its own letter independently, persisted in `pillLetters` across
// separate answerAllQuestions calls (separate checkpoint attempts) — this
// proves that: pill 1 locks in after attempt 1 (frozen, never re-clicked),
// while pill 3 — untouched by pill 1's progress — keeps advancing on its
// own from its own start letter on attempt 2, instead of both pills being
// forced onto whatever single letter that attempt happens to use.
test('answerAllQuestions advances each pill\'s letter independently across attempts via a shared pillLetters map', async () => {
  const pillLetters = new Map();
  let pill1Locked = false;
  const lettersUsed = [];

  function driverFor(pass) {
    return {
      switchTo() { return { defaultContent: async () => {} }; },
      async executeScript(code) {
        if (code.includes("some((b) => !b.disabled)")) return true;
        if (code.includes('primary-light-shade-color')) return [];
        return false;
      },
    };
  }

  const baseOptions = () => ({
    getCurrentQuestionDomFn: async () => ({ kind: 'fake-dom' }),
    ensurePillsExpandedFn: async () => true,
    readPillsFn: async () => [{ n: 1 }, { n: 3 }],
    isPillLockedFn: async (_driver, n) => n === 1 && pill1Locked,
    clickPillFn: async () => true,
    waitForPillActiveFn: async () => true,
    answerEveryGroupFn: async (_driver, letter) => {
      lettersUsed.push(letter);
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

  await answerAllQuestions(driverFor(1), pillLetters, baseOptions());
  // Unseen pills start at a hash of the pill number, not always 'A' — see
  // the staggered-start test below for why. Pill 1 hashes to index 1 ('B'),
  // pill 3 hashes to index 4 ('E').
  assert.deepEqual(lettersUsed, ['B', 'E']);
  assert.deepEqual([...pillLetters.entries()], [[1, 2], [3, 5]]); // both advanced one step past their start

  pill1Locked = true; // pretend pill 1's "B" attempt was graded correct
  lettersUsed.length = 0;

  await answerAllQuestions(driverFor(2), pillLetters, baseOptions());
  assert.deepEqual(lettersUsed, ['F']); // only pill 3 touched — pill 1 is locked, skipped entirely
  assert.deepEqual([...pillLetters.entries()], [[1, 2], [3, 6]]); // pill 1 frozen, pill 3 kept advancing
});

// The bug this specifically fixes: a real checkpoint had pills stuck sharing
// the same small option set, ALL wrong every single attempt (never locking
// in to break sync) — since they all started at index 0 and always failed
// together, they stayed in perfect lockstep, always trying the identical
// letter as each other, forever (verified live: score frozen dead flat for
// 49+ attempts, 7 pills, same shared letter every round). A first fix
// (starting each unseen pill's index at pillNumber % 6) helped but wasn't
// enough: pills 14 and 20 are exactly 6 apart, so `n % 6` gave them the
// identical start too — verified live, they then stayed locked together for
// 17 more attempts flat at 93%. Any LINEAR function of n mod 6 collides for
// every pair spaced by a multiple of 6, no choice of coefficients avoids it.
// A real (non-linear) integer hash does — this checks that specific
// production pair (14 and 20) no longer collide, not just any two pills.
test('answerAllQuestions staggers unseen pills\' starting letters with a non-linear hash, so pills spaced 6 apart no longer collide', async () => {
  const lettersByPill = {};
  let activePill = null;
  const driver = {
    switchTo() { return { defaultContent: async () => {} }; },
    async executeScript(code) {
      if (code.includes("some((b) => !b.disabled)")) return true;
      if (code.includes('primary-light-shade-color')) return [];
      return false;
    },
  };

  await answerAllQuestions(driver, new Map(), {
    getCurrentQuestionDomFn: async () => ({ kind: 'fake-dom' }),
    ensurePillsExpandedFn: async () => true,
    readPillsFn: async () => [{ n: 14 }, { n: 20 }],
    isPillLockedFn: async () => false,
    clickPillFn: async (_driver, n) => {
      activePill = n;
      return true;
    },
    waitForPillActiveFn: async () => true,
    answerEveryGroupFn: async (_driver, letter) => {
      lettersByPill[activePill] = letter;
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

  assert.notEqual(lettersByPill[14], lettersByPill[20]);
});
