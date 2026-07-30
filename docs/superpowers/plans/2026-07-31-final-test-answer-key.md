# Final Test Answer-Key Scraper Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `final-test` bot mode that reads every question on an already-open Final Test attempt (50 questions, 1 attempt, pill-nav native quiz) and writes a Markdown answer key — without ever clicking an option, Save, or Submit.

**Architecture:** New `scripts/run-final-test.js` reuses `run-checkpoint.js`'s already-solved pill helpers (`ensurePillsExpanded`, `readPills`, `clickPill`) and the existing `readingComprehension`/`audioMultipleChoice` question-type parsers unmodified. For each pill it parses the question, skips `audioMultipleChoice` (listening — can't be answered without hearing it), and for everything else calls `answerQuestion()` from `src/llm.js` once (no feedback loop — there's no per-question feedback and only one real attempt exists) to get a single best-guess answer. Results are collected into a Markdown file, never fed back into the DOM.

**Tech Stack:** Bun, `node:test` + `node:assert/strict` (via `bun test`), `selenium-webdriver`, CommonJS — matches every existing file in `src/`/`scripts/`.

## Global Constraints

- Bun `>=1.3.0` (from `package.json` `engines`) — no Node-only APIs.
- CommonJS modules (`require`/`module.exports`), matching every existing file under `src/`/`scripts/`.
- Tests run via `bun test tests/*.test.js`, written with `node:test` (`test(...)`) and `node:assert/strict`, one file per source module, driver/client objects faked inline (no mocking library) — matches every existing file under `tests/`.
- No new npm dependencies.
- This script must never call `handler.answer()`, click Save, or click Submit — read-only, per the approved design (`docs/superpowers/specs/2026-07-31-final-test-answer-key-design.md`), since the Final Test has exactly 1 attempt.
- One question's LLM failure must not abort the other 49 — record `NEEDS REVIEW` and continue.

---

## File Structure

- **Modify `scripts/run-checkpoint.js`** — export `readPills` and `clickPill` alongside the already-exported `ensurePillsExpanded` (both exist in the file today but aren't in `module.exports`), so `run-final-test.js` can reuse them instead of duplicating pill-navigation logic.
- **Create `scripts/run-final-test.js`** — new read-only scraper: attaches to the browser, loops every pill, parses+guesses each question, writes `final-test-answers.md`.
- **Modify `scripts/cli.js`** — add `final-test` to `MODES` (no `detectMode()` branch — explicitly invoked only, like `checkpoint-capture`).
- **Modify `package.json`** — add `"final-test": "bun scripts/cli.js final-test"`.
- **Modify `.gitignore`** — add `final-test-answers.md` (scratch output, not committed).
- **Modify `README.md`** — document the new mode next to `checkpoint`/`course` in the modes list and project structure table.
- **Create `tests/run-final-test.test.js`** — unit tests for the pure formatting functions and the scrape loop, all against a faked `driver` and injected fake `answerQuestionFn`.

---

## Task 1: Export `readPills` and `clickPill` from `run-checkpoint.js`

**Files:**
- Modify: `scripts/run-checkpoint.js:628` (`module.exports`)
- Test: `tests/run-checkpoint.test.js` (append)

**Interfaces:**
- Produces: `readPills(driver): Promise<Array<{n: number}>>` (already implemented at `scripts/run-checkpoint.js`, just not exported), `clickPill(driver, n): Promise<boolean>` (already implemented, just not exported).

- [ ] **Step 1: Write the failing test**

Append to `tests/run-checkpoint.test.js`:

```js
test('readPills and clickPill are exported for reuse by other scripts', () => {
  const mod = require('../scripts/run-checkpoint');
  assert.equal(typeof mod.readPills, 'function');
  assert.equal(typeof mod.clickPill, 'function');
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `bun test tests/run-checkpoint.test.js -t "readPills and clickPill are exported"`
Expected: FAIL — `mod.readPills` is `undefined`, `typeof undefined === 'function'` is `false`.

- [ ] **Step 3: Add the exports**

In `scripts/run-checkpoint.js`, change the `module.exports` line (currently line 628):

```js
module.exports = { run: main, waitForScore, ensurePillsExpanded, attachToBrave, clickGateButton, clickSaveButton, waitForPillSaved, isPillLocked, clickSubmit, answerAllQuestions, readPills, clickPill };
```

- [ ] **Step 4: Run test to verify it passes**

Run: `bun test tests/run-checkpoint.test.js -t "readPills and clickPill are exported"`
Expected: PASS

- [ ] **Step 5: Run the full checkpoint test file to confirm nothing broke**

Run: `bun test tests/run-checkpoint.test.js`
Expected: All tests PASS (this change only adds two keys to an object literal).

- [ ] **Step 6: Commit**

```bash
git add scripts/run-checkpoint.js tests/run-checkpoint.test.js
git commit -m "refactor: export readPills/clickPill from run-checkpoint for reuse"
```

---

## Task 2: Pure formatting functions — question record + Markdown output

**Files:**
- Create: `scripts/run-final-test.js` (this task only adds the two pure functions + requires at the top; the scrape loop and `main()` come in Task 3)
- Test: `tests/run-final-test.test.js`

**Interfaces:**
- Produces: `formatQuestionRecord({ number, handlerName, questionData, answer }): { number, type, text, options, answer }` where `type` is `'mcq' | 'listening' | 'unhandled'`.
- Produces: `buildMarkdown(records: Array<ReturnType<typeof formatQuestionRecord>>): string`.

- [ ] **Step 1: Write the failing tests**

Create `tests/run-final-test.test.js`:

```js
const assert = require('node:assert/strict');
const { test } = require('node:test');
const { formatQuestionRecord, buildMarkdown } = require('../scripts/run-final-test');

test('formatQuestionRecord builds an mcq record from readingComprehension-shaped data', () => {
  const record = formatQuestionRecord({
    number: 3,
    handlerName: 'readingComprehension',
    questionData: { text: 'Which sentence uses the past perfect correctly?', options: [{ letter: 'A', text: 'She go.' }, { letter: 'B', text: 'She had gone.' }] },
    answer: 'B',
  });
  assert.deepEqual(record, {
    number: 3,
    type: 'mcq',
    text: 'Which sentence uses the past perfect correctly?',
    options: [{ letter: 'A', text: 'She go.' }, { letter: 'B', text: 'She had gone.' }],
    answer: 'B',
  });
});

test('formatQuestionRecord builds a listening record for audioMultipleChoice, no answer field needed', () => {
  const record = formatQuestionRecord({ number: 7, handlerName: 'audioMultipleChoice', questionData: null, answer: null });
  assert.deepEqual(record, { number: 7, type: 'listening' });
});

test('formatQuestionRecord builds an unhandled record when no handler matched', () => {
  const record = formatQuestionRecord({ number: 12, handlerName: null, questionData: null, answer: null });
  assert.deepEqual(record, { number: 12, type: 'unhandled' });
});

test('buildMarkdown renders an mcq question with its options and answer', () => {
  const md = buildMarkdown([
    { number: 3, type: 'mcq', text: 'Which sentence uses the past perfect correctly?', options: [{ letter: 'A', text: 'She go.' }, { letter: 'B', text: 'She had gone.' }], answer: 'B' },
  ]);
  assert.equal(
    md,
    '## Q3\nWhich sentence uses the past perfect correctly?\nA. She go.\nB. She had gone.\n**Jawaban: B**\n'
  );
});

test('buildMarkdown renders a listening question as a skip line', () => {
  const md = buildMarkdown([{ number: 7, type: 'listening' }]);
  assert.equal(md, '## Q7 (listening — skipped, jawab manual)\n');
});

test('buildMarkdown renders an unhandled question as a manual-review line', () => {
  const md = buildMarkdown([{ number: 12, type: 'unhandled' }]);
  assert.equal(md, '## Q12 (unhandled question type — jawab manual)\n');
});

test('buildMarkdown joins multiple questions with a blank line between them', () => {
  const md = buildMarkdown([
    { number: 1, type: 'mcq', text: 'Q one?', options: [{ letter: 'A', text: 'x' }], answer: 'A' },
    { number: 2, type: 'listening' },
  ]);
  assert.equal(md, '## Q1\nQ one?\nA. x\n**Jawaban: A**\n\n## Q2 (listening — skipped, jawab manual)\n');
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `bun test tests/run-final-test.test.js`
Expected: FAIL — `Cannot find module '../scripts/run-final-test'` (file doesn't exist yet).

- [ ] **Step 3: Write the minimal implementation**

Create `scripts/run-final-test.js`:

```js
function formatQuestionRecord({ number, handlerName, questionData, answer }) {
  if (handlerName === 'audioMultipleChoice') return { number, type: 'listening' };
  if (!handlerName) return { number, type: 'unhandled' };
  return { number, type: 'mcq', text: questionData.text, options: questionData.options, answer };
}

function buildMarkdown(records) {
  return records
    .map((record) => {
      if (record.type === 'listening') return `## Q${record.number} (listening — skipped, jawab manual)\n`;
      if (record.type === 'unhandled') return `## Q${record.number} (unhandled question type — jawab manual)\n`;
      const optionLines = record.options.map((opt) => `${opt.letter}. ${opt.text}`).join('\n');
      return `## Q${record.number}\n${record.text}\n${optionLines}\n**Jawaban: ${record.answer}**\n`;
    })
    .join('\n');
}

module.exports = { formatQuestionRecord, buildMarkdown };
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `bun test tests/run-final-test.test.js`
Expected: All 7 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add scripts/run-final-test.js tests/run-final-test.test.js
git commit -m "feat: add final-test answer-key formatting functions"
```

---

## Task 3: Scrape loop — parse every pill, guess via LLM, skip listening, retry-then-NEEDS-REVIEW on failure

**Files:**
- Modify: `scripts/run-final-test.js` (add requires, `answerWithRetry`, `scrapeFinalTest`, keep the Task 2 functions)
- Test: `tests/run-final-test.test.js` (append)

**Interfaces:**
- Consumes: `formatQuestionRecord`, `buildMarkdown` (Task 2); `readPills(driver)`, `clickPill(driver, n)` (Task 1, from `../scripts/run-checkpoint`); `getCurrentQuestionDom(driver, opts?)` (from `../src/browser`, existing); `createRegistry()` (from `../src/questionTypes/registry`, existing); `audioMultipleChoice`, `readingComprehension` type modules (existing, each exposing `.name`, `.detect(dom)`, `.parse(dom)`); `answerQuestion(client, model, instruction, questionData, feedback)` (from `../src/llm`, existing).
- Produces: `answerWithRetry(llmClient, model, questionData, opts?): Promise<string>` — returns the guessed letter, or the string `'NEEDS REVIEW'` after `maxAttempts` (default 3) failed calls. `scrapeFinalTest(driver, opts?): Promise<{ status: 'complete', records: Array } | { status: 'no-pills' }>`.

- [ ] **Step 1: Write the failing tests**

Append to `tests/run-final-test.test.js`:

```js
const { scrapeFinalTest, answerWithRetry } = require('../scripts/run-final-test');

function makeDom(handlerName, questionData) {
  return { handlerName, questionData };
}

function makeFakeRegistry(byPillHandler) {
  return () => ({
    findHandler(dom) {
      if (!dom.handlerName) return null;
      return { name: dom.handlerName, parse: () => dom.questionData };
    },
  });
}

test('answerWithRetry returns the LLM answer on the first success', async () => {
  const answerQuestionFn = async () => ({ answer: 'C' });
  const result = await answerWithRetry({}, 'model', { text: 'q', options: [] }, { answerQuestionFn });
  assert.equal(result, 'C');
});

test('answerWithRetry retries up to maxAttempts then returns NEEDS REVIEW', async () => {
  let calls = 0;
  const answerQuestionFn = async () => { calls += 1; throw new Error('boom'); };
  const result = await answerWithRetry({}, 'model', { text: 'q', options: [] }, { answerQuestionFn, maxAttempts: 3, sleepFn: async () => {} });
  assert.equal(result, 'NEEDS REVIEW');
  assert.equal(calls, 3);
});

test('answerWithRetry recovers if a later attempt succeeds', async () => {
  let calls = 0;
  const answerQuestionFn = async () => {
    calls += 1;
    if (calls < 2) throw new Error('boom');
    return { answer: 'D' };
  };
  const result = await answerWithRetry({}, 'model', { text: 'q', options: [] }, { answerQuestionFn, maxAttempts: 3, sleepFn: async () => {} });
  assert.equal(result, 'D');
  assert.equal(calls, 2);
});

test('scrapeFinalTest walks every pill, answers mcq questions, skips listening, records unhandled', async () => {
  const pillDoms = {
    1: makeDom('readingComprehension', { text: 'Q1?', options: [{ letter: 'A', text: 'x' }] }),
    2: makeDom('audioMultipleChoice', { instruction: 'listen', questionText: 'Q2?', options: [] }),
    3: makeDom(null, null),
  };
  const driver = {};
  const result = await scrapeFinalTest(driver, {
    readPillsFn: async () => [{ n: 1 }, { n: 2 }, { n: 3 }],
    clickPillFn: async () => true,
    getCurrentQuestionDomFn: async (_driver, n) => {},
    createRegistryFn: makeFakeRegistry(),
    answerQuestionFn: async () => ({ answer: 'A' }),
    // Override dom lookup directly since this fake registry keys off dom.handlerName,
    // not real DOM parsing — pillDomLookupFn simulates "current pill's dom" per call.
    pillDomLookupFn: (n) => pillDoms[n],
  });
  assert.equal(result.status, 'complete');
  assert.deepEqual(result.records, [
    { number: 1, type: 'mcq', text: 'Q1?', options: [{ letter: 'A', text: 'x' }], answer: 'A' },
    { number: 2, type: 'listening' },
    { number: 3, type: 'unhandled' },
  ]);
});

test('scrapeFinalTest returns no-pills status without calling the LLM when readPills is empty', async () => {
  let llmCalled = false;
  const driver = {};
  const result = await scrapeFinalTest(driver, {
    readPillsFn: async () => [],
    clickPillFn: async () => true,
    getCurrentQuestionDomFn: async () => {},
    createRegistryFn: makeFakeRegistry(),
    answerQuestionFn: async () => { llmCalled = true; return { answer: 'A' }; },
    pillDomLookupFn: () => null,
  });
  assert.deepEqual(result, { status: 'no-pills' });
  assert.equal(llmCalled, false);
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `bun test tests/run-final-test.test.js`
Expected: FAIL — `scrapeFinalTest`/`answerWithRetry` are not exported yet.

- [ ] **Step 3: Write the implementation**

Replace the full contents of `scripts/run-final-test.js` with:

```js
const { Builder } = require('selenium-webdriver');
const chrome = require('selenium-webdriver/chrome');
const fs = require('node:fs');
const { resolveChromedriverPath } = require('../src/chromedriver');
const { config } = require('../src/config');
const { getCurrentQuestionDom } = require('../src/browser');
const { createRegistry } = require('../src/questionTypes/registry');
const audioMultipleChoice = require('../src/questionTypes/audioMultipleChoice');
const readingComprehension = require('../src/questionTypes/readingComprehension');
const { createClient, answerQuestion } = require('../src/llm');
const { readPills, clickPill, ensurePillsExpanded } = require('./run-checkpoint');
const logger = require('../src/logger');

// This script attaches to an already-running Chromium-based browser sitting
// on an OPEN Final Test attempt (50 questions, 1 attempt, same pill-nav
// native quiz DOM as a checkpoint) and reads every question into a Markdown
// answer key. It never clicks an option, Save, or Submit — a Final Test has
// exactly one attempt, so there is no room to brute-force like
// run-checkpoint.js does. Listening questions (audioMultipleChoice) are
// skipped since the bot can't hear the audio. Run via
// `node scripts/cli.js final-test`, or invoke this file directly if the
// browser is already up and sitting on an open Final Test attempt.
const CHROMEDRIVER_PATH = process.env.CHROMEDRIVER_PATH;
const OUTPUT_PATH = './final-test-answers.md';

function formatQuestionRecord({ number, handlerName, questionData, answer }) {
  if (handlerName === 'audioMultipleChoice') return { number, type: 'listening' };
  if (!handlerName) return { number, type: 'unhandled' };
  return { number, type: 'mcq', text: questionData.text, options: questionData.options, answer };
}

function buildMarkdown(records) {
  return records
    .map((record) => {
      if (record.type === 'listening') return `## Q${record.number} (listening — skipped, jawab manual)\n`;
      if (record.type === 'unhandled') return `## Q${record.number} (unhandled question type — jawab manual)\n`;
      const optionLines = record.options.map((opt) => `${opt.letter}. ${opt.text}`).join('\n');
      return `## Q${record.number}\n${record.text}\n${optionLines}\n**Jawaban: ${record.answer}**\n`;
    })
    .join('\n');
}

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

// A single LLM guess per question — there's no per-question feedback to loop
// on (unlike run-exercise.js's retry-with-feedback) and only one real
// attempt exists, so retries here are purely for transient call failures
// (matches src/runner.js's callWithRetry), not for trying a different
// answer. Exhausting retries marks the question for manual review instead of
// throwing, so one bad question can't abort the other 49.
const ANSWER_INSTRUCTION =
  'Determine the single correct option letter for this question, using the passage/question text and options provided. Respond as JSON: { "answer": "<letter>" }.';

async function answerWithRetry(llmClient, model, questionData, { answerQuestionFn = answerQuestion, maxAttempts = 3, sleepFn = (ms) => new Promise((r) => setTimeout(r, ms)) } = {}) {
  let lastErr;
  for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
    try {
      const result = await answerQuestionFn(llmClient, model, ANSWER_INSTRUCTION, questionData, null);
      return result.answer;
    } catch (err) {
      lastErr = err;
      if (attempt < maxAttempts) await sleepFn(1000);
    }
  }
  logger.warn('Final test: LLM failed for a question after retries — marking for manual review', { error: lastErr.message });
  return 'NEEDS REVIEW';
}

