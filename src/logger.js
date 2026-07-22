const fs = require('node:fs');
const path = require('node:path');

// When a dashboard (src/dashboard.js) is active, JSON lines would just
// corrupt its rendered UI, so output is redirected there instead of stdout.
let sink = null;
function setSink(nextSink) {
  sink = nextSink;
}

function log(level, message, meta = {}) {
  if (sink) {
    sink(level, message, meta);
    return;
  }
  console.log(JSON.stringify({ time: new Date().toISOString(), level, message, ...meta }));
}

function info(message, meta) { log('info', message, meta); }
function warn(message, meta) { log('warn', message, meta); }
function error(message, meta) { log('error', message, meta); }

function saveUnhandled(dir, name, { screenshotBuffer, html }) {
  fs.mkdirSync(dir, { recursive: true });
  const base = path.join(dir, name);
  let pngPath = null;
  if (screenshotBuffer) {
    pngPath = `${base}.png`;
    fs.writeFileSync(pngPath, screenshotBuffer);
  }
  const htmlPath = `${base}.html`;
  fs.writeFileSync(htmlPath, html);
  return { pngPath, htmlPath };
}

module.exports = { info, warn, error, saveUnhandled, setSink };
