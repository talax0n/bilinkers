# Checkpoint Orchestrator Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Drive one already-open Beelingua checkpoint gate page (30 questions, passing score 100, unlimited attempts) through to a passing result, retrying the whole attempt (capped) when the score falls short.

**Architecture:** A new `scripts/run-checkpoint.js` orchestrator wraps `run-exercise.js`'s existing question-answering loop (reused unmodified via its `run(existingDriver)` export) in a gate-click → run → check-score → retry cycle. `run-exercise.js` gets one small addition: its result-screen handler now parses and returns the numeric score instead of discarding it. `cli.js` gains a `checkpoint` mode plus auto-detection.

**Tech Stack:** Bun, `node:test` + `node:assert/strict` (via `bun test`), `selenium-webdriver`, CommonJS (matches the whole repo — no new dependencies).

## Global Constraints

- Bun `>=1.3.0` (from `package.json` `engines`) — no Node-only APIs.
- CommonJS modules (`require`/`module.exports`), matching every existing file in `src/` and `scripts/`.
- Tests run via `bun test tests/*.test.js`, written with `node:test` (`test(...)`) and `node:assert/strict`, one file per source module, driver objects faked inline (no mocking library) — matches every existing file under `tests/`.
- No new npm dependencies.
- Safety cap of 5 attempts on the checkpoint retry loop (not the platform's literal "Unlimited") — per the approved design (`docs/superpowers/specs/2026-07-22-checkpoint-orchestrator-design.md`).
- An unhandled question type stops the whole checkpoint run immediately — it must never count against or trigger the retry loop.

---

## File Structure

- **Modify `scripts/run-exercise.js`** — `submitIfPresent()` returns `{ submitted, score }` instead of a bare boolean; `score` is the number parsed off the result screen (`undefined` when no result screen appeared). `main()`'s returned object gains a `score` field. `submitIfPresent` is exported alongside `run` so it's unit-testable without a real browser.
- **Create `scripts/run-checkpoint.js`** — new orchestrator: attaches to the browser, clicks the gate's `Start Attempt N`/`Continue` button, calls `run-exercise.js`'s `run()` on the same driver, checks the returned score, retries from the gate URL up to 5 times, stops immediately on `status: 'unhandled'`.
- **Modify `scripts/cli.js`** — add `checkpoint` to `MODES`; add a `checkpoint` branch to `detectMode()`.
- **Modify `package.json`** — add a `"checkpoint": "bun scripts/cli.js checkpoint"` script.
- **Modify `README.md`** — document the new mode, matching how `unit` was documented in `a191b0a`.
- **Create `tests/run-exercise-submit.test.js`** — unit tests for the score-parsing change to `submitIfPresent`.
- **Create `tests/run-checkpoint.test.js`** — unit tests for the attempt loop (gate matching, retry-on-low-score, stop-on-unhandled, attempt cap), all against a faked `driver` and an injected fake `runExerciseFn`.

---

## Task 1: Parse the score out of `run-exercise.js`'s result screen

**Files:**
- Modify: `scripts/run-exercise.js:68-106` (`submitIfPresent`), `:164-172` (`main`'s finishing branch), `:183` (`module.exports`)
- Test: `tests/run-exercise-submit.test.js`

**Interfaces:**
- Produces: `submitIfPresent(driver): Promise<{ submitted: boolean, score?: number }>` (exported), `run(existingDriver?): Promise<{ status: 'complete'|'unhandled', questionNum: number, score?: number }>` (already exported as `run`, gains `score`).

- [ ] **Step 1: Write the failing tests**

Create `tests/run-exercise-submit.test.js`:

```js
const assert = require('node:assert/strict');
const { test } = require('node:test');
const { submitIfPresent } = require('../scripts/run-exercise');

function makeDriver(script) {
  return {
    async executeScript(code) { return script(code); },
    async sleep() {},
  };
}

test('submitIfPresent resolves { submitted: false } when there is nothing to submit', async () => {
  const driver = makeDriver(() => false);
  const result = await submitIfPresent(driver);
  assert.deepEqual(result, { submitted: false });
});

test('submitIfPresent parses the score off the result screen before/while clicking Next', async () => {
  const driver = makeDriver((code) => {
    if (code.includes("=== 'Submit'")) return true;
    if (code.includes("=== 'Yes'")) return true;
    if (code.includes('Your Score')) return { score: 100, clicked: true };
    return false;
  });
  const result = await submitIfPresent(driver);
  assert.deepEqual(result, { submitted: true, score: 100 });
});

test('submitIfPresent keeps polling until the result screen (and its Next button) appear', async () => {
  let pollCount = 0;
  const driver = makeDriver((code) => {
    if (code.includes("=== 'Submit'")) return true;
    if (code.includes("=== 'Yes'")) return true;
    if (code.includes('Your Score')) {
      pollCount += 1;
      if (pollCount < 3) return { score: null, clicked: false };
      return { score: 87, clicked: true };
    }
    return false;
  });
  const result = await submitIfPresent(driver);
  assert.deepEqual(result, { submitted: true, score: 87 });
  assert.equal(pollCount, 3);
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `bun test tests/run-exercise-submit.test.js`
Expected: FAIL — `submitIfPresent` is not exported from `scripts/run-exercise.js` (`TypeError: submitIfPresent is not a function` or similar).

- [ ] **Step 3: Replace `submitIfPresent` and update its callers**

In `scripts/run-exercise.js`, replace the whole `submitIfPresent` function (currently lines 68-106) with:

```js
async function submitIfPresent(driver) {
  const submitted = await driver.executeScript(`
    const buttons = Array.from(document.querySelectorAll('button'));
    const btn = buttons.find((b) => b.textContent.trim() === 'Submit');
    if (btn && !btn.disabled) { btn.click(); return true; }
    return false;
  `);
  if (!submitted) return { submitted: false };

  await driver.sleep(1000 + Math.random() * 500);

  await driver.executeScript(`
    const buttons = Array.from(document.querySelectorAll('button'));
    const btn = buttons.find((b) => b.textContent.trim() === 'Yes');
    if (btn && !btn.disabled) { btn.click(); return true; }
    return false;
  `);
  await driver.sleep(1500);

  // The result screen ("Excellent! You Passed! Your Score: 100") takes a
  // beat longer than a fixed sleep to render (score/confetti animation) — a
  // single immediate click attempt right after can miss the "Next" button
  // entirely. Read the score and click Next in the same executeScript call
  // each poll so the score text is captured from the same render pass that
  // triggers the click, instead of racing a separate read against Next
  // navigating away.
  const deadline = Date.now() + 8000;
  let score;
  while (Date.now() < deadline) {
    const result = await driver.executeScript(`
      const scoreMatch = document.body.textContent.match(/Your Score:\\s*(\\d+)/);
      const buttons = Array.from(document.querySelectorAll('button'));
      const btn = buttons.find((b) => b.textContent.trim() === 'Next');
      const clicked = Boolean(btn && !btn.disabled);
      if (clicked) btn.click();
      return { score: scoreMatch ? Number(scoreMatch[1]) : null, clicked };
    `);
    if (result.score !== null && score === undefined) score = result.score;
    if (result.clicked) break;
    await driver.sleep(300);
  }
  await driver.sleep(1500);

  return { submitted: true, score };
}
```

Then update `main()`'s finishing branch (currently):

```js
    let advanced = await goToNextQuestion(driver);
    if (!advanced) {
      advanced = await clickPillNumber(driver, questionNum + 1);
    }
    if (!advanced) {
      const submitted = await submitIfPresent(driver);
      logger.info(submitted ? 'Submitted the exercise' : 'No further "Next" button or pill — exercise complete', { questionNum });
      return { status: 'complete', questionNum };
    }
```

to:

```js
    let advanced = await goToNextQuestion(driver);
    if (!advanced) {
      advanced = await clickPillNumber(driver, questionNum + 1);
    }
    if (!advanced) {
      const submitResult = await submitIfPresent(driver);
      logger.info(submitResult.submitted ? 'Submitted the exercise' : 'No further "Next" button or pill — exercise complete', { questionNum });
      return { status: 'complete', questionNum, score: submitResult.score };
    }
```

Finally, update the export at the bottom of the file:

```js
module.exports = { run: main, submitIfPresent };
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `bun test tests/run-exercise-submit.test.js`
Expected: PASS (3 tests).

- [ ] **Step 5: Run the full test suite to check nothing else broke**

Run: `bun test tests/*.test.js`
Expected: PASS (all existing tests still green — this change is additive to `submitIfPresent`'s return shape and callers).

- [ ] **Step 6: Commit**

```bash
git add scripts/run-exercise.js tests/run-exercise-submit.test.js
git commit -m "$(cat <<'EOF'
feat: parse the score off the exercise result screen

submitIfPresent used to click past "Excellent! You Passed! Your
Score: X" without reading it, since run-unit.js never needed the
number. The checkpoint orchestrator does need it, to decide whether
an attempt passed. Score and the Next-button click are read in the
same executeScript call each poll so the text is captured from the
same render pass that triggers the click, rather than racing a
separate read against Next navigating away.
EOF
)"
```

---

## Task 2: `scripts/run-checkpoint.js` — the attempt loop

**Files:**
- Create: `scripts/run-checkpoint.js`
- Test: `tests/run-checkpoint.test.js`

**Interfaces:**
- Consumes: `run-exercise.js`'s `run(existingDriver): Promise<{ status, questionNum, score? }>` (Task 1's shape) — injected as `runExerciseFn`, defaulting to the real one.
- Produces: `module.exports = { run: main }`, where `main(existingDriver?, { runExerciseFn } = {})` resolves one of:
  - `{ status: 'passed', attempt: number, score: 100 }`
  - `{ status: 'unhandled', attempt: number, questionNum: number }`
  - `{ status: 'exhausted', attempts: number }`
  - `{ status: 'no-gate', attempt: number }` (gate button never found)

- [ ] **Step 1: Write the failing tests**

Create `tests/run-checkpoint.test.js`:

```js
const assert = require('node:assert/strict');
const { test } = require('node:test');
const { run: runCheckpoint } = require('../scripts/run-checkpoint');

function makeDriver({ gateClickable = true } = {}) {
  const urls = ['https://lms.binus.ac.id/checkpoint-gate'];
  return {
    switchTo() { return { defaultContent: async () => {} }; },
    async getCurrentUrl() { return urls[urls.length - 1]; },
    async get(url) { urls.push(url); },
    async sleep() {},
    async executeScript(code) {
      if (code.includes('Start Attempt') && !gateClickable) return false;
      if (code.includes('Start Attempt')) return true;
      return false;
    },
  };
}

test('passes on the first attempt when the score is 100', async () => {
  const driver = makeDriver();
  const runExerciseFn = async () => ({ status: 'complete', questionNum: 30, score: 100 });
  const result = await runCheckpoint(driver, { runExerciseFn });
  assert.deepEqual(result, { status: 'passed', attempt: 1, score: 100 });
});

test('retries from the gate when the score is below 100, then passes', async () => {
  const driver = makeDriver();
  let call = 0;
  const runExerciseFn = async () => {
    call += 1;
    return call === 1 ? { status: 'complete', questionNum: 30, score: 83 } : { status: 'complete', questionNum: 30, score: 100 };
  };
  const result = await runCheckpoint(driver, { runExerciseFn });
  assert.deepEqual(result, { status: 'passed', attempt: 2, score: 100 });
});

test('stops immediately on an unhandled question type, without retrying', async () => {
  const driver = makeDriver();
  let calls = 0;
  const runExerciseFn = async () => {
    calls += 1;
    return { status: 'unhandled', questionNum: 7 };
  };
  const result = await runCheckpoint(driver, { runExerciseFn });
  assert.deepEqual(result, { status: 'unhandled', attempt: 1, questionNum: 7 });
  assert.equal(calls, 1);
});

test('gives up after 5 attempts without reaching 100', async () => {
  const driver = makeDriver();
  let calls = 0;
  const runExerciseFn = async () => {
    calls += 1;
    return { status: 'complete', questionNum: 30, score: 90 };
  };
  const result = await runCheckpoint(driver, { runExerciseFn });
  assert.deepEqual(result, { status: 'exhausted', attempts: 5 });
  assert.equal(calls, 5);
});

test('stops when the gate button is never found', async () => {
  const driver = makeDriver({ gateClickable: false });
  const runExerciseFn = async () => { throw new Error('should not be called'); };
  const result = await runCheckpoint(driver, { runExerciseFn });
  assert.deepEqual(result, { status: 'no-gate', attempt: 1 });
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `bun test tests/run-checkpoint.test.js`
Expected: FAIL — `Cannot find module '../scripts/run-checkpoint'`.

- [ ] **Step 3: Write `scripts/run-checkpoint.js`**

```js
const { Builder } = require('selenium-webdriver');
const chrome = require('selenium-webdriver/chrome');
const { resolveChromedriverPath } = require('../src/chromedriver');
const logger = require('../src/logger');
const { run: runExercise } = require('./run-exercise');

// This script attaches to an already-running Chromium-based browser (Chrome,
// Brave, Edge) launched with --remote-debugging-port=9222, and drives a
// checkpoint (see the "ENG-B1.2 ... Checkpoint 1" gate screenshot: Total
// Question 30, Passing Score 100, Maximum Attempt Unlimited) end to end:
// click the gate's Start/Continue button, run run-exercise.js's question
// loop on the same browser session, check the resulting score, and — since
// the platform allows unlimited attempts — retry the whole checkpoint from
// the gate if the score falls short of 100, up to a safety cap. Run via
// `node scripts/cli.js checkpoint`, or invoke this file directly if the
// browser is already up and sitting on a checkpoint's gate page.
// CHROMEDRIVER_PATH overrides auto-detection below (matches chromedriver to
// whatever's actually listening on the debug port) — only needed if that
// fails for your setup.
const CHROMEDRIVER_PATH = process.env.CHROMEDRIVER_PATH;

// A safety cap, not the platform's actual "Unlimited" policy — without one,
// a structural problem (not just an unlucky guess) would retry forever.
const MAX_ATTEMPTS = 5;

async function attachToBrave() {
  const options = new chrome.Options();
  options.debuggerAddress('localhost:9222');
  const chromedriverPath = CHROMEDRIVER_PATH || (await resolveChromedriverPath());
  const builder = new Builder().forBrowser('chrome').setChromeOptions(options).setChromeService(new chrome.ServiceBuilder(chromedriverPath));
  const driver = await builder.build();

  const handles = await driver.getAllWindowHandles();
  for (const handle of handles) {
    await driver.switchTo().window(handle);
    const url = await driver.getCurrentUrl();
    if (url.includes('lms.binus.ac.id')) return driver;
  }
  throw new Error('No lms.binus.ac.id tab found among open Brave windows.');
}

// The gate button's text isn't fixed: "Start Attempt 1" on a fresh
// checkpoint, "Start Attempt N" on a later one, or "Continue" if an attempt
// is already in progress (verified live — screenshot showed a Continue
// button distinct from Start Attempt N) — matched by prefix/exact text
// rather than one literal string.
async function clickGateButton(driver, timeoutMs = 8000) {
  await driver.switchTo().defaultContent();
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const clicked = await driver.executeScript(`
      const buttons = Array.from(document.querySelectorAll('button'));
      const btn = buttons.find((b) => {
        const text = b.textContent.trim();
        return text.startsWith('Start Attempt') || text === 'Continue';
      });
      if (btn && !btn.disabled) { btn.click(); return true; }
      return false;
    `);
    if (clicked) return true;
    await driver.sleep(300);
  }
  return false;
}

async function main(existingDriver, { runExerciseFn = runExercise } = {}) {
  const driver = existingDriver || (await attachToBrave());
  const gateUrl = await driver.getCurrentUrl();

  for (let attempt = 1; attempt <= MAX_ATTEMPTS; attempt += 1) {
    logger.info('Starting checkpoint attempt', { attempt });

    const clicked = await clickGateButton(driver);
    if (!clicked) {
      logger.warn('Could not find a Start Attempt/Continue button on the gate page — stopping', { attempt });
      return { status: 'no-gate', attempt };
    }
    await driver.sleep(1500);

    const result = await runExerciseFn(driver);
    logger.info('Checkpoint attempt finished', { attempt, ...result });

    if (result.status === 'unhandled') {
      // A structural gap, not a wrong guess — retrying hits the same
      // unhandled question type again, so stop right away instead of
      // burning attempts against the cap.
      logger.warn('Unhandled question type inside checkpoint — stopping', { attempt, questionNum: result.questionNum });
      return { status: 'unhandled', attempt, questionNum: result.questionNum };
    }

    if (result.score === 100) {
      logger.info('Checkpoint passed', { attempt, score: result.score });
      return { status: 'passed', attempt, score: result.score };
    }

    logger.warn('Checkpoint attempt did not reach a passing score — retrying', { attempt, score: result.score });
    await driver.switchTo().defaultContent();
    await driver.get(gateUrl);
    await driver.sleep(1500);
  }

  logger.warn('Checkpoint not passed after max attempts', { attempts: MAX_ATTEMPTS });
  return { status: 'exhausted', attempts: MAX_ATTEMPTS };
}

if (require.main === module) {
  main().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}

module.exports = { run: main };
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `bun test tests/run-checkpoint.test.js`
Expected: PASS (5 tests).

- [ ] **Step 5: Run the full test suite**

Run: `bun test tests/*.test.js`
Expected: PASS (all tests, existing + new).

- [ ] **Step 6: Commit**

```bash
git add scripts/run-checkpoint.js tests/run-checkpoint.test.js
git commit -m "$(cat <<'EOF'
feat: add a checkpoint orchestrator

Drives one already-open checkpoint gate (30 questions, passing score
100, unlimited attempts) by reusing run-exercise.js's question loop
unmodified — a checkpoint is just a bigger native exercise entered
through a differently-labeled gate ("Start Attempt N" / "Continue"
rather than "Start"/"Continue"). Since the platform allows unlimited
attempts, retries the whole checkpoint from the gate when the score
falls short of 100, capped at 5 attempts so a structural problem
(e.g. an unhandled question type) can't loop forever — an unhandled
type stops immediately instead, since retrying would hit the same
wall.
EOF
)"
```

---

## Task 3: Wire `checkpoint` into `cli.js`, `package.json`, and the README

**Files:**
- Modify: `scripts/cli.js:33-37` (`MODES`), `:96-142` (`detectMode`)
- Modify: `package.json:8-14` (`scripts`)
- Modify: `README.md` (wherever `unit`/`exercise`/`iframe` modes are documented — follow the existing structure)

**Interfaces:**
- Consumes: `scripts/run-checkpoint.js`'s `module.exports = { run }` (Task 2).
- Produces: `node scripts/cli.js checkpoint` and `bun run checkpoint` as new entry points; `detectMode()` returns `'checkpoint'` for a checkpoint gate page.

- [ ] **Step 1: Add `checkpoint` to `MODES`**

In `scripts/cli.js`, change:

```js
const MODES = {
  exercise: '../scripts/run-exercise.js',
  unit: '../scripts/run-unit.js',
  iframe: '../scripts/run-iframe-exercise.js',
};
```

to:

```js
const MODES = {
  exercise: '../scripts/run-exercise.js',
  unit: '../scripts/run-unit.js',
  iframe: '../scripts/run-iframe-exercise.js',
  checkpoint: '../scripts/run-checkpoint.js',
};
```

- [ ] **Step 2: Add a `checkpoint` branch to `detectMode()`**

In `scripts/cli.js`, inside `detectMode()`, the current body (after the iframe check, before the `exercise` lettered-option check) is:

```js
    const topHtml = await driver.executeScript('return document.documentElement.outerHTML');
    if (topHtml.includes('bl-w-full justify-content-start')) {
      return 'exercise';
    }
```

Change it to check for a checkpoint gate first, since a checkpoint's gate page has the exact same `Total Question`/`Passing Score`/`Start or Continue` shape as a plain exercise's `BlExercise` gate — the one thing that's different is the "Checkpoint" wording in its heading, so that has to be the discriminator, not the generic gate markers:

```js
    const topHtml = await driver.executeScript('return document.documentElement.outerHTML');

    // A checkpoint's gate page ("ENG-B1.2 - INDEPENDENT (V2) - Checkpoint 1",
    // Total Question/Passing Score/Maximum Attempt, a "Start Attempt N" or
    // "Continue" button) has the exact same shape as a plain exercise's
    // BlExercise gate — "Checkpoint" in the heading is the one thing that
    // tells them apart, so that's the check, not the generic gate markers.
    const isCheckpointGate = await driver.executeScript(`
      const hasCheckpointHeading = /Checkpoint/i.test(document.body.textContent);
      const buttons = Array.from(document.querySelectorAll('button'));
      const hasGateButton = buttons.some((b) => {
        const text = b.textContent.trim();
        return text.startsWith('Start Attempt') || text === 'Continue';
      });
      return hasCheckpointHeading && hasGateButton;
    `);
    if (isCheckpointGate) {
      return 'checkpoint';
    }

    if (topHtml.includes('bl-w-full justify-content-start')) {
      return 'exercise';
    }
```

- [ ] **Step 3: Update the usage comment at the top of `cli.js`**

Change:

```js
// One-command entry point: launches your Chromium-based browser with remote
// debugging on, waits for you to log in and open the exercise, then runs the
// matching bot script. Usage: node scripts/cli.js [exercise|iframe|unit] [url]
```

to:

```js
// One-command entry point: launches your Chromium-based browser with remote
// debugging on, waits for you to log in and open the exercise, then runs the
// matching bot script. Usage: node scripts/cli.js [exercise|iframe|unit|checkpoint] [url]
```

- [ ] **Step 4: Add the `checkpoint` npm script**

In `package.json`, change:

```json
    "exercise": "bun scripts/cli.js exercise",
    "iframe": "bun scripts/cli.js iframe",
    "unit": "bun scripts/cli.js unit"
```

to:

```json
    "exercise": "bun scripts/cli.js exercise",
    "iframe": "bun scripts/cli.js iframe",
    "unit": "bun scripts/cli.js unit",
    "checkpoint": "bun scripts/cli.js checkpoint"
```

- [ ] **Step 5: Document the mode in the README**

Open `README.md`, find where the `unit` mode is documented (added in `a191b0a`), and add a matching entry for `checkpoint`: what it's for (a 30-question, pass-100, unlimited-attempt checkpoint gate between units), that it's entered by opening the checkpoint's gate page and running `npm run checkpoint` (or letting `npm run bot` auto-detect it), and that it retries the whole checkpoint up to 5 times if a score under 100 comes back, stopping immediately instead on an unhandled question type. Match the surrounding section's heading level and tone exactly — read the file first to place it consistently rather than guessing the structure.

- [ ] **Step 6: Run the full test suite**

Run: `bun test tests/*.test.js`
Expected: PASS (`cli.js` has no dedicated test file — matches the existing convention noted in the prior session's handoff — so this just confirms Tasks 1-2's tests and everything else are still green).

- [ ] **Step 7: Commit**

```bash
git add scripts/cli.js package.json README.md
git commit -m "$(cat <<'EOF'
feat: wire the checkpoint orchestrator into cli.js

Adds checkpoint to MODES (npm run checkpoint) and to detectMode()'s
auto-detection. A checkpoint's gate page has the exact same
Total Question/Passing Score/Maximum Attempt shape as a plain
exercise's BlExercise gate, so "Checkpoint" in the page heading is
the discriminator, not the generic gate markers.
EOF
)"
```

---

## Task 4: Live verification against a real checkpoint

This task has no code changes — it's the same "verify live before trusting it" step every prior orchestrator change in this repo has gone through (per the handoff's "Key gotchas": DOM assumptions here — the gate button text, the result-screen score format, the "Checkpoint" heading text — are based on the screenshots discussed during design, not yet driven against the real page).