// Walks every pill 1..N (N read live, not hardcoded), parsing and guessing
// each question. Read-only: only pill navigation touches the DOM, no
// option/Save/Submit click ever happens. pillDomLookupFn/getCurrentQuestionDomFn
// are separated (rather than one combined call) so tests can fake per-pill
// DOM content without a real driver.
async function scrapeFinalTest(
  driver,
  {
    llmClient,
    model,
    readPillsFn = readPills,
    clickPillFn = clickPill,
    ensurePillsExpandedFn = ensurePillsExpanded,
    getCurrentQuestionDomFn = getCurrentQuestionDom,
    createRegistryFn = createRegistry,
    answerQuestionFn = answerQuestion,
    pillDomLookupFn = null,
    logger: injectedLogger = logger,
  } = {}
) {
  const registry = createRegistryFn();
  registry.register(audioMultipleChoice);
  registry.register(readingComprehension);

  await ensurePillsExpandedFn(driver);
  const pills = await readPillsFn(driver);
  if (pills.length === 0) {
    injectedLogger.warn('Final test: no nav pills found — stopping', {});
    return { status: 'no-pills' };
  }

  const records = [];
  for (const { n } of pills) {
    await clickPillFn(driver, n);
    const dom = pillDomLookupFn ? pillDomLookupFn(n) : await getCurrentQuestionDomFn(driver, { iframeWaitMs: 600 });
    const handler = registry.findHandler(dom);

    if (!handler) {
      injectedLogger.warn('Final test: unhandled question type', { pill: n });
      records.push(formatQuestionRecord({ number: n, handlerName: null, questionData: null, answer: null }));
      continue;
    }

    if (handler.name === 'audioMultipleChoice') {
      injectedLogger.info('Final test: skipping listening question', { pill: n });
      records.push(formatQuestionRecord({ number: n, handlerName: handler.name, questionData: null, answer: null }));
      continue;
    }

    const questionData = handler.parse(dom);
    const answer = await answerWithRetry(llmClient, model, questionData, { answerQuestionFn });
    injectedLogger.info('Final test: question answered', { pill: n, answer });
    records.push(formatQuestionRecord({ number: n, handlerName: handler.name, questionData, answer }));
  }

  return { status: 'complete', records };
}

