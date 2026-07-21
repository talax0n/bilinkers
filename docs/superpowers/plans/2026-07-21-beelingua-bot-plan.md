# Beelingua Auto-Bot Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. **Task 8 is an exception — it requires live interactive browser use with the user and must NOT be dispatched to a subagent.**

**Goal:** Build a Node.js bot that logs into Beelingua (manual handoff), reads course questions, uses an OpenAI-compatible LLM to answer them, and drives Selenium to submit answers, looping through the whole course unattended.

**Architecture:** Small, focused modules (config, logger, progress, llm, question-type registry, browser, runner) wired together in `src/runner.js`. Question types are plugins: each exports `detect/parse/answer/checkResult`. Tasks 1–7 build DOM-agnostic, unit-testable scaffolding. Task 8 is a live session (site requires manual login and its DOM is unknown) that adds the first real question-type module, course-navigation selectors, and the dry-run smoke script.

**Tech Stack:** Node.js (built-in `node:test` + `node:assert/strict` for tests, no test framework dependency), `selenium-webdriver`, `openai`, `dotenv`.

## Global Constraints

- No credential storage or auto-login — login is always manual, per spec.
- Unknown question types must log + pause, never guess via vision fallback (per spec, explicitly deferred).
- Retry-on-wrong is exactly one retry, then log and move on (per spec).
- Progress persists to `progress.json` after each section so runs are resumable (per spec).
- No CI/live-site test fixtures — verification of DOM-dependent code happens live (per spec).

---

### Task 1: Project scaffold + config

**Files:**
- Create: `package.json`
- Create: `.env.example`
- Create: `.gitignore`
- Create: `src/config.js`
- Test: `tests/config.test.js`

**Interfaces:**
- Produces: `buildConfig(env: object) -> ConfigObject`, and `config` (the default-built `ConfigObject` from `process.env`), both exported from `src/config.js`.
  - `ConfigObject` shape: `{ openai: { baseURL, apiKey, model }, timeouts: { loginPollMs, loginTimeoutMs, elementWaitMs }, retry: { maxAnswerRetries }, paths: { progressFile, unhandledLogDir } }`

- [ ] **Step 1: Create package.json**

```json
{
  "name": "beelingua-bot",
  "version": "1.0.0",
  "private": true,
  "type": "commonjs",
  "scripts": {
    "test": "node --test tests/",
    "smoke": "node scripts/smoke.js",
    "start": "node src/runner.js"
  },
  "dependencies": {
    "dotenv": "^16.4.5",
    "openai": "^4.60.0",
    "selenium-webdriver": "^4.24.0"
  }
}
```

- [ ] **Step 2: Install dependencies**

Run: `npm install`
Expected: `node_modules/` created, `package-lock.json` created, no errors.

- [ ] **Step 3: Create .env.example and .gitignore**

`.env.example`:
```
OPENAI_BASE_URL=https://your-provider.example.com/v1
OPENAI_API_KEY=sk-your-key-here
OPENAI_MODEL=gpt-4o
```

`.gitignore`:
```
node_modules/
.env
progress.json
logs/
```

- [ ] **Step 4: Write the failing test for config**

`tests/config.test.js`:
```js
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
  assert.equal(cfg.retry.maxAnswerRetries, 1);
  assert.equal(cfg.paths.progressFile, './progress.json');
  assert.equal(cfg.paths.unhandledLogDir, './logs/unhandled');
});
```

- [ ] **Step 5: Run test to verify it fails**

Run: `npm test`
Expected: FAIL — `Cannot find module '../src/config'`

- [ ] **Step 6: Write src/config.js**

