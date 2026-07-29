const assert = require('node:assert/strict');
const { test } = require('node:test');
const errorAnalysis = require('../src/questionTypes/errorAnalysis');

function dom(html, insideIframe = true) {
  return { outerHTML: html, insideIframe };
}

const UNDERLINE_SAMPLE = `
<div>
  <b><u>had went</u></b> to the store, and <b><u>bought</u></b> some milk,
  then <b><u>drove</u></b> home before it <b><u>rained</u></b>.
</div>
<div class="quiz-input-radio"></div>
<div class="quiz-input-radio"></div>
<div class="quiz-input-radio"></div>
<div class="quiz-input-radio"></div>
`;

// Verified live: a "By the time the guests arrived..." question marked its
// five candidates as absolutely-positioned overlay labels
// (data-options: "...label: A...") instead of inline <u> tags — no <u>
// appears anywhere in the markup for this template variant.
const OVERLAY_SAMPLE = `
<div>By the time the guests arrived, the organizers have already set up all the chairs.</div>
<div id="container-f" data-options="label: A"></div>
<div id="container-g" data-options="label: B"></div>
<div id="container-h" data-options="label: C"></div>
<div id="container-i" data-options="label: D"></div>
<div id="container-j" data-options="label: E"></div>
<input class="quiz-input-radio">
<input class="quiz-input-radio">
<input class="quiz-input-radio">
<input class="quiz-input-radio">
<input class="quiz-input-radio">
`;

test('detect matches an errorAnalysis quiz-input-radio page', () => {
  assert.equal(errorAnalysis.detect(dom(UNDERLINE_SAMPLE)), true);
});

test('parse extracts one candidate per underlined word', () => {
  const { candidates } = errorAnalysis.parse(dom(UNDERLINE_SAMPLE));
  assert.deepEqual(
    candidates.map((c) => c.letter),
    ['A', 'B', 'C', 'D']
  );
  assert.equal(candidates[0].word, 'had went');
});

test('parse falls back to one candidate per radio when no <u> tags are present', () => {
  const { candidates } = errorAnalysis.parse(dom(OVERLAY_SAMPLE));
  assert.deepEqual(
    candidates.map((c) => c.letter),
    ['A', 'B', 'C', 'D', 'E']
  );
});

// Verified live: a question had 5 .quiz-input-radio options but only 4 <u>
// tags (one candidate word wasn't underlined) — candidates came out
// non-empty but one short, so the old "only pad when zero found" fallback
// never kicked in and the brute-forcer could never try the 5th (actually
// correct) option.
const PARTIAL_UNDERLINE_SAMPLE = `
<div>
  She left <b><u>her umbrella at</u></b> the cafe, so she <b><u>should have gotten</u></b> wet
  <b><u>on</u></b> <b><u>her way</u></b> home.
</div>
<input class="quiz-input-radio">
<input class="quiz-input-radio">
<input class="quiz-input-radio">
<input class="quiz-input-radio">
<input class="quiz-input-radio">
`;

test('parse pads candidates up to the real radio count when fewer <u> tags are found than options', () => {
  const { candidates } = errorAnalysis.parse(dom(PARTIAL_UNDERLINE_SAMPLE));
  assert.deepEqual(
    candidates.map((c) => c.letter),
    ['A', 'B', 'C', 'D', 'E']
  );
  assert.equal(candidates[0].word, 'her umbrella at');
  assert.equal(candidates[3].word, 'her way');
  assert.equal(candidates[4].word, ''); // padded — 5th radio has no matching <u>
});

// This quiz sub-type only honors one click-through-Submit per page load —
// once graded, .quiz-input-radio inputs stay permanently disabled and
// re-clicking is a silent no-op (verified live). answer() must detect that
// and reload + re-navigate to the same question hash before attempting the
// next letter, instead of quietly resubmitting whatever locked in first.
function makeErrorAnalysisDriver({ startLocked }) {
  const calls = { refreshed: 0, hashSet: [] };
  let locked = startLocked;
  let hash = '#/question-8';

  const driver = {
    switchTo() {
      return { defaultContent: async () => {}, frame: async () => {} };
    },
    async findElements() {
      return [{}]; // one fake iframe, so getCurrentQuestionDom treats it as present
    },
    navigate() {
      return {
        refresh: async () => {
          calls.refreshed += 1;
          locked = false; // a fresh load always starts unlocked
        },
      };
    },
    async sleep() {},
    async executeScript(code, ...args) {
      if (code.includes('ltiForm')) return false; // launcher never present, always "ready"
      if (code.includes('window.location.hash') && code.includes('return')) return hash;
      if (code.includes('window.location.hash = arguments[0]')) {
        hash = args[0];
        calls.hashSet.push(args[0]);
        return null;
      }
      if (code.includes('document.documentElement.outerHTML')) return '<html></html>';
      if (code.includes('every((i) => i.disabled)')) return locked;
      if (code.includes('const indexes = new Set(arguments[0])')) {
        if (locked) return true; // click() on a disabled input still "succeeds" but does nothing
        locked = false;
        return true;
      }
      if (code.includes('quiz-submit-btn')) return null;
      return null;
    },
  };
  return { driver, calls, isLocked: () => locked };
}