async function main(existingDriver) {
  const driver = existingDriver || (await attachToBrave());
  const llmClient = createClient(config);

  const result = await scrapeFinalTest(driver, { llmClient, model: config.openai.model });
  if (result.status === 'no-pills') {
    logger.warn('Final test: nothing to scrape — is the browser on an open Final Test attempt?', {});
    return result;
  }

  const markdown = buildMarkdown(result.records);
  fs.writeFileSync(OUTPUT_PATH, markdown);
  logger.info('Final test: answer key written', { path: OUTPUT_PATH, questions: result.records.length });
  return result;
}

if (require.main === module) {
  main().catch((err) => {
    logger.error('Final test scraper failed', { error: err.message });
    process.exitCode = 1;
  });
}

module.exports = { run: main, formatQuestionRecord, buildMarkdown, answerWithRetry, scrapeFinalTest };
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `bun test tests/run-final-test.test.js`
Expected: All 12 tests PASS (7 from Task 2 + 5 from this task).

- [ ] **Step 5: Run the full test suite to confirm nothing else broke**

Run: `bun test tests/*.test.js`
Expected: All tests PASS.

- [ ] **Step 6: Commit**

```bash
git add scripts/run-final-test.js tests/run-final-test.test.js
git commit -m "feat: scrape final test questions into an LLM-guessed answer key"
```