```js
require('dotenv').config();

function buildConfig(env) {
  return {
    openai: {
      baseURL: env.OPENAI_BASE_URL,
      apiKey: env.OPENAI_API_KEY,
      model: env.OPENAI_MODEL || 'gpt-4o',
    },
    timeouts: {
      loginPollMs: 2000,
      loginTimeoutMs: 5 * 60 * 1000,
      elementWaitMs: 10000,
    },
    retry: {
      maxAnswerRetries: 1,
    },
    paths: {
      progressFile: './progress.json',
      unhandledLogDir: './logs/unhandled',
    },
  };
}

module.exports = { buildConfig, config: buildConfig(process.env) };
```

- [ ] **Step 7: Run test to verify it passes**

Run: `npm test`
Expected: PASS — 3 tests passing.

- [ ] **Step 8: Commit**

```bash
git add package.json package-lock.json .env.example .gitignore src/config.js tests/config.test.js
git commit -m "feat: add project scaffold and config module"
```

---

### Task 2: Logger

**Files:**
- Create: `src/logger.js`
- Test: `tests/logger.test.js`

**Interfaces:**
- Consumes: none (stdlib `fs`, `path` only).
- Produces: `info(message, meta?)`, `warn(message, meta?)`, `error(message, meta?)` — each prints a JSON line to stdout. `saveUnhandled(dir, name, { screenshotBuffer, html }) -> { pngPath, htmlPath }` — writes files, creating `dir` if needed.

- [ ] **Step 1: Write the failing test**

`tests/logger.test.js`:
```js
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test`
Expected: FAIL — `Cannot find module '../src/logger'`

- [ ] **Step 3: Write src/logger.js**

```js
const fs = require('node:fs');
const path = require('node:path');

function log(level, message, meta = {}) {
  console.log(JSON.stringify({ time: new Date().toISOString(), level, message, ...meta }));
}

function info(message, meta) { log('info', message, meta); }
function warn(message, meta) { log('warn', message, meta); }
function error(message, meta) { log('error', message, meta); }

function saveUnhandled(dir, name, { screenshotBuffer, html }) {
  fs.mkdirSync(dir, { recursive: true });
  const base = path.join(dir, name);
  let pngPath = null;
  if (screenshotBuffer) {
    pngPath = `${base}.png`;
    fs.writeFileSync(pngPath, screenshotBuffer);
  }
  const htmlPath = `${base}.html`;
  fs.writeFileSync(htmlPath, html);
  return { pngPath, htmlPath };
}

module.exports = { info, warn, error, saveUnhandled };
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test`
Expected: PASS — 5 tests passing total.

- [ ] **Step 5: Commit**

```bash
git add src/logger.js tests/logger.test.js
git commit -m "feat: add logger with unhandled-question-type dumps"
```

---

### Task 3: Progress persistence

**Files:**
- Create: `src/progress.js`
- Test: `tests/progress.test.js`

**Interfaces:**
- Produces: `loadProgress(filePath) -> { lessonIndex: number, sectionIndex: number }` (returns default `{ lessonIndex: 0, sectionIndex: 0 }` if file missing). `saveProgress(filePath, progress) -> void`.

- [ ] **Step 1: Write the failing test**

`tests/progress.test.js`:
```js
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test`
Expected: FAIL — `Cannot find module '../src/progress'`

- [ ] **Step 3: Write src/progress.js**

```js
const fs = require('node:fs');

function loadProgress(filePath) {
  if (!fs.existsSync(filePath)) {
    return { lessonIndex: 0, sectionIndex: 0 };
  }
  return JSON.parse(fs.readFileSync(filePath, 'utf8'));
}

function saveProgress(filePath, progress) {
  fs.writeFileSync(filePath, JSON.stringify(progress, null, 2));
}

module.exports = { loadProgress, saveProgress };
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test`
Expected: PASS — 7 tests passing total.

- [ ] **Step 5: Commit**

```bash
git add src/progress.js tests/progress.test.js
git commit -m "feat: add resumable progress persistence"
```

---

### Task 4: LLM wrapper

**Files:**
- Create: `src/llm.js`
- Test: `tests/llm.test.js`

