const assert = require('node:assert/strict');
const { test } = require('node:test');
const quizMatching = require('../src/questionTypes/quizMatching');

function dom(html, insideIframe = true) {
  return { outerHTML: html, insideIframe };
}

const SAMPLE = `
<div id="quiz-matching-16" class="quiz quiz-matching">
  <div id="quiz-matching-16-target" class="dropzone quiz-matching-16" data-drag-target="1" style="left: 5%; top: 32%; width: 22%; height: 8%;"></div>
  <div id="quiz-matching-16-drag" class="draggable quiz-matching-16" data-drag="1" style="left: 26%; top: 74%; width: 20%; height: 5%;">leaky</div>
  <div id="quiz-matching-n-drag-2" class="draggable quiz-matching-n" data-drag="2" style="left: 36%; top: 60%; width: 20%; height: 5%;">unclog</div>
</div>
`;

test('detect matches a quiz-matching drag-drop page', () => {
  assert.equal(quizMatching.detect(dom(SAMPLE)), true);
});

test('detect ignores content outside the iframe', () => {
  assert.equal(quizMatching.detect(dom(SAMPLE, false)), false);
});

test('detect ignores pages with no drag/drop markup', () => {
  assert.equal(quizMatching.detect(dom('<a href="#/n">Next</a>')), false);
});

test('answer fills each dropzone with the draggable whose data-drag matches its data-drag-target, then submits', async () => {
  const executed = [];
  const driver = {
    executeScript: async (script) => {
      executed.push(script);
      if (executed.length === 1) return 1; // count of dropzones filled
      return undefined;
    },
    sleep: async () => {},
  };
  await quizMatching.answer(driver);
  assert.equal(executed.length, 2);
  assert.match(executed[0], /data-drag-target/);
  assert.match(executed[1], /quiz-submit-btn/);
});

test('answer throws when no dropzone/draggable pairs are found (nothing filled)', async () => {
  const driver = { executeScript: async () => 0, sleep: async () => {} };
  await assert.rejects(() => quizMatching.answer(driver), /no dropzone\/draggable pairs/);
});