---

## Task 4: Wire the `final-test` CLI mode

**Files:**
- Modify: `scripts/cli.js` (`MODES` object)
- Modify: `package.json` (`scripts` object)
- Modify: `.gitignore`

**Interfaces:**
- Consumes: `scripts/run-final-test.js`'s `require.main === module` entry point (Task 3) — `cli.js` just needs the path, `main()` is invoked the same way every other mode's script self-invokes when run directly.

- [ ] **Step 1: Add the mode to `cli.js`**

In `scripts/cli.js`, in the `MODES` object (currently lines 34-43), add a new entry after `level`:

```js
const MODES = {
  exercise: '../scripts/run-exercise.js',
  unit: '../scripts/run-unit.js',
  iframe: '../scripts/run-iframe-exercise.js',
  reading: '../scripts/run-reading-exercise.js',
  checkpoint: '../scripts/run-checkpoint.js',
  'checkpoint-capture': '../scripts/run-checkpoint-capture.js',
  course: '../scripts/run-course.js',
  level: '../scripts/run-level.js',
  'final-test': '../scripts/run-final-test.js',
};
```

No `detectMode()` change — `final-test` is explicitly invoked only (`node scripts/cli.js final-test`), same as `checkpoint-capture`, never auto-detected.

- [ ] **Step 2: Add the npm script**