**Interfaces:**
- Consumes: an OpenAI-SDK-shaped client (`client.chat.completions.create(...)`), injected — never constructed inside the function under test.
- Produces: `createClient(config) -> OpenAIClient`, `answerQuestion(client, model, instruction, questionData, feedback = null) -> Promise<object>` (parsed JSON answer from the LLM).

- [ ] **Step 1: Write the failing test**

`tests/llm.test.js`:
```js
const assert = require('node:assert/strict');
const { test } = require('node:test');
const { answerQuestion } = require('../src/llm');

function makeFakeClient(responseObj) {
  const calls = [];
  return {
    calls,
    chat: {
      completions: {
        create: async (params) => {
          calls.push(params);
          return { choices: [{ message: { content: JSON.stringify(responseObj) } }] };
        },
      },
    },
  };
}

test('answerQuestion returns parsed JSON from the LLM response', async () => {
  const client = makeFakeClient({ answer: 'went' });
  const result = await answerQuestion(client, 'gpt-4o', 'Fill the blank', { text: 'She ___ home.' });
  assert.deepEqual(result, { answer: 'went' });
});

test('answerQuestion includes feedback in the prompt when retrying', async () => {
  const client = makeFakeClient({ answer: 'goes' });
  await answerQuestion(client, 'gpt-4o', 'Fill the blank', { text: 'She ___ home.' }, 'previous answer was wrong');
  const userMessage = client.calls[0].messages.find((m) => m.role === 'user');
  assert.match(userMessage.content, /previous answer was wrong/);
});

test('answerQuestion sends model and json response_format', async () => {
  const client = makeFakeClient({ answer: 'x' });
  await answerQuestion(client, 'my-model', 'instr', { text: 'q' });
  assert.equal(client.calls[0].model, 'my-model');
  assert.deepEqual(client.calls[0].response_format, { type: 'json_object' });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test`
Expected: FAIL — `Cannot find module '../src/llm'`

- [ ] **Step 3: Write src/llm.js**

```js
const OpenAI = require('openai');

function createClient(config) {
  return new OpenAI({ baseURL: config.openai.baseURL, apiKey: config.openai.apiKey });
}

async function answerQuestion(client, model, instruction, questionData, feedback = null) {
  const systemPrompt =
    'You are completing an English-course exercise. Respond with strict JSON only, ' +
    'matching the shape implied by the question data (e.g. { "answer": "..." } for a ' +
    'single answer, or { "answers": [...] } for multiple blanks/pills). No prose outside the JSON.';
  const userPrompt = JSON.stringify({ instruction, questionData, feedback });

  const response = await client.chat.completions.create({
    model,
    messages: [
      { role: 'system', content: systemPrompt },
      { role: 'user', content: userPrompt },
    ],
    response_format: { type: 'json_object' },
  });

  return JSON.parse(response.choices[0].message.content);
}

module.exports = { createClient, answerQuestion };
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test`
Expected: PASS — 10 tests passing total.

- [ ] **Step 5: Commit**

```bash
git add src/llm.js tests/llm.test.js
git commit -m "feat: add LLM wrapper for answering questions"
```

---

### Task 5: Question-type registry

**Files:**
- Create: `src/questionTypes/registry.js`
- Test: `tests/registry.test.js`

**Interfaces:**
- Produces: `createRegistry() -> Registry` where `Registry = { register(typeModule), findHandler(dom) -> typeModule|null, list() -> typeModule[] }`.
- Defines the type-module contract every future `src/questionTypes/*.js` (besides `registry.js`) must implement: `{ name: string, detect(dom) -> boolean, parse(dom) -> questionData, answer(driver, llmResult) -> Promise<void>, checkResult(dom) -> Promise<'correct'|'incorrect'> }`. This contract is documented here for Task 8 to follow; no concrete type module is built until Task 8 (real DOM unknown until then).

