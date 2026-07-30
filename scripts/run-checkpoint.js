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

// Per-pill brute-force letters cycle A..F so elimination stays unbounded
// (unlimited attempts) even in the unlikely event it hasn't converged by F.
const LETTERS = ['A', 'B', 'C', 'D', 'E', 'F'];

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
// answer every still-wrong pill, submit, and the platform re-presents (via
// isPillLocked, not disappearance — a checkpoint keeps showing every pill,
// just locks the correct ones) only the still-incorrect ones as editable on
// the next attempt (verified live). Each pill cycles its OWN letter
// (A, B, C, ...) independently rather than one letter shared by the whole
// attempt — see answerAllQuestions for why a shared letter can't solve
// clustered vocabulary questions. No LLM involved either way.
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

// Locked (already-correct) pills carry 'success-shade-color' and never gain
// 'secondary-shade-color' when clicked — the app apparently doesn't render
// into an already-locked question, so waitForPillActive can never succeed
// for one (verified live: clicking an already-correct pill left it
// success-shade-color, no active-state transition, yet nothing was actually
// stuck). Checked before navigating so locked pills are skipped outright
// instead of being treated as a failed pill transition.
async function isPillLocked(driver, n) {
  return driver.executeScript(
    `
    const target = String(arguments[0]);
    const pill = [...document.querySelectorAll('.bl-button__container')].find((el) => el.textContent.trim() === target);
    return pill?.classList.contains('success-shade-color') || false;
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

// Clicking a pill to advance was found to occasionally leave the just-answered
// question unsaved (Submit at question 30 not reflecting it) — so an explicit
// Save is now fired after every answer too, on top of the pill navigation,
// belt-and-suspenders. Matches "Save" or "Save & Next" (label varies by
// attempt type), never "Submit" (only the checkpoint-wide Submit finalizes).
async function clickSaveButton(driver) {
  await driver.switchTo().defaultContent();
  return driver.executeScript(`
    const buttons = Array.from(document.querySelectorAll('button'));
    const btn = buttons.find((b) => {
      const t = b.textContent.trim();
      return t === 'Save' || t === 'Save & Next';
    });
    if (btn && !btn.disabled) { btn.click(); return true; }
    return false;
  `);
}

// Save fires a network call; clicking the next pill before it lands can get
// swallowed by the save's own re-render (verified live: pill navigation then
// stalls on some later, unrelated pill — a race, not a fixed bad pill). Poll
// until the just-saved pill drops its "unanswered" marker
// (primary-light-shade-color, same class the completion-pass check below
// uses) before moving on, so navigation only ever starts once the save has
// actually landed.
async function waitForPillSaved(driver, n, timeoutMs = 5000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const saved = await driver.executeScript(
      `
      const target = String(arguments[0]);
      const pill = [...document.querySelectorAll('.bl-button__container')].find((el) => el.textContent.trim() === target);
      return pill ? !pill.classList.contains('primary-light-shade-color') : false;
    `,
      String(n)
    );
    if (saved) return true;
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

// Brute-forces one checkpoint attempt. Navigation is by clicking each
// numbered pill 1..N directly; each answer is locked in with an explicit
// Save click right after selecting it (relying on the pill transition alone
// to save was found to sometimes drop question 30's answer before Submit).
// A pill whose options are all locked is already correct from a previous
// attempt — skipped, so its correct answer is preserved.
//
// Each pill gets its OWN letter, tracked independently in `pillLetters` (a
// Map<pillNumber, letterIndex> owned and persisted across attempts by the
// caller) — NOT one shared letter for every pill this attempt. A single
// global letter was the original design (see git history), but verified
// live it can never solve a "vocabulary in context" cluster: several blanks
// in the same passage sharing one candidate word pool (e.g. seven relative-
// pronoun blanks all offering the same five who/when/where/which/that
// options). Forcing every blank in that cluster to the same letter each
// attempt satisfies at most one of them at a time — a real checkpoint
// plateaued at 60%/12-unresolved-pills for 27 straight attempts (3+ full
// A-F cycles with zero change) before this fix. Independent per-pill letters
// let each blank's own elimination converge regardless of what its cluster-
// mates are doing this attempt.
async function answerAllQuestions(
  driver,
  pillLetters,
  {
    createRegistryFn = createRegistry,
    getCurrentQuestionDomFn = getCurrentQuestionDom,
    ensurePillsExpandedFn = ensurePillsExpanded,
    readPillsFn = readPills,
    clickPillFn = clickPill,
    isPillLockedFn = isPillLocked,
    waitForPillActiveFn = waitForPillActive,
    answerEveryGroupFn = answerEveryGroup,
    clickSaveButtonFn = clickSaveButton,
    waitForPillSavedFn = waitForPillSaved,
    clickSubmitFn = clickSubmit,
    logger: injectedLogger = logger,
    maxCompletionPasses = 3,
  } = {}
) {
  const registry = createRegistryFn();
  registry.register(audioMultipleChoice);
  registry.register(readingComprehension);

  await getCurrentQuestionDomFn(driver);
  // The gate's Continue click can land the attempt page before its pills
  // have rendered — verified live: a single fixed post-click sleep found
  // zero pills, while re-checking a couple seconds later on the same
  // (already-navigated) page found 25+. Polled here instead of one fixed
  // wait, so a slow render doesn't get misread as "no gate/no questions".
  let pills = [];
  let reloaded = false;
  for (;;) {
    const pillsDeadline = Date.now() + 8000;
    while (Date.now() < pillsDeadline) {
      await ensurePillsExpandedFn(driver);
      pills = await readPillsFn(driver);
      if (pills.length > 0) break;
      await driver.sleep(300);
    }
    if (pills.length > 0 || reloaded) break;

    // Distinct from slow rendering: the LMS occasionally serves "Failed to
    // load Assessment type / Please contact Beelingua Support" instead of
    // the attempt (verified live after resuming an in-progress attempt) — a
    // plain page reload cleared it and the pills rendered normally. Only
    // retried once; a second failure is a real problem, not a glitch.
    const failedToLoad = await driver.executeScript(`return document.body.innerText.includes('Failed to load Assessment type')`);
    if (!failedToLoad) break;
    injectedLogger.warn('Checkpoint: page failed to load the assessment — reloading once', {});
    await driver.navigate().refresh();
    await driver.sleep(2000);
    reloaded = true;
  }
  if (pills.length === 0) {
    injectedLogger.warn('Checkpoint: no nav pills found — stopping', {});
    return { status: 'unhandled', questionNum: 0 };
  }
  injectedLogger.info('Checkpoint attempt: answering questions', { pills: pills.length });

  let answered = 0;
  let previousUnanswered = null;
  // Every pill this attempt actually tries to answer uses the SAME letter
  // for the whole attempt (reused across completion-pass re-visits — those
  // exist to retry a save that didn't land in time, not to try a new
  // letter), snapshotted here from the persistent per-pill state. Advanced
  // in pillLetters (for every OTHER attempt to pick up) only once, after
  // this attempt finishes.
  // Pills that have been wrong on every attempt so far all advance their
  // counter the same number of times (never having locked in to break the
  // pattern) — if they all started from the same index 0, they stay in
  // perfect lockstep forever, always trying the identical letter as each
  // other on every single attempt. For an independent per-pill MCQ that's
  // harmless. For a shared-pool cluster (verified live: 7 pills stuck at
  // the same 5-6 options each, cycling A->B->C->...->A in unison for 49+
  // attempts, score frozen dead flat at 77% the whole time) it's fatal:
  // giving every cluster member the identical word every round guarantees
  // a duplicate-use collision each time, so none of them can ever lock in,
  // which keeps them synchronized, which repeats the collision forever.
  // Staggering each pill's STARTING index by its own pill number means
  // cluster-mates are never in lockstep to begin with — they naturally
  // rotate through different relative letters every round instead. Plain
  // `n % LETTERS.length` was tried first and verified live to still fail:
  // pill 14 and pill 20 (exactly 6 apart) both landed on offset 2 and
  // stayed locked together for 17+ more attempts flat at 93% — any LINEAR
  // function of n mod 6 collides for every pair of pills spaced by a
  // multiple of 6, no choice of coefficients avoids it. A real (non-linear)
  // integer hash does, so pills 6 apart no longer trivially collide.
  //
  // That only staggers the STARTING offset, though — a NEW collision class
  // still exists: two pills whose starting offsets happen to hash to the
  // same bucket (pigeonhole-likely once there are more than a handful of
  // still-unresolved pills, since there are only 6 buckets) advance by the
  // same +1 every attempt and stay bucket-identical forever, i.e. permanent
  // lockstep — verified live: pills 11/20/22/24 all landed on the same
  // startIndexFor bucket and tracked each other in perfect lockstep
  // (identical letter, every attempt, 50+ attempts straight, flat score)
  // regardless of restart, since the hash is deterministic. Deriving the
  // actual LETTER from a hash of (pill number, cycling index) rather than
  // the index alone fixes this permanently, not just for this one collision:
  // even pills whose idx trajectories are bit-for-bit identical forever
  // still pick different letters every attempt, because their pill numbers
  // differ. idx itself still advances by a plain +1 and still cycles
  // through 6 distinct raw values per pill, so each pill still exhausts all
  // 6 letters (just in a pill-specific shuffled order) — full coverage is
  // preserved, only the cross-pill correlation is removed.
  const startIndexFor = (n) => {
    let h = n;
    h = ((h >>> 16) ^ h) * 0x45d9f3b;
    h = ((h >>> 16) ^ h) * 0x45d9f3b;
    h = (h >>> 16) ^ h;
    return Math.abs(h) % LETTERS.length;
  };
  // Takes the question's actual option count so the hash only ever lands on
  // a letter that exists for THIS pill — previously this modded by the fixed
  // LETTERS.length (6) regardless of how many options were on screen, so a
  // 4-option question hit "no option for this letter" on E/F rounds and sat
  // unanswered for a whole completion pass before the next attempt's letter
  // happened to land back in range.
  const letterIndexFor = (n, idx, count) => {
    let h = (n * 2654435761) ^ idx;
    h = ((h >>> 16) ^ h) * 0x45d9f3b;
    h = ((h >>> 16) ^ h) * 0x45d9f3b;
    h = (h >>> 16) ^ h;
    return Math.abs(h) % count;
  };
  const usedLetterIndex = new Map();
  const letterFor = (n, optionCount) => {
    if (!usedLetterIndex.has(n)) {
      usedLetterIndex.set(n, pillLetters.get(n) ?? startIndexFor(n));
    }
    const count = optionCount > 0 ? Math.min(optionCount, LETTERS.length) : LETTERS.length;
    return LETTERS[letterIndexFor(n, usedLetterIndex.get(n), count)];
  };

  for (let completionPass = 1; completionPass <= maxCompletionPasses; completionPass += 1) {
    for (const { n } of pills) {
      await ensurePillsExpandedFn(driver);
      if (await isPillLockedFn(driver, n)) continue; // already correct — no click, no active-state to wait for

      // A pill click occasionally doesn't register on the first try — same
      // class of race as the Save/Submit clicks above (verified live: a
      // pill reported "did not settle" mid-run, then answered normally on a
      // plain re-click seconds later with nothing else changed). Retried
      // once here before treating it as a genuine stall.
      let settled = (await clickPillFn(driver, n)) && (await waitForPillActiveFn(driver, n));
      if (!settled) {
        settled = (await clickPillFn(driver, n)) && (await waitForPillActiveFn(driver, n));
      }
      if (!settled) {
        injectedLogger.warn('Checkpoint: pill navigation did not settle — stopping', { pill: n });
        return { status: 'unhandled', questionNum: answered };
      }

      // Checkpoint questions render top-level (no iframe), so don't burn the
      // default 8s iframe wait on every pill.
      const dom = await getCurrentQuestionDomFn(driver, { iframeWaitMs: 600 });
      const handler = registry.findHandler(dom);
      if (!handler) {
        injectedLogger.warn('Checkpoint: unhandled question type — skipping pill', { pill: n });
        continue;
      }

      // waitForPillActiveFn above only confirms the PILL itself is marked
      // active — it says nothing about whether the question's own answer
      // buttons have finished rendering yet. A single immediate check here
      // can catch them still mid-render (all momentarily disabled), which
      // this code otherwise reads as "already locked" and silently skips —
      // no click, no log line, indistinguishable from a genuinely-solved
      // pill. Poll for a couple seconds first so a slow render isn't
      // mistaken for a locked question.
      let optionCount = 0;
      const enabledDeadline = Date.now() + 2000;
      while (Date.now() < enabledDeadline) {
        optionCount = await driver.executeScript(`return [...document.querySelectorAll('button.bl-w-full.justify-content-start')].filter((b) => !b.disabled).length`);
        if (optionCount > 0) break;
        await driver.sleep(150);
      }
      if (optionCount === 0) continue; // already correct (locked) — leave it, preserve the earlier letter

      const letter = letterFor(n, optionCount);
      try {
        const selected = await answerEveryGroupFn(driver, letter);
        if (selected === 0) throw new Error(`no option button found for letter "${letter}"`);
        await clickSaveButtonFn(driver);
        await waitForPillSavedFn(driver, n);
        answered += 1;
        injectedLogger.info('Checkpoint question answered', { pill: n, type: handler.name, letter, selected, completionPass });
      } catch (err) {
        // Fewer options than this pill's current letter (e.g. a 3-option
        // TFNG on an "E" round for that specific pill) — its own letter
        // still advances for next attempt below, same as if it had been
        // tried and found wrong, so it keeps cycling toward one that exists.
        injectedLogger.info('Checkpoint question has no option for this letter — will retry with the next letter next attempt', {
          pill: n,
          letter,
          error: err.message,
          completionPass,
        });
      }
    }

    await ensurePillsExpandedFn(driver);
    const unanswered = await driver.executeScript(`
      return [...document.querySelectorAll('.bl-button__container')]
        .filter((pill) => /^\\d{1,2}$/.test(pill.textContent.trim()) && pill.classList.contains('primary-light-shade-color'))
        .map((pill) => Number(pill.textContent.trim()));
    `);
    if (unanswered.length === 0) break;

    injectedLogger.warn('Checkpoint: unanswered pills remain after completion pass', { unanswered, completionPass, maxCompletionPasses });
    // Neither case here is a structural gap (registry.findHandler already
    // covers that, above) — it just means the current letter genuinely
    // doesn't apply to whatever's left (e.g. a 3-option question on an "F"
    // round). Previously this returned 'unhandled', which the outer
    // attempt-retry loop in main() treats as fatal and stops the whole
    // checkpoint — verified live: a checkpoint that was legitimately
    // converging (score climbing 40 -> 60 across letters) got killed here
    // on attempt 6 instead of submitting its partial answers and letting
    // the next letter keep going. So this now just stops iterating passes
    // and falls through to the normal Submit below, same as if every pill
    // had resolved cleanly.
    if (previousUnanswered && unanswered.length >= previousUnanswered.length) {
      injectedLogger.info('Checkpoint: unanswered pills made no progress this letter — submitting what is answered', {
        unanswered,
        previousUnanswered,
        completionPass,
      });
      break;
    }
    previousUnanswered = unanswered;

    if (completionPass === maxCompletionPasses) {
      injectedLogger.info('Checkpoint: unanswered pills remain after max completion passes — submitting what is answered', {
        unanswered,
        completionPass,
        maxCompletionPasses,
      });
      break;
    }
  }

  // Advance every pill actually tried this attempt to its next letter, so
  // the next attempt (whether it turns out correct or not — correctness
  // only becomes knowable via isPillLockedFn on the NEXT attempt) picks up
  // where this one left off instead of retrying the same letter forever.
  for (const [n, idx] of usedLetterIndex) {
    pillLetters.set(n, idx + 1);
  }

  await clickSubmitFn(driver);
  return { status: 'answered', questionNum: answered };
}

async function runCheckpointAttempt(driver, pillLetters) {
  const answerResult = await answerAllQuestions(driver, pillLetters);
  if (answerResult.status === 'unhandled') {
    return { status: 'unhandled', questionNum: answerResult.questionNum };
  }

  const score = await waitForScore(driver);
  logger.info('Checkpoint attempt graded', { questionNum: answerResult.questionNum, score });
  return { status: 'complete', questionNum: answerResult.questionNum, score };
}

async function main(existingDriver, { runExerciseFn = runCheckpointAttempt } = {}) {
  const driver = existingDriver || (await attachToBrave());
  const gateUrl = await driver.getCurrentUrl();

  // Owned here, not per-attempt: each pill's letter must persist and
  // advance across attempts (see answerAllQuestions), not reset every time
  // the gate reloads.
  const pillLetters = new Map();

  for (let attempt = 1; ; attempt += 1) {
    logger.info('Starting checkpoint attempt', { attempt });

    const clicked = await clickGateButton(driver);
    if (!clicked) {
      logger.warn('Could not find a Start Attempt/Continue button on the gate page — stopping', { attempt });
      return { status: 'no-gate', attempt };
    }
    await driver.sleep(1500);

    const result = await runExerciseFn(driver, pillLetters);
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

module.exports = { run: main, waitForScore, ensurePillsExpanded, attachToBrave, clickGateButton, clickSaveButton, waitForPillSaved, isPillLocked, clickSubmit, answerAllQuestions, readPills, clickPill };
