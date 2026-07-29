const assert = require('node:assert/strict');
const { test } = require('node:test');
const fillInBlank = require('../src/questionTypes/fillInBlank');

function dom(html, insideIframe = true) {
  return { outerHTML: html, insideIframe };
}

const SAMPLE = `
<div><strong>I wish it __________ (rainy).</strong></div>
<input class="quiz-input-sa">
`;

const LIVE_SHAPED = `
<p class="question-text">
  <b data-x="1"><span>I don't know ___.</span></b>
  <b class="cue"><span>(they, join us)</span></b>
</p>
<input class="quiz-input-sa">
`;

test('detect matches a quiz-input-sa page', () => {
  assert.equal(fillInBlank.detect(dom(SAMPLE)), true);
});

test('detect rejects a page with no quiz-input-sa', () => {
  assert.equal(fillInBlank.detect(dom('<div>no blanks here</div>')), false);
});

test('parse keeps existing strong fixture sentence extraction', () => {
  const parsed = fillInBlank.parse(dom(SAMPLE));
  assert.equal(parsed.sentence, 'I wish it __________ (rainy).');
  assert.equal(parsed.blankCount, 1);
});

test('parse extracts live-shaped b markup with nested spans and cue text', () => {
  const parsed = fillInBlank.parse(dom(LIVE_SHAPED));
  assert.equal(parsed.sentence, "I don't know ___. (they, join us)");
  assert.equal(parsed.blankCount, 1);
});

// The generic .correct.timeout/.incorrect.timeout markers were verified
// live to be unreliable for this quiz type — a genuinely correct answer
// still flashed the .incorrect banner on the way to .quiz-next-btn going
// visible. checkResult must trust only .quiz-next-btn, never the incorrect
// banner, for "correct".
test('checkResult resolves correct once .quiz-next-btn becomes visible, even after an incorrect-banner flash', async () => {
  let calls = 0;
  const driver = {
    async executeScript(code) {
      calls += 1;
      if (code.includes('quiz-next-btn')) {
        // First couple of checks: not yet visible (still mid-grading,
        // banner flashing). From the 3rd check on: visible.
        return calls >= 3;
      }
      return null;
    },
    async sleep() {},
  };

  const result = await fillInBlank.checkResult({ driver });
  assert.equal(result, 'correct');
});

// checkResult polls a real 5s wall-clock deadline (matches quizMatching's
// own checkResult), which outlives bun's default 5s per-test timeout; this
// test alone needs a longer explicit timeout, not a production change.
test('checkResult resolves incorrect after the deadline if .quiz-next-btn never appears, retrying Submit once along the way', { timeout: 8000 }, async () => {
  let submitClicks = 0;
  const driver = {
    async executeScript(code) {
      if (code.includes('quiz-next-btn')) return false;
      if (code.includes('quiz-submit-btn')) {
        submitClicks += 1;
        return null;
      }
      return null;
    },
    async sleep() {},
  };

  const result = await fillInBlank.checkResult({ driver });
  assert.equal(result, 'incorrect');
  assert.equal(submitClicks, 1);
});
