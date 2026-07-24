const { Builder } = require('selenium-webdriver');
const chrome = require('selenium-webdriver/chrome');
const { resolveChromedriverPath } = require('../src/chromedriver');
const { config } = require('../src/config');
const { getCurrentQuestionDom } = require('../src/browser');
const { createRegistry } = require('../src/questionTypes/registry');
const audioMultipleChoice = require('../src/questionTypes/audioMultipleChoice');
const readingComprehension = require('../src/questionTypes/readingComprehension');
const logger = require('../src/logger');

// This script attaches to an already-running Chromium-based browser (Chrome,
// Brave, Edge) launched with --remote-debugging-port=9222, and drives a
// checkpoint (see the "ENG-B1.2 ... Checkpoint 1" gate screenshot: Total
// Question 30, Passing Score 100, Maximum Attempt Unlimited) end to end:
// click the gate's Start/Continue button, answer every question, submit,
// check the resulting score, and — since the platform allows unlimited
// attempts — retry the whole checkpoint from the gate if the score falls
// short of 100, up to a safety cap. Run via `node scripts/cli.js
// checkpoint`, or invoke this file directly if the browser is already up
// and sitting on a checkpoint's gate page.
// CHROMEDRIVER_PATH overrides auto-detection below (matches chromedriver to
// whatever's actually listening on the debug port) — only needed if that
// fails for your setup.
const CHROMEDRIVER_PATH = process.env.CHROMEDRIVER_PATH;

// Platform policy is genuinely unlimited attempts, so keep retrying wrong
// scores forever — only an unhandled question type (a structural gap, not
// an unlucky guess) stops the loop early.

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

// A checkpoint's per-question nav is "Save & Next" (verified live — not
// "Next", which goToNextQuestion() looks for, and not a bare pill click,
// which navigates without persisting the just-picked option since pills are
// for jumping/review, not saving).
async function clickSaveAndNext(driver) {
  await driver.switchTo().defaultContent();
  return driver.executeScript(`
    const buttons = Array.from(document.querySelectorAll('button'));
    const btn = buttons.find((b) => b.textContent.trim() === 'Save & Next');
    if (btn && !btn.disabled) { btn.click(); return true; }
    return false;
  `);
}

// The filtered "only-incorrect" retry attempts label the per-question nav
// button plain "Save" instead of "Save & Next" (verified live: clicking it
// advances exactly like "Save & Next" on a full attempt) — so the
// brute-force loop needs this fallback, otherwise it stalls on question 1 of
// every retry round waiting for a "Save & Next" that never appears.
async function clickSaveButton(driver) {
  await driver.switchTo().defaultContent();
  return driver.executeScript(`
    const buttons = Array.from(document.querySelectorAll('button'));
    const btn = buttons.find((b) => b.textContent.trim() === 'Save');
    if (btn && !btn.disabled) { btn.click(); return true; }
    return false;
  `);
}

// The last question has no "Save & Next" — instead it shows a per-question
// "Save" *and* a checkpoint-wide "Submit" side by side (verified live).
// "Save" only re-saves the current selection without finalizing anything —
// clicking it was the original bug here, since it comes before "Submit" in
// DOM order and looked like a valid advance button. "Submit" is the one
// that actually finalizes the checkpoint, gated behind an "Are you sure?"
// Yes/No confirm, same shape as run-exercise.js's native-exercise finishing
// sequence (but with no further "Next" afterward — the very next render is
// the result page).
async function clickSubmit(driver) {
  await driver.switchTo().defaultContent();
  const clicked = await driver.executeScript(`
    const buttons = Array.from(document.querySelectorAll('button'));
    const btn = buttons.find((b) => b.textContent.trim() === 'Submit');
    if (btn && !btn.disabled) { btn.click(); return true; }
    return false;
  `);
  if (!clicked) return false;

  const deadline = Date.now() + 8000;
  while (Date.now() < deadline) {
    const confirmed = await driver.executeScript(`
      const buttons = Array.from(document.querySelectorAll('button'));
      const btn = buttons.find((b) => b.textContent.trim() === 'Yes');
      if (btn && !btn.disabled) { btn.click(); return true; }
      return false;
    `);
    if (confirmed) return true;
    await driver.sleep(200);
  }
  return false;
}

// Confirming Submit lands directly on ".../result/version/{id}" (no
// "/review" needed just for the number), reading e.g. "...Sorry, Try
// Again.Your Score:83Correct25Incorrect5No Answer0..." — the score sits
// right after "Your Score:" with no separator (verified live: a checkpoint
// that missed 5 of 30 questions read "Your Score:83").
async function waitForScore(driver, timeoutMs = 10000) {
  await driver.switchTo().defaultContent();
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const score = await driver.executeScript(`
      const match = document.body.textContent.match(/Your Score:(\\d+)/);
      return match ? Number(match[1]) : null;
    `);
    if (score !== null) return score;
    await driver.sleep(300);
  }
  return undefined;
}