- [ ] **Step 1: Write the failing test**

`tests/registry.test.js`:
```js
const assert = require('node:assert/strict');
const { test } = require('node:test');
const { createRegistry } = require('../src/questionTypes/registry');

test('findHandler returns the first module whose detect() matches', () => {
  const registry = createRegistry();
  const mcq = { name: 'multipleChoice', detect: (dom) => dom.kind === 'mcq' };
  const fillBlank = { name: 'fillBlank', detect: (dom) => dom.kind === 'blank' };
  registry.register(mcq);
  registry.register(fillBlank);

  assert.equal(registry.findHandler({ kind: 'blank' }), fillBlank);
  assert.equal(registry.findHandler({ kind: 'mcq' }), mcq);
});

test('findHandler returns null when nothing matches', () => {
  const registry = createRegistry();
  registry.register({ name: 'mcq', detect: () => false });
  assert.equal(registry.findHandler({ kind: 'unknown' }), null);
});

test('list returns registered modules in registration order', () => {
  const registry = createRegistry();
  const a = { name: 'a', detect: () => false };
  const b = { name: 'b', detect: () => false };
  registry.register(a);
  registry.register(b);
  assert.deepEqual(registry.list(), [a, b]);
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test`
Expected: FAIL — `Cannot find module '../src/questionTypes/registry'`

- [ ] **Step 3: Write src/questionTypes/registry.js**

```js
function createRegistry() {
  const types = [];
  return {
    register(typeModule) {
      types.push(typeModule);
    },
    findHandler(dom) {
      return types.find((t) => t.detect(dom)) || null;
    },
    list() {
      return types.slice();
    },
  };
}

module.exports = { createRegistry };
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test`
Expected: PASS — 13 tests passing total.

- [ ] **Step 5: Commit**

```bash
git add src/questionTypes/registry.js tests/registry.test.js
git commit -m "feat: add question-type registry"
```

---

### Task 6: Runner core (answer/retry loop)

**Files:**
- Create: `src/runner.js`
- Test: `tests/runner.test.js`

**Interfaces:**
- Consumes: `registry.findHandler(dom)` (Task 5), a handler's `parse/answer/checkResult` (contract from Task 5), `answerQuestion(client, model, instruction, questionData, feedback)` (Task 4, injectable as `answerQuestionFn` for testing).
- Produces: `processQuestion({ driver, dom, registry, llmClient, model, instruction, retryLimit, answerQuestionFn? }) -> Promise<{ status: 'correct'|'incorrect'|'unhandled', attempts: number }>`.

- [ ] **Step 1: Write the failing test**

`tests/runner.test.js`:
```js
const assert = require('node:assert/strict');
const { test } = require('node:test');
const { processQuestion } = require('../src/runner');
const { createRegistry } = require('../src/questionTypes/registry');

test('processQuestion returns unhandled when no type module matches', async () => {
  const registry = createRegistry();
  registry.register({ name: 'mcq', detect: () => false });
  const result = await processQuestion({
    driver: {},
    dom: {},
    registry,
    llmClient: {},
    model: 'gpt-4o',
    instruction: 'instr',
    retryLimit: 1,
  });
  assert.deepEqual(result, { status: 'unhandled', attempts: 0 });
});

test('processQuestion succeeds on first attempt when correct', async () => {
  const registry = createRegistry();
  const answerCalls = [];
  registry.register({
    name: 'mcq',
    detect: () => true,
    parse: () => ({ text: 'q' }),
    answer: async (driver, llmResult) => { answerCalls.push(llmResult); },
    checkResult: async () => 'correct',
  });

  const answerQuestionFn = async () => ({ answer: 'A' });

  const result = await processQuestion({
    driver: {},
    dom: {},
    registry,
    llmClient: {},
    model: 'gpt-4o',
    instruction: 'instr',
    retryLimit: 1,
    answerQuestionFn,
  });

  assert.deepEqual(result, { status: 'correct', attempts: 1 });
  assert.equal(answerCalls.length, 1);
});

test('processQuestion retries once with feedback then gives up', async () => {
  const registry = createRegistry();
  const feedbacks = [];
  registry.register({
    name: 'mcq',
    detect: () => true,
    parse: () => ({ text: 'q' }),
    answer: async () => {},
    checkResult: async () => 'incorrect',
  });

  const answerQuestionFn = async (client, model, instruction, questionData, feedback) => {
    feedbacks.push(feedback);
    return { answer: 'A' };
  };

  const result = await processQuestion({
    driver: {},
    dom: {},
    registry,
    llmClient: {},
    model: 'gpt-4o',
    instruction: 'instr',
    retryLimit: 1,
    answerQuestionFn,
  });

  assert.deepEqual(result, { status: 'incorrect', attempts: 2 });
  assert.deepEqual(feedbacks, [null, 'previous answer was wrong, try again']);
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test`
Expected: FAIL — `Cannot find module '../src/runner'`

