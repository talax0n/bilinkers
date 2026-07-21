const { Builder, By, until } = require('selenium-webdriver');
const chrome = require('selenium-webdriver/chrome');

async function createDriver() {
  const options = new chrome.Options();
  if (process.env.CHROME_BINARY_PATH) {
    options.setChromeBinaryPath(process.env.CHROME_BINARY_PATH);
  }
  // --test-type=webdriver (a chromedriver default flag) crashes Chromium at
  // startup on this machine with "Mach rendezvous failed, terminating
  // process (parent died?)" — excluding it is required for the browser to
  // launch at all here.
  options.excludeSwitches('test-type');
  options.addArguments('--no-sandbox', '--disable-dev-shm-usage');

  return new Builder().forBrowser('chrome').setChromeOptions(options).build();
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
