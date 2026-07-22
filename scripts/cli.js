require('dotenv').config();
const fs = require('fs');
const { spawn } = require('child_process');
const readline = require('readline');
const { Builder, By } = require('selenium-webdriver');
const chrome = require('selenium-webdriver/chrome');
const { resolveChromedriverPath } = require('../src/chromedriver');

// One-command entry point: launches your Chromium-based browser with remote
// debugging on, waits for you to log in and open the exercise, then runs the
// matching bot script. Usage: node scripts/cli.js [exercise|iframe] [url]
// Mode is optional — if omitted, it's auto-detected from the page after you
// press Enter (native MUI app vs LTI-embedded iframe activity have distinct,
// unambiguous DOM markers), so you don't have to know which one to pick.
const DEFAULT_BINARY_PATHS = {
  darwin: {
    chrome: '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    brave: '/Applications/Brave Browser.app/Contents/MacOS/Brave Browser',
    edge: '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge',
  },
  win32: {
    chrome: 'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
    brave: 'C:\\Program Files\\BraveSoftware\\Brave-Browser\\Application\\brave.exe',
    edge: 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
  },
};

const MODES = {
  exercise: '../scripts/run-exercise.js',
  iframe: '../scripts/run-iframe-exercise.js',
};

function resolveBinaryPath() {
  const browserKey = (process.env.BROWSER || 'chrome').toLowerCase();

  const explicitPath = process.env.BROWSER_BINARY_PATH || process.env.CHROME_BINARY_PATH;
  if (explicitPath) {
    if (!fs.existsSync(explicitPath)) {
      throw new Error(
        `BROWSER_BINARY_PATH (or CHROME_BINARY_PATH) is set to "${explicitPath}", but nothing exists there. ` +
          `Double check the path in .env — see the README's "Finding your browser binary path" section.`
      );
    }
    return explicitPath;
  }

  const platformPaths = DEFAULT_BINARY_PATHS[process.platform];
  const binaryPath = platformPaths && platformPaths[browserKey];
  if (!binaryPath) {
    throw new Error(
      `No default binary path known for BROWSER=${browserKey} on ${process.platform}. Set BROWSER_BINARY_PATH in .env.`
    );
  }
  if (!fs.existsSync(binaryPath)) {
    throw new Error(
      `BROWSER=${browserKey} defaults to "${binaryPath}" on ${process.platform}, but it's not installed there ` +
        `(no app found at that path). Either install ${browserKey}, or set BROWSER to a browser you do have ` +
        `(chrome/brave/edge) and/or set BROWSER_BINARY_PATH in .env to its actual location — ` +
        `see the README's "Finding your browser binary path" section.`
    );
  }
  return binaryPath;
}

function waitForEnter(promptText) {
  return new Promise((resolve) => {
    const rl = readline.createInterface({ input: process.stdin, output: process.stdout });
    rl.question(promptText, () => {
      rl.close();
      resolve();
    });
  });
}

// The two activity types have distinct, unambiguous DOM markers: the native
// MUI app renders MuiButtonBase/.bl-w-full option buttons directly on the
// page, while the LTI-embedded activity renders quiz-input-sa/quiz-input-radio
// inputs inside an iframe. Checking for these removes the need to know which
// `npm run` command matches what's on screen.
async function detectMode() {
  const options = new chrome.Options();
  options.debuggerAddress('localhost:9222');
  const chromedriverPath = await resolveChromedriverPath();
  const driver = await new Builder().forBrowser('chrome').setChromeOptions(options).setChromeService(new chrome.ServiceBuilder(chromedriverPath)).build();

  const handles = await driver.getAllWindowHandles();
  for (const handle of handles) {
    await driver.switchTo().window(handle);
    const url = await driver.getCurrentUrl();
    if (!url.includes('lms.binus.ac.id')) continue;

    const topHtml = await driver.executeScript('return document.documentElement.outerHTML');
    if (topHtml.includes('MuiButtonBase') || topHtml.includes('bl-w-full')) {
      return 'exercise';
    }

    await driver.switchTo().defaultContent();
    const iframes = await driver.findElements(By.css('iframe'));
    if (iframes.length > 0) {
      await driver.switchTo().frame(iframes[0]);
      const iframeHtml = await driver.executeScript('return document.documentElement.outerHTML');
      if (iframeHtml.includes('quiz-input-sa') || iframeHtml.includes('quiz-input-radio')) {
        return 'iframe';
      }
    }
    return null;
  }
  throw new Error('No lms.binus.ac.id tab found to detect the exercise type from.');
}

async function main() {
  const rawMode = process.argv[2];
  const explicitMode = MODES[rawMode] ? rawMode : undefined;
  const url = (explicitMode ? process.argv[3] : rawMode) || 'https://lms.binus.ac.id';

  const binaryPath = resolveBinaryPath();

  console.log(`Launching browser: ${binaryPath}`);
  const child = spawn(binaryPath, ['--remote-debugging-port=9222', url], { detached: true, stdio: 'ignore' });
  child.on('error', (err) => {
    console.error(`Failed to launch browser at "${binaryPath}": ${err.message}`);
    process.exit(1);
  });
  child.unref();

  await waitForEnter('Log in and open the exercise, then press Enter to start the bot... ');

  let mode = explicitMode;
  if (!mode) {
    console.log('Detecting exercise type...');
    mode = await detectMode();
    if (!mode) {
      console.error(
        `Could not auto-detect the exercise type. Run with an explicit mode instead: node scripts/cli.js <${Object.keys(MODES).join('|')}>`
      );
      process.exit(1);
    }
    console.log(`Detected: ${mode}`);
  }

  const { run } = require(MODES[mode]);
  await run();
}

if (require.main === module) {
  main().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}