- [ ] **Step 3: Write src/runner.js**

```js
const { answerQuestion } = require('./llm');

async function processQuestion({
  driver,
  dom,
  registry,
  llmClient,
  model,
  instruction,
  retryLimit,
  answerQuestionFn = answerQuestion,
}) {
  const handler = registry.findHandler(dom);
  if (!handler) {
    return { status: 'unhandled', attempts: 0 };
  }

  const questionData = handler.parse(dom);
  let attempts = 0;
  let feedback = null;
  let outcome = 'incorrect';

  while (attempts <= retryLimit) {
    const llmResult = await answerQuestionFn(llmClient, model, instruction, questionData, feedback);
    await handler.answer(driver, llmResult);
    outcome = await handler.checkResult(dom);
    attempts += 1;
    if (outcome === 'correct') break;
    feedback = 'previous answer was wrong, try again';
  }

  return { status: outcome, attempts };
}

module.exports = { processQuestion };
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test`
Expected: PASS — 16 tests passing total.

- [ ] **Step 5: Commit**

```bash
git add src/runner.js tests/runner.test.js
git commit -m "feat: add runner core with retry-once-on-wrong logic"
```

---

### Task 7: Browser driver + manual-login wait

**Files:**
- Create: `src/browser.js`
- Test: `tests/browser.test.js`

**Interfaces:**
- Produces: `createDriver() -> Promise<WebDriver>` (real Selenium Chrome driver, not unit-tested — exercised live in Task 8). `waitForLogin(driver, { pollMs, timeoutMs, postLoginSelector }) -> Promise<true>` (throws on timeout) — unit-tested with a fake driver.

- [ ] **Step 1: Write the failing test**

`tests/browser.test.js`:
```js
const assert = require('node:assert/strict');
const { test } = require('node:test');
const { waitForLogin } = require('../src/browser');

function makeFakeDriver(foundOnAttempt) {
  let attempt = 0;
  return {
    async findElements() {
      attempt += 1;
      return attempt >= foundOnAttempt ? [{}] : [];
    },
    async sleep() {},
  };
}

test('waitForLogin resolves true once the post-login element appears', async () => {
  const driver = makeFakeDriver(3);
  const result = await waitForLogin(driver, { pollMs: 1, timeoutMs: 1000, postLoginSelector: '.dashboard' });
  assert.equal(result, true);
});

test('waitForLogin throws when timeout is reached before element appears', async () => {
  const driver = {
    async findElements() { return []; },
    async sleep() {},
  };
  await assert.rejects(
    () => waitForLogin(driver, { pollMs: 1, timeoutMs: 5, postLoginSelector: '.dashboard' }),
    /Timed out waiting for manual login/
  );
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test`
Expected: FAIL — `Cannot find module '../src/browser'`

- [ ] **Step 3: Write src/browser.js**

