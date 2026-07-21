const assert = require('node:assert/strict');
const { test } = require('node:test');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { saveUnhandled } = require('../src/logger');

test('saveUnhandled writes html and png files into a fresh dir', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'beelingua-log-'));
  const targetDir = path.join(dir, 'nested', 'unhandled');
  const result = saveUnhandled(targetDir, 'dragdrop-001', {
    screenshotBuffer: Buffer.from([1, 2, 3]),
    html: '<div>question</div>',
  });

  assert.equal(fs.existsSync(result.pngPath), true);
  assert.equal(fs.existsSync(result.htmlPath), true);
  assert.equal(fs.readFileSync(result.htmlPath, 'utf8'), '<div>question</div>');
});

test('saveUnhandled skips png when no screenshotBuffer given', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'beelingua-log-'));
  const result = saveUnhandled(dir, 'text-only', { html: '<p>x</p>' });

  assert.equal(result.pngPath, null);
  assert.equal(fs.existsSync(result.htmlPath), true);
});