In `package.json`, in the `"scripts"` object (currently lines 6-17), add after `"level"`:

```json
    "level": "bun scripts/cli.js level",
    "final-test": "bun scripts/cli.js final-test"
```

(Keep valid JSON — add a trailing comma after the existing `"level"` line.)

- [ ] **Step 3: Ignore the scratch output file**

In `.gitignore`, add a new line:

```
final-test-answers.md
```

- [ ] **Step 4: Verify the mode resolves**

Run: `node -e "const {MODES}=require('./scripts/cli.js'); console.log(MODES)"` — this will actually execute `cli.js`'s top-level code (it launches a browser), so instead just visually confirm by reading the file, or run:

Run: `grep -n "final-test" scripts/cli.js package.json .gitignore`
Expected: three matches, one per file, each showing the new line.

- [ ] **Step 5: Run the full test suite**

Run: `bun test tests/*.test.js`
Expected: All tests PASS (this task touches no tested logic, only config/wiring).

- [ ] **Step 6: Commit**

```bash
git add scripts/cli.js package.json .gitignore
git commit -m "feat: wire up the final-test CLI mode"
```

---

## Task 5: Document the mode in README

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add it to the intro paragraph**

In `README.md` line 3, after the existing sentence about `npm run checkpoint`, add:

```
It can also scrape a Final Test's questions into a Markdown answer key (`npm run final-test`) — read-only, since a Final Test allows only 1 attempt, so the bot never clicks an option itself and leaves the actual answering to you.
```

