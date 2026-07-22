const assert = require('node:assert/strict');
const { test } = require('node:test');
const { buildConfig } = require('../src/config');

test('buildConfig defaults model when not set', () => {
  const cfg = buildConfig({ OPENAI_BASE_URL: 'https://x', OPENAI_API_KEY: 'k' });
  assert.equal(cfg.openai.model, 'gpt-4o');
  assert.equal(cfg.openai.baseURL, 'https://x');
  assert.equal(cfg.openai.apiKey, 'k');
});

test('buildConfig uses provided model', () => {
  const cfg = buildConfig({ OPENAI_MODEL: 'custom-model' });
  assert.equal(cfg.openai.model, 'custom-model');
});

test('buildConfig sets retry and path defaults', () => {
  const cfg = buildConfig({});
  assert.equal(cfg.retry.maxAnswerRetries, 10);
  assert.equal(cfg.paths.progressFile, './progress.json');
  assert.equal(cfg.paths.unhandledLogDir, './logs/unhandled');
});