**Files:** none (manual verification only).

- [ ] **Step 1: Open a real checkpoint's gate page**

In the Brave session already attached on port 9222 (or via `npm run bot` to launch fresh), navigate to a checkpoint node from the course map (e.g. the "C1.1" hexagon from the screenshot) so the gate page ("... Checkpoint 1", Total Question 30, Passing Score 100, Start Attempt 1) is on screen.

- [ ] **Step 2: Run the checkpoint orchestrator**

Run: `npm run checkpoint`

Watch the dashboard/log output. Confirm:
- The gate's "Start Attempt 1" button gets clicked and the quiz loads.
- Questions are answered and advanced through (pill nav / Submit-Yes-Next) the same way a native exercise runs.
- The result screen's score is logged correctly, matching what's shown on screen.
- If the score is below 100, the run navigates back to the gate and clicks "Start Attempt 2" (not "Start Attempt 1" again) or "Continue" — confirm the gate-button matcher works against whatever the real second-attempt button says, since this is the one piece of DOM text not yet seen live.

- [ ] **Step 3: Fix any live-only surprises inline**

If the real DOM differs from what the screenshots suggested (e.g. the result screen's score text isn't exactly "Your Score: X", or the checkpoint heading doesn't literally contain "Checkpoint"), adjust `submitIfPresent`'s regex, `clickGateButton`'s matcher, or `detectMode`'s `isCheckpointGate` check accordingly, add/update the corresponding unit test from Tasks 1-3, rerun `bun test tests/*.test.js`, and commit the fix with a message explaining what was verified live and why the assumption needed correcting (matching the existing commit style throughout this repo's history).

- [ ] **Step 4: Confirm `npm run bot` auto-detection works end-to-end**

With the browser back on a checkpoint's gate page, run `npm run bot`, press Enter, and confirm the console prints `Detected: checkpoint` before the orchestrator starts — this is the one path (auto-detect) not exercised by the unit tests in Task 3, since `cli.js` has no test file.

---

## Self-Review Notes

- **Spec coverage:** entry point (Task 2 `clickGateButton`), question flow reuse (Task 2 consumes `run-exercise.js`'s `run`), score extraction (Task 1), attempt loop + cap + unhandled short-circuit (Task 2), cli.js wiring incl. the "Checkpoint"-heading discriminator (Task 3), live verification given the design's explicitly-unverified DOM assumptions (Task 4). All spec sections covered.
- **Placeholder scan:** none found — every step has complete, runnable code or an exact command with expected output.
- **Type consistency:** `submitIfPresent` → `{ submitted, score? }` (Task 1) matches what Task 2's tests feed as `runExerciseFn`'s resolved `score` field; `run-checkpoint.js`'s `main(existingDriver?, { runExerciseFn } = {})` signature matches its test file's calls in Task 2; `detectMode()`'s new `'checkpoint'` return value matches the `MODES.checkpoint` key added in the same task.
