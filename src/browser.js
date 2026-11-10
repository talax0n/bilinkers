const { Builder, By } = require('selenium-webdriver');
const chrome = require('selenium-webdriver/chrome');
const { resolveChromedriverPath } = require('./chromedriver');

const LMS_HOST = 'lms.binus.ac.id';

// Attaches to the browser scripts/cli.js launched with --remote-debugging-port=9222
// and focuses its LMS tab. CHROMEDRIVER_PATH overrides chromedriver auto-detection.
async function attachToLms() {
  const options = new chrome.Options();
  options.debuggerAddress('localhost:9222');
  const chromedriverPath = process.env.CHROMEDRIVER_PATH || (await resolveChromedriverPath());
  const driver = await new Builder()
    .forBrowser('chrome')
    .setChromeOptions(options)
    .setChromeService(new chrome.ServiceBuilder(chromedriverPath))
    .build();

  for (const handle of await driver.getAllWindowHandles()) {
    await driver.switchTo().window(handle);
    if ((await driver.getCurrentUrl()).includes(LMS_HOST)) return driver;
  }
  throw new Error(`No ${LMS_HOST} tab found in the attached browser.`);
}

async function waitForUrlChange(driver, previousUrl, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const url = await driver.getCurrentUrl();
    if (url !== previousUrl) return url;
    await driver.sleep(200);
  }
  return null;
}

// The LTI player injects its content iframe via JS after load, so it isn't
// there yet on the first check right after a question/page transition.
//
// Opening an activity also briefly renders, inside that same iframe: first
// an intermediate "LTI launcher" page (a hidden auto-submitting form, id
// "ltiForm", POSTing off to the actual Bits player), then — even after that
// POST completes and the Bits app itself has loaded — an empty <main
// id="content"> for a beat while its JS populates the slide/question into
// it. Caught in either state, the page has no question markers and no #/n
// link, so it was previously misread as an unanswerable/unhandled page.
// Re-checking (with fresh iframe lookups, since navigation happens inside
// the iframe's own document rather than replacing the <iframe> element
// itself) waits both out instead.
async function getCurrentQuestionDom(driver, { iframeWaitMs = 30000, iframePollMs = 200, launcherWaitMs = 30000 } = {}) {
  await driver.switchTo().defaultContent();

  let iframes = await driver.findElements(By.css('iframe'));
  const deadline = Date.now() + iframeWaitMs;
  while (iframes.length === 0 && Date.now() < deadline) {
    await driver.sleep(iframePollMs);
    iframes = await driver.findElements(By.css('iframe'));
  }

  if (iframes.length > 0) {
    await driver.switchTo().frame(iframes[0]);

    const launcherDeadline = Date.now() + launcherWaitMs;
    for (;;) {
      const notReady = await driver.executeScript(`
        if (!document.documentElement || !document.body) return true;
        if (document.body.innerText.includes('Do you want to continue from the page your last visited?')) {
          const no = [...document.querySelectorAll('span')].find((el) => el.textContent.trim() === 'NO');
          if (no) no.parentElement.click();
          return true;
        }
        if (document.getElementById('ltiForm')) return true;
        const content = document.getElementById('content');
        if (!content) return true;
        return content.children.length === 0;
      `);
      if (!notReady || Date.now() > launcherDeadline) break;
      await driver.sleep(300);
      await driver.switchTo().defaultContent();
      const freshIframes = await driver.findElements(By.css('iframe'));
      if (freshIframes.length === 0) break;
      await driver.switchTo().frame(freshIframes[0]);
    }
  }

  const outerHTML = await driver.executeScript('return document.documentElement ? document.documentElement.outerHTML : null');
  // A past listening question's MuiSlider-root audio player is left mounted
  // (just visually hidden, not removed) once the SPA moves on to a later
  // question — matching purely on outerHTML substring then keeps matching
  // audioMultipleChoice forever after the first real listening question,
  // even though its markup no longer represents the current question.
  // offsetParent is null for any element that (or whose ancestor) is
  // display:none, so this only counts a slider that's actually on-screen.
  const hasVisibleSlider = await driver.executeScript(`
    return [...document.querySelectorAll('.MuiSlider-root')].some((el) => el.offsetParent !== null);
  `);
  return { outerHTML, driver, insideIframe: iframes.length > 0, hasVisibleSlider };
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

module.exports = { attachToLms, waitForUrlChange, getCurrentQuestionDom, goToNextQuestion };
