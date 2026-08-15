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
// The Submit click has the same silent-drop race as every other click this
// codebase has hit live (pill nav, Save, quizMatching's submit): a click that
// doesn't register leaves the attempt page exactly as it was, no confirm modal,
// no error. A single click attempt then burns the whole attempt's worth of
// answering for nothing when waitForScore below times out. So the Submit click
// is retried: click Submit, poll for the "Are you sure?" Yes confirm; if the
// confirm never shows within the window, the Submit click didn't land, so
// click it again. Returns true only once the confirm modal actually appeared
// and was confirmed — a false return is now a real "submit did not go
// through" signal the caller surfaces instead of ignoring.
async function clickSubmit(driver, { maxSubmitClicks = 3, confirmWaitMs = 8000, enabledWaitMs = 4000, logger: injectedLogger = logger } = {}) {
  await driver.switchTo().defaultContent();

  for (let submitAttempt = 1; submitAttempt <= maxSubmitClicks; submitAttempt += 1) {
    // A transiently-disabled Submit is NOT a failure — the save's network
    // call can still be settling and leave the button disabled for a moment
    // (verified live everywhere else in this file: pill/save clicks race the
    // same re-render). Poll for it to become enabled before declaring this
    // attempt dead.
    const deadline = Date.now() + enabledWaitMs;
    let clicked = false;
    while (Date.now() < deadline) {
      clicked = await driver.executeScript(`
        const buttons = Array.from(document.querySelectorAll('button'));
        const btn = buttons.find((b) => b.textContent.trim() === 'Submit');
        if (btn && !btn.disabled) { btn.click(); return true; }
        return btn ? false : null;
      `);
      if (clicked) break;
      if (clicked === null) break; // button not rendered at all — give up fast, retrying won't summon it
      await driver.sleep(200);
    }
    if (!clicked) {
      if (clicked === null) {
        injectedLogger.warn('Checkpoint: Submit button never appeared — cannot submit', { submitAttempt });
      } else {
        injectedLogger.warn('Checkpoint: Submit button stayed disabled — cannot submit', { submitAttempt });
      }
      return false;
    }

    const confirmDeadline = Date.now() + confirmWaitMs;
    while (Date.now() < confirmDeadline) {
      const confirmed = await driver.executeScript(`
        const buttons = Array.from(document.querySelectorAll('button'));
        const btn = buttons.find((b) => b.textContent.trim() === 'Yes');
        if (btn && !btn.disabled) { btn.click(); return true; }
        return false;
      `);
      if (confirmed) return true;
      await driver.sleep(200);
    }

    // No Yes confirm within the window — the Submit click never registered
    // (same race as the pill/save clicks). Retry the whole Submit+confirm.
    await driver.sleep(300);
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
// the next attempt (verified live). Each attempt uses ONE SHARED letter for
// every unlocked pill (A, then B, then C, ...), chosen by main() — the
// shared-letter brute force: correctness is only knowable next attempt, so a
// single letter per attempt is the simplest convergence model. No LLM
// involved either way.
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
// Uses a SHARED letter for every unlocked pill this attempt (one attempt =
// one letter, chosen by main() from LETTERS: A, then B, then C, ...). The
// checkpoint has no per-question feedback, so correctness is only knowable
// on the NEXT attempt — correct pills lock (isPillLockedFn) and drop out of
// the run, and the platform re-presents only still-incorrect pills as
// editable. Cycling one shared letter across attempts is the brute-force
// blocking model: each round answers everything still wrong with a fresh
// letter until the whole checkpoint is correct.
async function answerAllQuestions(
  driver,
  letter,
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
  // Brute-force blocking with a SHARED letter: every unlocked pill this
  // attempt is answered with the same letter (A, then B on the next attempt,
  // ...). The checkpoint has no per-question feedback — only the final score
  // — so a question's correctness is only knowable on the NEXT attempt via
  // isPillLockedFn (correct pills lock and get skipped; the platform
  // re-presents only still-incorrect pills as editable). One shared letter per
  // attempt keeps the model simple: the outer attempt loop in main() picks
  // the letter, and correct pills dropping out each round is the convergence.
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
      if (optionCount === 0) continue; // already correct (locked) — leave it

      const attemptLetter = String(letter);
      try {
        const selected = await answerEveryGroupFn(driver, attemptLetter);
        if (selected === 0) throw new Error(`no option button found for letter "${attemptLetter}"`);
        await clickSaveButtonFn(driver);
        await waitForPillSavedFn(driver, n);
        answered += 1;
        injectedLogger.info('Checkpoint question answered', { pill: n, type: handler.name, letter: attemptLetter, selected, completionPass });
      } catch (err) {
        // A pill with fewer options than this attempt's letter (e.g. a
        // 3-option TFNG on an "E" round) has nothing to click — the next
        // attempt's letter may fit it.
        injectedLogger.info('Checkpoint question has no option for this letter — will retry with the next letter next attempt', {
          pill: n,
          letter: attemptLetter,
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

  const submitted = await clickSubmitFn(driver);
  if (!submitted) {
    injectedLogger.warn('Checkpoint: submit did not go through — stopping instead of pretending the attempt completed', { questionNum: answered });
    return { status: 'submit-failed', questionNum: answered };
  }
  return { status: 'answered', questionNum: answered };
}

async function runCheckpointAttempt(driver, letter) {
  const answerResult = await answerAllQuestions(driver, letter);
  if (answerResult.status === 'unhandled') {
    return { status: 'unhandled', questionNum: answerResult.questionNum };
  }
  if (answerResult.status === 'submit-failed') {
    // The Submit click never landed (or the confirm never appeared) — no
    // result page is coming, so don't burn waitForScore's timeout pretending
    // an attempt happened. Surface it so main can decide instead of treating
    // it as a completed-but-wrong attempt.
    return { status: 'submit-failed', questionNum: answerResult.questionNum };
  }

  const score = await waitForScore(driver);
  logger.info('Checkpoint attempt graded', { questionNum: answerResult.questionNum, letter, score });
  return { status: 'complete', questionNum: answerResult.questionNum, score };
}

async function main(existingDriver, { runExerciseFn = runCheckpointAttempt } = {}) {
  const driver = existingDriver || (await attachToBrave());
  const gateUrl = await driver.getCurrentUrl();

  // Brute-force blocking: one shared letter per attempt, advancing A -> B ->
  // C -> ... on each retry. Correct pills from earlier attempts stay locked
  // and are skipped inside answerAllQuestions, so each new letter only has to
  // handle what's still wrong. Letters wrap around LETTERS for unlimited
  // attempts.
  let letterIndex = 0;
  let submitFailures = 0;

  for (let attempt = 1; ; attempt += 1) {
    const letter = LETTERS[letterIndex % LETTERS.length];
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

    if (result.status === 'submit-failed') {
      // The attempt page's per-pill answers were saved individually, so a
      // retry from the gate (Continue) resumes the same attempt with its
      // answers intact and only needs to land Submit. But a submit that
      // fails 3 times in a row is a real problem, not a transient click
      // race — stop rather than burn attempts against a broken button.
      submitFailures += 1;
      logger.warn('Checkpoint: submit did not go through — retrying from the gate', { attempt, submitFailures });
      if (submitFailures >= 3) {
        logger.warn('Checkpoint: submit kept failing — stopping', { attempt, submitFailures });
        return { status: 'submit-failed', attempt };
      }
      await driver.switchTo().defaultContent();
      await driver.get(gateUrl);
      await driver.sleep(1500);
      continue;
    }

    submitFailures = 0;

    if (result.score === 100) {
      logger.info('Checkpoint passed', { attempt, letter, score: result.score });
      return { status: 'passed', attempt, score: result.score };
    }

    logger.warn('Checkpoint attempt did not reach a passing score — retrying', { attempt, letter, score: result.score });
    letterIndex += 1;
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
