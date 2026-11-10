require('dotenv').config();
const fs = require('fs');
const { spawn } = require('child_process');
const readline = require('readline');
const { By } = require('selenium-webdriver');
const { attachToLms } = require('./browser');
const logger = require('./logger');
const { isGroupedReadingLayout, groupedReadingLayoutFromDocument } = require('./groupedReading');

// Usage: node src/cli.js [mode] [url]. Mode is auto-detected when omitted.
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
  exercise: './runners/exercise.js',
  unit: './runners/unit.js',
  iframe: './runners/iframe.js',
  reading: './runners/reading.js',
  checkpoint: './runners/checkpoint.js',
  course: './runners/course.js',
  'final-test': './runners/final-test.js',
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

// Checked in order. App chrome (MuiButtonBase, .bl-w-full) is on every page, so only specific markers count.
async function detectMode() {
  const driver = await attachToLms();

  await driver.switchTo().defaultContent();
  const iframes = await driver.findElements(By.css('iframe'));
  if (iframes.length > 0) {
    await driver.switchTo().frame(iframes[0]);
    const iframeHtml = await driver.executeScript('return document.documentElement.outerHTML');
    if (
      iframeHtml.includes('quiz-input-sa') ||
      iframeHtml.includes('quiz-input-radio') ||
      iframeHtml.includes('quiz-matching') ||
      /href="#\/n"/.test(iframeHtml)
    ) {
      return 'iframe';
    }
    // A reading BlExercise is also iframe-embedded but uses MUI option buttons, not quiz-input-*.
    if (iframeHtml.includes('bl-w-full justify-content-start')) {
      return 'reading';
    }
    await driver.switchTo().defaultContent();
  }

  const topHtml = await driver.executeScript('return document.documentElement.outerHTML');

  // A checkpoint gate looks like an exercise gate. Only the word "Checkpoint" differs.
  const isCheckpointGate = await driver.executeScript(`
    const hasCheckpointHeading = /Checkpoint/i.test(document.body.textContent);
    const buttons = Array.from(document.querySelectorAll('button'));
    const hasGateButton = buttons.some((b) => {
      const text = b.textContent.trim();
      return text.startsWith('Start Attempt') || text === 'Continue';
    });
    return hasCheckpointHeading && hasGateButton;
  `);
  if (isCheckpointGate) {
    return 'checkpoint';
  }

  const isCourseMap = await driver.executeScript(`
    return [...document.querySelectorAll('.hoverable-pointer')].some((el) =>
      /^(Unit|Checkpoint)\\d+$/i.test(el.textContent.replace(/\\s+/g, '').trim())
    );
  `);
  if (isCourseMap) {
    return 'course';
  }

  const topLevelLayout = await driver.executeScript(`return (${groupedReadingLayoutFromDocument.toString()})()`);
  if (isGroupedReadingLayout(topLevelLayout)) {
    return 'reading';
  }

  if (topHtml.includes('bl-w-full justify-content-start')) {
    return 'exercise';
  }

  const isUnitList = await driver.executeScript(`
    const rows = document.querySelectorAll('button.bl-w-full');
    let titled = 0;
    rows.forEach((b) => { if (b.querySelector('.bl-text-ellipsis')) titled += 1; });
    return titled >= 2;
  `);
  if (isUnitList) {
    return 'unit';
  }

  return null;
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
        `Could not auto-detect the exercise type. Run with an explicit mode instead: node src/cli.js <${Object.keys(MODES).join('|')}>`
      );
      process.exit(1);
    }
    console.log(`Detected: ${mode}`);
  }

  let dashboard = null;
  if (process.stdout.isTTY && !process.env.NO_TUI) {
    const { createDashboard } = require('./dashboard');
    dashboard = await createDashboard({ title: `Beelingua Bot — ${mode}` });
    logger.setSink(dashboard.onLog);
  }

  try {
    const { run } = require(MODES[mode]);
    await run();
  } finally {
    if (dashboard) dashboard.stop();
  }
}

if (require.main === module) {
  main().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}
