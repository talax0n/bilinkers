const { Builder, By, until } = require('selenium-webdriver');
const chrome = require('selenium-webdriver/chrome');

// BROWSER selects the driver: chrome (default, also covers Chromium-based
// browsers like Brave via BROWSER_BINARY_PATH), firefox, edge, or safari.
async function createDriver() {
  const browserName = (process.env.BROWSER || 'chrome').toLowerCase();
  const binaryPath = process.env.BROWSER_BINARY_PATH || process.env.CHROME_BINARY_PATH;

  if (browserName === 'safari') {
    // safaridriver ships with macOS; enable once via `safaridriver --enable`.
    // No binary path / extra flags to set — Apple doesn't expose them.
    return new Builder().forBrowser('safari').build();
  }

  if (browserName === 'firefox') {
    const firefox = require('selenium-webdriver/firefox');
    const options = new firefox.Options();
    if (binaryPath) options.setBinary(binaryPath);
    return new Builder().forBrowser('firefox').setFirefoxOptions(options).build();
  }

  if (browserName === 'edge') {
    const edge = require('selenium-webdriver/edge');
    const options = new edge.Options();
    if (binaryPath) options.setBinaryPath(binaryPath);
    return new Builder().forBrowser('MicrosoftEdge').setEdgeOptions(options).build();
  }

  const options = new chrome.Options();
  if (binaryPath) {
    options.setChromeBinaryPath(binaryPath);
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

// The LTI player injects its content iframe via JS after load, so it isn't
// there yet on the first check right after a question/page transition.
async function getCurrentQuestionDom(driver, { iframeWaitMs = 8000, iframePollMs = 200 } = {}) {
  await driver.switchTo().defaultContent();

  let iframes = await driver.findElements(By.css('iframe'));
  const deadline = Date.now() + iframeWaitMs;
  while (iframes.length === 0 && Date.now() < deadline) {
    await driver.sleep(iframePollMs);
    iframes = await driver.findElements(By.css('iframe'));
  }

  if (iframes.length > 0) {
    await driver.switchTo().frame(iframes[0]);
  }

  const outerHTML = await driver.executeScript('return document.documentElement.outerHTML');
  return { outerHTML, driver, insideIframe: iframes.length > 0 };
}

async function goToNextQuestion(driver) {
  await driver.switchTo().defaultContent();
  return driver.executeScript(`
    const buttons = Array.from(document.querySelectorAll('button'));
    const btn = buttons.find((b) => b.textContent.trim() === 'Next');
    if (btn) { btn.click(); return true; }
    return false;
  `);
}

module.exports = { createDriver, waitForLogin, getCurrentQuestionDom, goToNextQuestion, By, until };
