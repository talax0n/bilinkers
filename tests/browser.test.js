const assert = require('node:assert/strict');
const { test } = require('node:test');
const { waitForLogin } = require('../src/browser');

function makeFakeDriver(foundOnAttempt) {
  let attempt = 0;
  return {
    async findElements() {
      attempt += 1;
      return attempt >= foundOnAttempt ? [{}] : [];
    },
    async sleep() {},
  };
}

test('waitForLogin resolves true once the post-login element appears', async () => {
  const driver = makeFakeDriver(3);
  const result = await waitForLogin(driver, { pollMs: 1, timeoutMs: 1000, postLoginSelector: '.dashboard' });
  assert.equal(result, true);
});

test('waitForLogin throws when timeout is reached before element appears', async () => {
  const driver = {
    async findElements() { return []; },
    async sleep() {},
  };
  await assert.rejects(
    () => waitForLogin(driver, { pollMs: 1, timeoutMs: 5, postLoginSelector: '.dashboard' }),
    /Timed out waiting for manual login/
  );
});