- [ ] **Step 2: Add it to the "Running it" mode list**

Near `README.md` line 110-111 (the `npm run checkpoint` / `npm run course` bullet lines), add:

```
npm run final-test  # scrapes an open Final Test's questions into final-test-answers.md — never clicks an answer, Save, or Submit
```

- [ ] **Step 3: Add it to the project structure table**

Near `README.md` line 189 (the `run-checkpoint.js` row), add a new row:

```
  run-final-test.js        Read-only Final Test scraper: walks every pill, guesses each non-listening question once via the LLM, skips listening questions, writes final-test-answers.md — never clicks an option, Save, or Submit (exports run(driver?), scrapeFinalTest, answerWithRetry, formatQuestionRecord, buildMarkdown, also runnable directly)
```

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: document the final-test mode"
```

---

## Task 6: Final full-suite verification

**Files:** none (verification only)

- [ ] **Step 1: Run the entire test suite**

Run: `bun test tests/*.test.js`
Expected: All tests PASS, including the new `tests/run-final-test.test.js` (12 tests) and the appended export check in `tests/run-checkpoint.test.js`.

- [ ] **Step 2: Confirm no stray uncommitted changes**

Run: `git status`
Expected: clean working tree (everything from Tasks 1-5 committed).

Live verification against a real Final Test page is out of scope for this plan (per the design doc's noted limitation — the page's exact DOM shape is inherited-but-unverified from `run-checkpoint.js`) and should happen the first time this mode is actually run against an unlocked Final Test.
