const assert = require('node:assert/strict');
const { test } = require('node:test');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { loadProgress, saveProgress } = require('../src/progress');

test('loadProgress returns defaults when file does not exist', () => {
  const filePath = path.join(os.tmpdir(), `beelingua-progress-${Date.now()}-missing.json`);
  const progress = loadProgress(filePath);
  assert.deepEqual(progress, { lessonIndex: 0, sectionIndex: 0 });
});

test('saveProgress then loadProgress roundtrips', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'beelingua-progress-'));
  const filePath = path.join(dir, 'progress.json');
  saveProgress(filePath, { lessonIndex: 2, sectionIndex: 5 });
  const loaded = loadProgress(filePath);
  assert.deepEqual(loaded, { lessonIndex: 2, sectionIndex: 5 });
});

test('loadProgress returns defaults when file is corrupted', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'beelingua-progress-'));
  const filePath = path.join(dir, 'progress.json');
  fs.writeFileSync(filePath, '{ not valid json');
  const progress = loadProgress(filePath);
  assert.deepEqual(progress, { lessonIndex: 0, sectionIndex: 0 });
});