// Checkpoints never show a per-question "Correct!"/"Incorrect!" heading, only
// a final score after submitting everything — so there's no live signal to
// brute-force a question in place. Instead it's brute-forced ACROSS attempts:
// answer every presented question with a single fixed letter, submit, and the
// platform re-presents only the still-incorrect ones on the next attempt
// (verified live). So attempt 1 answers all "A", attempt 2 answers the
// remaining wrong ones all "B", then "C", then "D", ... — by elimination each
// question is cleared by whichever letter is correct, with no LLM. No
// per-question bookkeeping is needed: the platform's "only re-present the
// wrong ones" behaviour is what makes a fixed letter per attempt converge.
// Reads the checkpoint's numbered nav pills (1..N). Every pill is a
// '.bl-button__container' with a numeric label — keyed off that class (not a
// set of background colours) so pills in any state are counted, including the
// tan-bordered ones a colour whitelist missed (verified live: a colour filter
// found only 16 of 30).
async function readPills(driver) {
  return driver.executeScript(`
    const seen = {}, out = [];
    for (const el of document.querySelectorAll('.bl-button__container')) {
      const t = el.textContent.trim();
      if (!/^\\d{1,2}$/.test(t)) continue;
      const n = Number(t);
      if (n in seen) continue;
      seen[n] = 1;
      out.push({ n });
    }
    return out.sort((a, b) => a.n - b.n);
  `);
}

// The checkpoint initially renders only 15 pills. Its unlabeled down-chevron
// button expands the rest; identify that control by its icon and position next
// to the pill grid, then verify expansion by observing the pill count grow.
async function ensurePillsExpanded(driver, timeoutMs = 3000) {
  await driver.switchTo().defaultContent();
  const before = await readPills(driver);
  if (before.length === 0) return false;

  const clicked = await driver.executeScript(`
    const pills = [...document.querySelectorAll('.bl-button__container')]
      .filter((el) => /^\\d{1,2}$/.test(el.textContent.trim()));
    const rects = pills.map((el) => el.getBoundingClientRect()).filter((r) => r.width && r.height);
    if (rects.length === 0) return false;

    const right = Math.max(...rects.map((r) => r.right));
    const top = Math.min(...rects.map((r) => r.top));
    const bottom = Math.max(...rects.map((r) => r.bottom));
    const buttons = [...document.querySelectorAll('button')].filter((button) => {
      const path = button.querySelector('path')?.getAttribute('d') || '';
      const rect = button.getBoundingClientRect();
      return path.startsWith('M16.59 8.59L12 13.17') && rect.width && rect.height && rect.left >= right && rect.top >= top - 10 && rect.top <= bottom;
    });
    if (buttons[0] && !buttons[0].disabled) { buttons[0].click(); return true; }
    return false;
  `);
  if (!clicked) return false;

  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if ((await readPills(driver)).length > before.length) return true;
    await driver.sleep(100);
  }
  return false;
}

async function clickPill(driver, n) {
  await driver.switchTo().defaultContent();
  return driver.executeScript(
    `
    const target = String(arguments[0]);
    const el = [...document.querySelectorAll('.bl-button__container')].find((e) => e.textContent.trim() === target);
    if (el) { (el.closest('button') || el).click(); return true; }
    return false;
  `,
    String(n)
  );
}

async function waitForPillActive(driver, n, timeoutMs = 5000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    await ensurePillsExpanded(driver);
    const active = await driver.executeScript(
      `
      const target = String(arguments[0]);
      const pill = [...document.querySelectorAll('.bl-button__container')].find((el) => el.textContent.trim() === target);
      return pill?.classList.contains('secondary-shade-color') || false;
    `,
      String(n)
    );
    if (active) return true;
    await driver.sleep(100);
  }
  return false;
}

async function answerEveryGroup(driver, letter) {
  return driver.executeScript(
    `
    const letter = String(arguments[0]);
    const buttons = [...document.querySelectorAll('button.bl-w-full.justify-content-start')]
      .filter((button) => button.textContent.trim().startsWith(letter) && !button.disabled);
    for (const button of buttons) button.click();
    return buttons.length;
  `,
    letter
  );
}

// Persists the current selection. This checkpoint labels the button "Save"
// (retry rounds) or "Save & Next" (full round); either just saves — navigation
// is driven by clicking pills, not by these, because "Save"'s own next-jump is
// erratic (verified live: it skipped 3 -> 27, leaving 4..26 unanswered).
async function saveCurrent(driver) {
  return (await clickSaveAndNext(driver)) || (await clickSaveButton(driver));
}

async function waitForPillSaved(driver, n, timeoutMs = 20000) {
  await ensurePillsExpanded(driver);
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const saved = await driver.executeScript(
      `
      const target = String(arguments[0]);
      const pill = [...document.querySelectorAll('.bl-button__container')].find((el) => el.textContent.trim() === target);
      return pill?.classList.contains('primary-shade-color') || /Your Score:\s*\d+/.test(document.body.textContent) || location.pathname.includes('/result/');
    `,
      String(n)
    );
    if (saved) return true;
    await driver.sleep(100);
  }
  return false;
}

