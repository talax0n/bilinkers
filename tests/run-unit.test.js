const assert = require('node:assert/strict');
const { test } = require('node:test');
const { run: runUnit, completeMediaActivity } = require('../scripts/run-unit');

test('waits until video playback actually ends', async () => {
  let statusChecks = 0;
  const driver = {
    switchTo() { return { defaultContent: async () => {} }; },
    async findElements() { return []; },
    async sleep() {},
    async executeScript(code) {
      if (code.includes('const vids') && code.includes('v.play()') && !code.includes('v.ended')) return 1;
      if (code.includes('v.ended')) {
        statusChecks += 1;
        return statusChecks >= 3;
      }
      return 0;
    },
  };

  assert.equal(await completeMediaActivity(driver), 'video');
  assert.equal(statusChecks, 3);
});

test('run-unit does not complete when zero rows are found', async () => {
  const driver = {
    switchTo() { return { defaultContent: async () => {} }; },
    async getCurrentUrl() { return 'https://lms.binus.ac.id/unit'; },
    async executeScript() { return []; },
  };

  assert.deepEqual(await runUnit(driver), { status: 'stuck', reason: 'no-rows' });
});
