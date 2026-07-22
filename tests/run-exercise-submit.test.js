const assert = require('node:assert/strict');
const { test } = require('node:test');
const { submitIfPresent } = require('../scripts/run-exercise');

function makeDriver(script) {
  return {
    async executeScript(code) { return script(code); },
    async sleep() {},
  };
}

test('submitIfPresent resolves { submitted: false } when there is nothing to submit', async () => {
  const driver = makeDriver(() => false);
  const result = await submitIfPresent(driver);
  assert.deepEqual(result, { submitted: false });
});

test('submitIfPresent parses the score off the result screen before/while clicking Next', async () => {
  const driver = makeDriver((code) => {
    if (code.includes("=== 'Submit'")) return true;
    if (code.includes("=== 'Yes'")) return true;
    if (code.includes('Your Score')) return { score: 100, clicked: true };
    return false;
  });
  const result = await submitIfPresent(driver);
  assert.deepEqual(result, { submitted: true, score: 100 });
});

test('submitIfPresent keeps polling until the result screen (and its Next button) appear', async () => {
  let pollCount = 0;
  const driver = makeDriver((code) => {
    if (code.includes("=== 'Submit'")) return true;
    if (code.includes("=== 'Yes'")) return true;
    if (code.includes('Your Score')) {
      pollCount += 1;
      if (pollCount < 3) return { score: null, clicked: false };
      return { score: 87, clicked: true };
    }
    return false;
  });
  const result = await submitIfPresent(driver);
  assert.deepEqual(result, { submitted: true, score: 87 });
  assert.equal(pollCount, 3);
});
