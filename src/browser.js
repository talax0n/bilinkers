const { Builder, By, until } = require('selenium-webdriver');

async function createDriver() {
  return new Builder().forBrowser('chrome').build();
}

async function waitForLogin(driver, { pollMs, timeoutMs, postLoginSelector }) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const found = await driver.findElements(By.css(postLoginSelector));
    if (found.length > 0) return true;
    await driver.sleep(pollMs);
  }
  throw new Error('Timed out waiting for manual login');
}

module.exports = { createDriver, waitForLogin, By, until };