test('answer clicks directly when the radio group is not locked', async () => {
  const { driver, calls } = makeErrorAnalysisDriver({ startLocked: false });
  await errorAnalysis.answer(driver, { answer: 'B' });
  assert.equal(calls.refreshed, 0);
  assert.deepEqual(calls.hashSet, []);
});

test('answer selects every requested letter and clears previously checked ones', async () => {
  const clickLog = [];
  const inputs = [
    { checked: true, click() { this.checked = !this.checked; clickLog.push('A'); } },
    { checked: false, click() { this.checked = !this.checked; clickLog.push('B'); } },
    { checked: false, click() { this.checked = !this.checked; clickLog.push('C'); } },
  ];
  const driver = {
    switchTo() { return { defaultContent: async () => {}, frame: async () => {} }; },
    async sleep() {},
    async executeScript(code, arg) {
      if (code.includes('every((i) => i.disabled)')) return false;
      if (code.includes('const indexes = new Set(arguments[0])')) {
        const indexes = new Set(arg);
        inputs.forEach((input, i) => {
          const shouldBeChecked = indexes.has(i);
          if (input.checked !== shouldBeChecked) input.click();
        });
        return [...indexes].every((index) => inputs[index]);
      }
      if (code.includes('quiz-submit-btn')) return null;
      return null;
    },
  };

  await errorAnalysis.answer(driver, { answers: ['B', 'C'] });
  assert.deepEqual(inputs.map((input) => input.checked), [false, true, true]);
  assert.deepEqual(clickLog, ['A', 'B', 'C']);
});

// The actual bug this fixes: right after opening a fresh activity, the
// radios aren't rendered yet on the very first attempt — a bare one-shot
// check threw immediately, and the brute-forcer's candidate index still
// advances on any failure (it never retries the same letter), permanently
// skipping whichever letter happened to be first. Verified live: letter A
// was the genuinely correct answer, got silently skipped this way, and the
// question exhausted B-E (all wrong) with no way back to A.
test('answer polls for the radio group to render instead of throwing on the first not-ready check', async () => {
  let clickAttempts = 0;
  const driver = {
    switchTo() { return { defaultContent: async () => {}, frame: async () => {} }; },
    async sleep() {},
    async executeScript(code) {
      if (code.includes("some((b) => !b.disabled)")) return false;
      if (code.includes('every((i) => i.disabled)')) return false; // not locked
      if (code.includes('const indexes = new Set(arguments[0])')) {
        clickAttempts += 1;
        return clickAttempts >= 3; // not rendered yet for the first two checks
      }
      return null;
    },
  };

  await errorAnalysis.answer(driver, { answer: 'A' }); // should not throw
  assert.equal(clickAttempts, 3);
});

test('answer reloads and re-navigates to the same hash when the radio group is already locked', async () => {
  const { driver, calls } = makeErrorAnalysisDriver({ startLocked: true });
  await errorAnalysis.answer(driver, { answer: 'C' });
  assert.equal(calls.refreshed, 1);
  assert.deepEqual(calls.hashSet, ['#/question-8']);
});

// Same unreliability already found and fixed for fillInBlank/quizMatching:
// the generic .correct.timeout/.incorrect.timeout markers don't reliably
// reflect this quiz type's actual result either — verified live, a
// genuinely correct answer still timed out waiting for either banner.
test('checkResult resolves correct once .quiz-next-btn becomes visible', async () => {
  let calls = 0;
  const driver = {
    async executeScript(code) {
      calls += 1;
      if (code.includes('quiz-next-btn')) return calls >= 3;
      return null;
    },
    async sleep() {},
  };

  const result = await errorAnalysis.checkResult({ driver });
  assert.equal(result, 'correct');
});

// checkResult polls a real 5s wall-clock deadline (matches quizMatching's
// and fillInBlank's own checkResult), which outlives bun's default 5s
// per-test timeout; this test alone needs a longer explicit timeout, not a
// production change.
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

  const result = await errorAnalysis.checkResult({ driver });
  assert.equal(result, 'incorrect');
  assert.equal(submitClicks, 1);
});
