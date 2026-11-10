const { Builder, By } = require('selenium-webdriver');
const chrome = require('selenium-webdriver/chrome');
const { resolveChromedriverPath } = require('./chromedriver');

const LMS_HOST = 'lms.binus.ac.id';

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

// The iframe is injected late. Inside it, a launcher form (#ltiForm) and then an empty
// #content render before the question, so wait both out with fresh iframe lookups.
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
  // Earlier listening sliders stay mounted but hidden, so only count a visible one.
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
