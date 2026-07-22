const assert = require('node:assert/strict');
const { test } = require('node:test');
const vocabularyIntro = require('../src/questionTypes/vocabularyIntro');

function dom(html, insideIframe = true) {
  return { outerHTML: html, insideIframe };
}

test('detect matches an iframe slide with a "#/n" next link', () => {
  assert.equal(vocabularyIntro.detect(dom('<a href="#/n">Next</a>')), true);
});

test('detect ignores content outside the iframe', () => {
  assert.equal(vocabularyIntro.detect(dom('<a href="#/n">Next</a>', false)), false);
});

test('detect defers to fillInBlank/errorAnalysis on real quiz pages', () => {
  assert.equal(vocabularyIntro.detect(dom('<a href="#/n"></a><input class="quiz-input-sa">')), false);
  assert.equal(vocabularyIntro.detect(dom('<a href="#/n"></a><input class="quiz-input-radio">')), false);
});

test('checkResult always reports correct (nothing is graded on these pages)', async () => {
  assert.equal(await vocabularyIntro.checkResult(), 'correct');
});
