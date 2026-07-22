const http = require('http');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { execFileSync } = require('child_process');

// selenium-manager's default (latest stable) chromedriver frequently doesn't
// match a CDP-attached browser's actual Chromium build (e.g. Brave reports
// its own version, not the underlying Chromium's — and even for plain
// Chrome, "latest" chromedriver can drift ahead of an unupdated browser).
// chromedriver only requires a matching *major* version, so this resolves
// one for whatever's actually listening on the debug port: reuse it from
// selenium's local cache if present, otherwise download a matching one.
function fetchBrowserMajorVersion(debuggerAddress) {
  return new Promise((resolve, reject) => {
    http
      .get(`http://${debuggerAddress}/json/version`, (res) => {
        let data = '';
        res.on('data', (chunk) => {
          data += chunk;
        });
        res.on('end', () => {
          try {
            const json = JSON.parse(data);
            const match = /\/(\d+)\./.exec(json.Browser || '');
            if (!match) {
              reject(new Error(`Could not parse a version out of browser string "${json.Browser}"`));
              return;
            }
            resolve(match[1]);
          } catch (err) {
            reject(err);
          }
        });
      })
      .on('error', (err) => {
        reject(
          new Error(
            `Could not reach the browser's debug port at ${debuggerAddress} (${err.message}). ` +
              'Is it running with --remote-debugging-port?'
          )
        );
      });
  });
}

function platformDir() {
  if (process.platform === 'win32') return 'win64';
  if (process.platform === 'darwin') return process.arch === 'arm64' ? 'mac-arm64' : 'mac-x64';
  return 'linux64';
}

function findCachedDriver(major) {
  const cacheBase = process.env.SE_CACHE_PATH || path.join(os.homedir(), '.cache', 'selenium');
  const cacheDir = path.join(cacheBase, 'chromedriver', platformDir());
  if (!fs.existsSync(cacheDir)) return null;

  const match = fs.readdirSync(cacheDir).find((version) => version.startsWith(`${major}.`));
  if (!match) return null;

  const exe = process.platform === 'win32' ? 'chromedriver.exe' : 'chromedriver';
  const driverPath = path.join(cacheDir, match, exe);
  return fs.existsSync(driverPath) ? driverPath : null;
}

function seleniumManagerBinary() {
  const platform = process.platform === 'win32' ? 'windows' : process.platform === 'darwin' ? 'macos' : 'linux';
  const exe = platform === 'windows' ? 'selenium-manager.exe' : 'selenium-manager';
  return path.join(path.dirname(require.resolve('selenium-webdriver/package.json')), 'bin', platform, exe);
}

function downloadMatchingDriver(major) {
  const output = execFileSync(
    seleniumManagerBinary(),
    ['--browser', 'chrome', '--browser-version', major, '--output', 'json'],
    { timeout: 60000 }
  ).toString();
  const parsed = JSON.parse(output);
  if (!parsed.result || !parsed.result.driver_path) {
    throw new Error(parsed.result && parsed.result.message ? parsed.result.message : 'unknown selenium-manager error');
  }
  return parsed.result.driver_path;
}

async function resolveChromedriverPath(debuggerAddress = 'localhost:9222') {
  const major = await fetchBrowserMajorVersion(debuggerAddress);

  const cached = findCachedDriver(major);
  if (cached) return cached;

  try {
    return downloadMatchingDriver(major);
  } catch (err) {
    throw new Error(
      `Could not find or download a chromedriver matching browser major version ${major}: ${err.message}\n` +
        'Download one manually from https://googlechromelabs.github.io/chrome-for-testing/ and set ' +
        'CHROMEDRIVER_PATH in .env to point at it.'
    );
  }
}

module.exports = { resolveChromedriverPath };