```js
const { Builder, By, until } = require('selenium-webdriver');

async function createDriver() {
  return new Builder().forBrowser('chrome').build();
}

async function waitForLogin(driver, { pollMs, timeoutMs, postLoginSelector }) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const found = await driver.findElements(By.css(postLoginSelector));
    if (found.length > 0) return true;
    await driver.sleep(pollMs);
  }
  throw new Error('Timed out waiting for manual login');
}

module.exports = { createDriver, waitForLogin, By, until };
```

Note: `By.css(...)` is called on the real `selenium-webdriver` `By` export even in the fake-driver test — that's fine, `By.css` just builds a locator object and doesn't touch a real browser.

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test`
Expected: PASS — 18 tests passing total.

- [ ] **Step 5: Commit**

```bash
git add src/browser.js tests/browser.test.js
git commit -m "feat: add browser driver setup and manual-login wait"
```

---

### Task 8: Live DOM discovery, first question-type module, course loop, smoke script

**This task requires live interaction with the user and the real Beelingua site. Do not dispatch to a subagent. Run inline, in conversation, with the user present to log in and navigate.**

**Files:**
- Create: `src/questionTypes/multipleChoice.js` (or whichever type is encountered first live — rename accordingly)
- Modify: `src/browser.js` — add course-navigation helpers (selectors discovered live)
- Modify: `src/runner.js` — add the top-level course loop (lesson/section iteration, calling `processQuestion` per question, saving progress via `src/progress.js`, wiring the registry)
- Create: `scripts/smoke.js` — dry-run script per spec (opens browser, waits for manual login, processes only the first question found, prints detected type + parsed data + LLM's answer, does not submit)

**Interfaces:**
- Consumes everything from Tasks 1–7: `config`, `logger`, `loadProgress/saveProgress`, `createClient/answerQuestion`, `createRegistry`, `processQuestion`, `createDriver/waitForLogin`.
- Produces: a runnable `npm start` (full unattended course run) and `npm run smoke` (dry-run single-question check).

- [ ] **Step 1: Live session — capture real DOM**

With the user: open Beelingua, log in manually, navigate to the first lesson/section. Use browser devtools (or a throwaway Selenium REPL script) to inspect the DOM for: the post-login marker element (for `waitForLogin`'s `postLoginSelector`), the section-instruction container, the first question's container and its type-distinguishing markup (e.g. multiple-choice options list), and the "next question" / "next section" controls.

- [ ] **Step 2: Write src/questionTypes/multipleChoice.js against the real selectors captured in Step 1**

Implement the full contract from Task 5: `{ name, detect(dom), parse(dom), answer(driver, llmResult), checkResult(dom) }`, using the real class names/attributes found live. Example shape (fill in real selectors from Step 1 — do not leave placeholder selectors):

```js
const { By } = require('../browser');

const name = 'multipleChoice';

function detect(dom) {
  // real selector from live discovery, e.g.:
  return dom.querySelector('.exercise--multiple-choice') !== null;
}

function parse(dom) {
  const questionText = dom.querySelector('.exercise__prompt').textContent.trim();
  const options = Array.from(dom.querySelectorAll('.exercise__option')).map((el) => el.textContent.trim());
  return { questionText, options };
}

async function answer(driver, llmResult) {
  const optionText = llmResult.answer;
  const options = await driver.findElements(By.css('.exercise__option'));
  for (const option of options) {
    const text = (await option.getText()).trim();
    if (text === optionText) {
      await option.click();
      return;
    }
  }
  throw new Error(`No option matched LLM answer: ${optionText}`);
}

async function checkResult(dom) {
  if (dom.querySelector('.exercise__feedback--correct')) return 'correct';
  return 'incorrect';
}