// Brute-forces one checkpoint attempt with a single fixed letter. Navigation
// is by clicking each numbered pill 1..N directly (NOT by "Save & Next", whose
// jump order is erratic here) so every question is actually visited. A pill
// whose options are all locked is already correct from a previous attempt —
// skipped, so its correct answer is preserved while only still-wrong questions
// get the new letter.
async function answerAllQuestions(driver, letter) {
  const registry = createRegistry();
  registry.register(audioMultipleChoice);
  registry.register(readingComprehension);

  await getCurrentQuestionDom(driver);
  await ensurePillsExpanded(driver);
  const pills = await readPills(driver);
  if (pills.length === 0) {
    logger.warn('Checkpoint: no nav pills found — stopping', {});
    return { status: 'unhandled', questionNum: 0 };
  }
  logger.info('Checkpoint attempt: answering questions', { pills: pills.length, letter });

  let answered = 0;
  for (const { n } of pills) {
    await ensurePillsExpanded(driver);
    if (!(await clickPill(driver, n)) || !(await waitForPillActive(driver, n))) {
      logger.warn('Checkpoint: pill navigation did not settle — stopping', { pill: n });
      return { status: 'unhandled', questionNum: answered };
    }

    // Checkpoint questions render top-level (no iframe), so don't burn the
    // default 8s iframe wait on every pill.
    const dom = await getCurrentQuestionDom(driver, { iframeWaitMs: 600 });
    const handler = registry.findHandler(dom);
    if (!handler) {
      logger.warn('Checkpoint: unhandled question type — skipping pill', { pill: n });
      continue;
    }

    const enabled = await driver.executeScript(`return [...document.querySelectorAll('button.bl-w-full.justify-content-start')].some((b) => !b.disabled)`);
    if (!enabled) continue; // already correct (locked) — leave it, preserve the earlier letter

    try {
      const selected = await answerEveryGroup(driver, letter);
      if (selected === 0) throw new Error(`no option button found for letter "${letter}"`);
      answered += 1;
      logger.info('Checkpoint question answered', { pill: n, type: handler.name, letter, selected });
    } catch (err) {
      // Fewer options than the current letter (e.g. a 3-option TFNG on a "D"
      // round) — it can't still be unresolved this late, so it's effectively
      // already correct; leave it.
      logger.info('Checkpoint question has no option for letter — skipping', { pill: n, letter, error: err.message });
    }
    await saveCurrent(driver);
    if (!(await waitForPillSaved(driver, n))) logger.warn('Checkpoint: answer not yet persisted — completion pass will retry', { pill: n });
  }

  await ensurePillsExpanded(driver);
  const unanswered = await driver.executeScript(`
    return [...document.querySelectorAll('.bl-button__container')]
      .filter((pill) => /^\\d{1,2}$/.test(pill.textContent.trim()) && pill.classList.contains('primary-light-shade-color'))
      .map((pill) => Number(pill.textContent.trim()));
  `);
  if (unanswered.length > 0) {
    logger.warn('Checkpoint: unanswered pills remain — retrying attempt pass', { unanswered });
    return answerAllQuestions(driver, letter);
  }

  // Some checkpoints auto-grade after the final Save; others render Submit.
  // clickSubmit no-ops for the auto-grade variant.
  await clickSubmit(driver);
  return { status: 'answered', questionNum: answered };
}

async function runCheckpointAttempt(driver, letter) {
  const answerResult = await answerAllQuestions(driver, letter);
  if (answerResult.status === 'unhandled') {
    return { status: 'unhandled', questionNum: answerResult.questionNum };
  }

  const score = await waitForScore(driver);
  logger.info('Checkpoint attempt graded', { questionNum: answerResult.questionNum, letter, score });
  return { status: 'complete', questionNum: answerResult.questionNum, score };
}

// Fixed letter per attempt; cycles A..F so retries stay unbounded (unlimited
// attempts) even in the unlikely event elimination hasn't converged by F.
const LETTERS = ['A', 'B', 'C', 'D', 'E', 'F'];

async function main(existingDriver, { runExerciseFn = runCheckpointAttempt } = {}) {
  const driver = existingDriver || (await attachToBrave());
  const gateUrl = await driver.getCurrentUrl();

  for (let attempt = 1; ; attempt += 1) {
    const letter = LETTERS[(attempt - 1) % LETTERS.length];
    logger.info('Starting checkpoint attempt', { attempt, letter });

    const clicked = await clickGateButton(driver);
    if (!clicked) {
      logger.warn('Could not find a Start Attempt/Continue button on the gate page — stopping', { attempt });
      return { status: 'no-gate', attempt };
    }
    await driver.sleep(1500);

    const result = await runExerciseFn(driver, letter);
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
}

if (require.main === module) {
  main().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}

module.exports = { run: main, waitForScore, ensurePillsExpanded, attachToBrave, clickGateButton, clickSaveAndNext, clickSubmit };
