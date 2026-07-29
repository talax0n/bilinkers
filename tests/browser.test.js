const assert = require('node:assert/strict');
const { test } = require('node:test');
const { getCurrentQuestionDom, waitForLogin } = require('../src/browser');

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

test('getCurrentQuestionDom treats absent iframe body during reload as not ready and retries fresh iframe lookup', async () => {
  const contexts = [];
  let currentContext = 'default';
  let iframeLookupCount = 0;
  let readinessCheckCount = 0;
  let sleepCount = 0;

  const driver = {
    switchTo() {
      return {
        defaultContent() {
          currentContext = 'default';
          contexts.push('default');
          return Promise.resolve();
        },
        frame() {
          currentContext = 'iframe';
          contexts.push('iframe');
          return Promise.resolve();
        },
      };
    },
    async findElements() {
      assert.equal(currentContext, 'default');
      iframeLookupCount += 1;
      return [{}];
    },
    async sleep(ms) {
      assert.equal(ms, 300);
      sleepCount += 1;
    },
    async executeScript(script) {
      if (script.includes("Do you want to continue from the page your last visited?")) {
        assert.equal(currentContext, 'iframe');
        readinessCheckCount += 1;
        if (readinessCheckCount === 1) return true;
        return false;
      }
      if (script === 'return document.documentElement ? document.documentElement.outerHTML : null') {
        assert.equal(currentContext, 'iframe');
        return '<html><body><main id="content"><div>Ready</div></main></body></html>';
      }
      throw new Error(`Unexpected script: ${script}`);
    },
  };

  const result = await getCurrentQuestionDom(driver, { iframeWaitMs: 100, iframePollMs: 1, launcherWaitMs: 1000 });

  assert.equal(result.insideIframe, true);
  assert.equal(result.outerHTML, '<html><body><main id="content"><div>Ready</div></main></body></html>');
  assert.equal(iframeLookupCount, 2);
  assert.equal(readinessCheckCount, 2);
  assert.equal(sleepCount, 1);
  assert.deepEqual(contexts, ['default', 'iframe', 'default', 'iframe']);
});