module.exports = { name, detect, parse, answer, checkResult };
```

- [ ] **Step 3: Verify the module live via a throwaway script**

Write and run a short throwaway Node script that: creates a driver, waits for login, gets the current page's DOM (via `driver.getPageSource()` parsed, or reading `.outerHTML` of the question container through `driver.executeScript`), runs `detect`/`parse` against it, and prints the parsed result. Confirm it matches the real question on screen. Delete the throwaway script once confirmed (it's not part of the shipped codebase).

- [ ] **Step 4: Add course-navigation helpers to src/browser.js**

Using selectors captured in Step 1, add functions such as `getSectionInstruction(driver) -> Promise<string>`, `getCurrentQuestionDom(driver) -> Promise<Document|Element>`, `goToNextQuestion(driver) -> Promise<boolean>` (returns false when section is out of questions), `goToNextSection(driver) -> Promise<boolean>` (returns false when course is complete). Exact implementation depends on what Step 1 found — implement against the real markup, no placeholder selectors.

- [ ] **Step 5: Wire the top-level course loop into src/runner.js**

Add a `runCourse({ driver, registry, llmClient, model, config, progress })` function that: loads progress via `loadProgress`, loops sections starting from saved position, reads section instruction, loops questions calling `processQuestion`, logs unhandled types via `logger.saveUnhandled` and pauses (per spec — throw/stop the loop rather than continue past an unhandled type), saves progress after each section via `saveProgress`, and continues until `goToNextSection` returns false.

- [ ] **Step 6: Write scripts/smoke.js**

```js
const { config } = require('../src/config');
const { createDriver, waitForLogin, getCurrentQuestionDom } = require('../src/browser');
const { createClient, answerQuestion } = require('../src/llm');
const { createRegistry } = require('../src/questionTypes/registry');
const multipleChoice = require('../src/questionTypes/multipleChoice');

async function main() {
  const registry = createRegistry();
  registry.register(multipleChoice);

  const driver = await createDriver();
  await driver.get('https://your-beelingua-login-url.example.com'); // real URL from Task 8 Step 1
  console.log('Log in manually, then the bot will continue...');
  await waitForLogin(driver, {
    pollMs: config.timeouts.loginPollMs,
    timeoutMs: config.timeouts.loginTimeoutMs,
    postLoginSelector: '.dashboard', // real selector from Task 8 Step 1
  });

  const dom = await getCurrentQuestionDom(driver);
  const handler = registry.findHandler(dom);
  if (!handler) {
    console.log('No handler matched this question — unhandled type.');
    await driver.quit();
    return;
  }

  const questionData = handler.parse(dom);
  console.log('Detected type:', handler.name);
  console.log('Parsed question data:', questionData);

  const client = createClient(config);
  const llmResult = await answerQuestion(client, config.openai.model, 'smoke test — no section instruction', questionData);
  console.log('LLM answer (NOT submitted):', llmResult);

  await driver.quit();
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
```

- [ ] **Step 7: Run the smoke script live**

Run: `npm run smoke`
Expected: browser opens, waits for manual login, prints detected type + parsed question data + LLM's proposed answer, exits without clicking anything. Confirm the printed answer looks correct for the real question shown.

- [ ] **Step 8: Run full test suite to confirm no regressions**

Run: `npm test`
Expected: PASS — all prior unit tests (18) still passing; Task 8's live-DOM code is intentionally not unit-tested (documented in the design spec's Testing section).

- [ ] **Step 9: Commit**

```bash
git add src/questionTypes/multipleChoice.js src/browser.js src/runner.js scripts/smoke.js
git commit -m "feat: add first question-type module, course loop, and smoke script"
```

---

## After Task 8

Remaining question types (fill-in-the-blank, pill-click, etc.) each get their own follow-up task: repeat Task 8's Steps 1–3 pattern (live capture → module → live verify) per type, register the new module in `runner.js`'s registry setup, and re-run the smoke/full-course flow. Not enumerated here since the concrete types beyond multiple-choice are only "etc" in the original request — add them as they're discovered live.
