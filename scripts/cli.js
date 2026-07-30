require('dotenv').config();
const fs = require('fs');
const { spawn } = require('child_process');
const readline = require('readline');
const { Builder, By } = require('selenium-webdriver');
const chrome = require('selenium-webdriver/chrome');
const { resolveChromedriverPath } = require('../src/chromedriver');
const logger = require('../src/logger');
const { isGroupedReadingLayout, groupedReadingLayoutFromDocument } = require('./grouped-reading');

// One-command entry point: launches your Chromium-based browser with remote
// debugging on, waits for you to log in and open the exercise, then runs the
// matching bot script. Usage: node scripts/cli.js [exercise|iframe|unit|checkpoint|course|level] [url]
// Mode is optional — if omitted, it's auto-detected from the page after you
// press Enter (the native MUI exercise, an LTI-embedded iframe activity, and
// a unit's own activity list all have distinct, unambiguous DOM markers), so
// you don't have to know which one to pick. For `unit`, open the unit page
// (e.g. "Unit 2", the list of activity rows — not a single exercise) before
// pressing Enter; it walks every unfinished row, running exercise/iframe on
// each in turn.
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
  unit: '../scripts/run-unit.js',
  iframe: '../scripts/run-iframe-exercise.js',
  reading: '../scripts/run-reading-exercise.js',
  checkpoint: '../scripts/run-checkpoint.js',
  'checkpoint-capture': '../scripts/run-checkpoint-capture.js',
  course: '../scripts/run-course.js',
  level: '../scripts/run-level.js',
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

// Three page kinds share this app's chrome (header, nav icons, the unit's
// own row list), all built from the same MuiButtonBase/.bl-w-full
// components — checking for those generically (as this used to) matches
// every page, including the unit list itself, and always misclassifies it
// as 'exercise' before ever getting a chance to check for an iframe or the
// row list. Checked in order of how unambiguous each signal actually is:
//   1. iframe present -> 'iframe' (the LTI/Bits player only ever appears
//      wrapped in an iframe, so this is independent of the app chrome noise)
//   2. iframe reading -> 'reading'
//   3. checkpoint gate -> 'checkpoint'
//   4. top-level grouped reading -> 'reading'
//   5. generic top-level option buttons -> 'exercise'
//   6. two or more 'button.bl-w-full' rows each carrying a
//      '.bl-text-ellipsis' title label -> 'unit' (verified live against
//      the "Unit 2" activity list: exactly this shape, no iframe, no
//      lettered options)
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
      // The reading BlExercise (passage + numbered 1..N tiles, lettered
      // option buttons) is ALSO iframe-embedded, but uses the MUI
      // 'bl-w-full justify-content-start' option buttons rather than the
      // Bits player's quiz-input-* markup — so it's distinguished from a
      // native (top-level) 'exercise' by living inside the iframe.
      if (iframeHtml.includes('bl-w-full justify-content-start')) {
        return 'reading';
      }
      await driver.switchTo().defaultContent();
    }

    const topHtml = await driver.executeScript('return document.documentElement.outerHTML');

    // A checkpoint's gate page ("ENG-B1.2 - INDEPENDENT (V2) - Checkpoint 1",
    // Total Question/Passing Score/Maximum Attempt, a "Start Attempt N" or
    // "Continue" button) has the exact same shape as a plain exercise's
    // BlExercise gate — "Checkpoint" in the heading is the one thing that
    // tells them apart, so that's the check, not the generic gate markers.
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

    // A course's roadmap (the isometric city map: "Unit 1", "Checkpoint 1",
    // ... hexagon nodes) renders each node as a rasterized PNG inside a
    // '.hoverable-pointer' div with a plain-text label sibling — unique to
    // this page, not shared with the unit/exercise/checkpoint chrome.
    const isCourseMap = await driver.executeScript(`
      return [...document.querySelectorAll('.hoverable-pointer')].some((el) =>
        /^(Unit|Checkpoint)\\d+$/i.test(el.textContent.replace(/\\s+/g, '').trim())
      );
    `);
    if (isCourseMap) {
      return 'course';
    }

    // The island map (one level up from a course roadmap) reuses the exact
    // same '.hoverable-pointer' node shape, just with "B1.1"-style course
    // labels ("B1.1", "C2.2", ...) instead of "Unit1"/"Checkpoint1" — so it
    // must be checked with its own regex, not lumped into isCourseMap.
    const isLevelMap = await driver.executeScript(`
      return [...document.querySelectorAll('.hoverable-pointer')].some((el) =>
        /^[BC]\\d\\.\\d$/i.test(el.textContent.replace(/\\s+/g, '').trim())
      );
    `);
    if (isLevelMap) {
      return 'level';
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

  // The plain JSON-line logger and a live OpenTUI dashboard both fight for
  // the same terminal, so the dashboard only takes over when running
  // interactively (a real TTY) — piped/CI output keeps the JSON lines.
  let dashboard = null;
  if (process.stdout.isTTY && !process.env.NO_TUI) {
    const { createDashboard } = require('../src/dashboard');
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
