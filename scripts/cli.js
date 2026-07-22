require('dotenv').config();
const fs = require('fs');
const { spawn } = require('child_process');
const readline = require('readline');

// One-command entry point: launches your Chromium-based browser with remote
// debugging on, waits for you to log in and open the exercise, then runs the
// matching bot script. Usage: node scripts/cli.js <exercise|iframe> [url]
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

async function main() {
  const mode = process.argv[2];
  const scriptPath = MODES[mode];
  if (!scriptPath) {
    console.error(`Usage: node scripts/cli.js <${Object.keys(MODES).join('|')}> [url]`);
    process.exit(1);
  }

  const url = process.argv[3] || 'https://lms.binus.ac.id';
  const binaryPath = resolveBinaryPath();

  console.log(`Launching browser: ${binaryPath}`);
  const child = spawn(binaryPath, ['--remote-debugging-port=9222', url], { detached: true, stdio: 'ignore' });
  child.on('error', (err) => {
    console.error(`Failed to launch browser at "${binaryPath}": ${err.message}`);
    process.exit(1);
  });
  child.unref();

  await waitForEnter('Log in and open the exercise, then press Enter to start the bot... ');

  const { run } = require(scriptPath);
  await run();
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
