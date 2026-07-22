const assert = require('node:assert/strict');
const { test } = require('node:test');
const http = require('node:http');
const { resolveChromedriverPath } = require('../src/chromedriver');

function withFakeDebugServer(browserString, fn) {
  return new Promise((resolve, reject) => {
    const server = http.createServer((req, res) => {
      res.end(JSON.stringify({ Browser: browserString }));
    });
    server.listen(0, '127.0.0.1', async () => {
      try {
        await fn(`127.0.0.1:${server.address().port}`);
        resolve();
      } catch (err) {
        reject(err);
      } finally {
        server.close();
      }
    });
  });
}

test('resolveChromedriverPath rejects with a clear message when the debug port is unreachable', async () => {
  await assert.rejects(
    () => resolveChromedriverPath('127.0.0.1:1'),
    /Could not reach the browser's debug port/
  );
});

test('resolveChromedriverPath rejects with a clear message when the Browser field has no version', async () => {
  await withFakeDebugServer('not-a-version-string', async (address) => {
    await assert.rejects(() => resolveChromedriverPath(address), /Could not parse a version/);
  });
});
