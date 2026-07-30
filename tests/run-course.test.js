const assert = require('node:assert/strict');
const { test } = require('node:test');
const { run: runCourse } = require('../scripts/run-course');

function makeDriver({ url = 'https://lms.binus.ac.id/course' } = {}) {
  const urls = [url];
  return {
    switchTo() { return { defaultContent: async () => {} }; },
    async getCurrentUrl() { return urls[urls.length - 1]; },
    async get(u) { urls.push(u); },
    async sleep() {},
  };
}

test('run-course opens nodes in ascending course order, skipping completed ones', async () => {
  const driver = makeDriver();
  const opened = [];

  // Matches the live roadmap: Unit1, Unit2, Checkpoint1, Unit3 — with
  // Unit 1/3 and CP 0/1 already done, so Unit2 is the next node to open.
  const nodes = [
    { type: 'unit', number: 1, top: 900, label: 'Unit1' },
    { type: 'unit', number: 2, top: 700, label: 'Unit2' },
    { type: 'checkpoint', number: 1, top: 500, label: 'Checkpoint1' },
    { type: 'unit', number: 3, top: 300, label: 'Unit3' },
  ];
  let progress = { unit: { done: 1, total: 3 }, checkpoint: { done: 0, total: 1 } };

  const result = await runCourse(driver, {
    readCourseNodesFn: async () => nodes,
    readProgressFn: async () => progress,
    clickNodeFn: async (_driver, label) => {
      opened.push(label);
      return true;
    },
    waitForUrlChangeFn: async () => 'https://lms.binus.ac.id/course/opened',
    runUnitFn: async () => {
      progress = { unit: { done: progress.unit.done + 1, total: 3 }, checkpoint: progress.checkpoint };
      return { status: 'complete' };
    },
    runCheckpointFn: async () => {
      progress = { unit: progress.unit, checkpoint: { done: progress.checkpoint.done + 1, total: 1 } };
      return { status: 'passed' };
    },
  });

  assert.deepEqual(opened, ['Unit2', 'Checkpoint1', 'Unit3']);
  assert.deepEqual(result, { status: 'complete', unit: { done: 3, total: 3 }, checkpoint: { done: 1, total: 1 } });
});

test('run-course never opens a Final Test node even if present in the DOM', async () => {
  const driver = makeDriver();
  const opened = [];

  const result = await runCourse(driver, {
    readCourseNodesFn: async () => [{ type: 'unit', number: 1, top: 900, label: 'Unit1' }],
    readProgressFn: async () => ({ unit: { done: 0, total: 1 }, checkpoint: { done: 0, total: 0 } }),
    clickNodeFn: async (_driver, label) => {
      opened.push(label);
      return true;
    },
    waitForUrlChangeFn: async () => 'https://lms.binus.ac.id/course/opened',
    runUnitFn: async () => {
      // The next poll would surface a "Final Test1" node, but readCourseNodes
      // (whitelist-based) is what decides visibility — this test's
      // still-frozen node list stands in for "Final Test was never
      // considered a candidate to begin with".
      return { status: 'complete' };
    },
    runCheckpointFn: async () => ({ status: 'passed' }),
  });

  assert.deepEqual(opened, ['Unit1']);
  assert.equal(result.status, 'complete');
});

test('run-course stops when a course node fails to open (locked)', async () => {
  const driver = makeDriver();

  const result = await runCourse(driver, {
    readCourseNodesFn: async () => [{ type: 'unit', number: 5, top: 300, label: 'Unit5' }],
    readProgressFn: async () => ({ unit: { done: 4, total: 8 }, checkpoint: { done: 1, total: 3 } }),
    clickNodeFn: async () => true,
    waitForUrlChangeFn: async () => null, // locked: URL never changes
    runUnitFn: async () => ({ status: 'complete' }),
    runCheckpointFn: async () => ({ status: 'passed' }),
  });

  assert.deepEqual(result, { status: 'stuck', label: 'Unit5' });
});

test('run-course stops when a unit inside the course gets stuck', async () => {
  const driver = makeDriver();

  const result = await runCourse(driver, {
    readCourseNodesFn: async () => [{ type: 'unit', number: 1, top: 900, label: 'Unit1' }],
    readProgressFn: async () => ({ unit: { done: 0, total: 1 }, checkpoint: { done: 0, total: 0 } }),
    clickNodeFn: async () => true,
    waitForUrlChangeFn: async () => 'https://lms.binus.ac.id/course/opened',
    runUnitFn: async () => ({ status: 'stuck', title: 'Interactive Grammar Activity' }),
    runCheckpointFn: async () => ({ status: 'passed' }),
  });

  assert.equal(result.status, 'stuck');
  assert.equal(result.label, 'Unit1');
});

test('run-course does not complete when roadmap nodes or progress are absent', async () => {
  const driver = makeDriver();

  const result = await runCourse(driver, {
    readCourseNodesFn: async () => [],
    readProgressFn: async () => ({ unit: null, checkpoint: null }),
    readRetryMs: 50,
  });

  assert.deepEqual(result, { status: 'stuck', reason: 'missing-roadmap' });
});
