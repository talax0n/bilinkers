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
  assert.equal(cfg.retry.maxAnswerRetries, 25);
  assert.equal(cfg.paths.progressFile, './progress.json');
  assert.equal(cfg.paths.unhandledLogDir, './logs/unhandled');
});

test('buildConfig uses MAX_ANSWER_RETRIES env override', () => {
  const cfg = buildConfig({ MAX_ANSWER_RETRIES: '7' });
  assert.equal(cfg.retry.maxAnswerRetries, 7);
});

test('buildConfig falls back for invalid MAX_ANSWER_RETRIES', () => {
  assert.equal(buildConfig({ MAX_ANSWER_RETRIES: '-1' }).retry.maxAnswerRetries, 25);
  assert.equal(buildConfig({ MAX_ANSWER_RETRIES: '1.5' }).retry.maxAnswerRetries, 25);
  assert.equal(buildConfig({ MAX_ANSWER_RETRIES: 'nope' }).retry.maxAnswerRetries, 25);
});
