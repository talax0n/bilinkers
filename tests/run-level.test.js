const assert = require('node:assert/strict');
const { test } = require('node:test');
const { run: runLevel } = require('../scripts/run-level');

function makeDriver({ url = 'https://lms.binus.ac.id/level' } = {}) {
  const urls = [url];
  return {
    switchTo() { return { defaultContent: async () => {} }; },
    async getCurrentUrl() { return urls[urls.length - 1]; },
    async get(u) { urls.push(u); },
    async sleep() {},
  };
}

test('run-level opens course nodes in ascending island order', async () => {
  const driver = makeDriver();
  const opened = [];

  // Matches the island screenshot's road: B1.1, B1.2, B2.1, B2.2, C1.1,
  // C1.2, C2.1, C2.2.
  const nodes = [
    { type: 'course', label: 'B1.1' },
    { type: 'course', label: 'B1.2' },
    { type: 'course', label: 'B2.1' },
    { type: 'course', label: 'B2.2' },
    { type: 'course', label: 'C1.1' },
    { type: 'course', label: 'C1.2' },
    { type: 'course', label: 'C2.1' },
    { type: 'course', label: 'C2.2' },
  ];

  const result = await runLevel(driver, {
    readLevelNodesFn: async () => nodes,
    clickNodeFn: async (_driver, label) => {
      opened.push(label);
      return true;
    },
    waitForUrlChangeFn: async () => 'https://lms.binus.ac.id/level/opened',
    runCourseFn: async () => ({ status: 'complete' }),
  });

  assert.deepEqual(opened, ['B1.1', 'B1.2', 'B2.1', 'B2.2', 'C1.1', 'C1.2', 'C2.1', 'C2.2']);
  assert.deepEqual(result, { status: 'complete', completed: opened });
});

test('run-level stops when a course node fails to open (locked)', async () => {
  const driver = makeDriver();

  const result = await runLevel(driver, {
    readLevelNodesFn: async () => [{ type: 'course', label: 'C1.1' }],
    clickNodeFn: async () => true,
    waitForUrlChangeFn: async () => null, // locked: URL never changes
    runCourseFn: async () => ({ status: 'complete' }),
  });

  assert.deepEqual(result, { status: 'stuck', label: 'C1.1', completed: [] });
});

test('run-level stops when a course inside the island gets stuck', async () => {
  const driver = makeDriver();

  const result = await runLevel(driver, {
    readLevelNodesFn: async () => [{ type: 'course', label: 'B1.1' }],
    clickNodeFn: async () => true,
    waitForUrlChangeFn: async () => 'https://lms.binus.ac.id/level/opened',
    runCourseFn: async () => ({ status: 'stuck', label: 'Unit1' }),
  });

  assert.equal(result.status, 'stuck');
  assert.equal(result.label, 'B1.1');
  assert.deepEqual(result.completed, []);
});

test('run-level does not complete when the island map cannot be read', async () => {
  const driver = makeDriver();

  const result = await runLevel(driver, {
    readLevelNodesFn: async () => [],
    readRetryMs: 50,
  });

  assert.deepEqual(result, { status: 'stuck', reason: 'missing-roadmap', completed: [] });
});

test('run-level moves on to the next course once one completes', async () => {
  const driver = makeDriver();
  const opened = [];

  const nodes = [
    { type: 'course', label: 'B1.1' },
    { type: 'course', label: 'B1.2' },
  ];

  const result = await runLevel(driver, {
    readLevelNodesFn: async () => nodes,
    clickNodeFn: async (_driver, label) => {
      opened.push(label);
      return true;
    },
    waitForUrlChangeFn: async () => 'https://lms.binus.ac.id/level/opened',
    runCourseFn: async () => ({ status: 'complete' }),
  });

  assert.deepEqual(opened, ['B1.1', 'B1.2']);
  assert.deepEqual(result, { status: 'complete', completed: ['B1.1', 'B1.2'] });
});
