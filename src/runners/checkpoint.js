const { config } = require('../config');
const { attachToLms, getCurrentQuestionDom } = require('../browser');
const { createRegistry } = require('../questionTypes/registry');
const audioMultipleChoice = require('../questionTypes/audioMultipleChoice');
const readingComprehension = require('../questionTypes/readingComprehension');
const logger = require('../logger');

// Site behavior this relies on: docs/platform-notes.md#checkpoint

// One letter per attempt, cycling A..F. Attempts repeat without limit until the score is 100.
const LETTERS = ['A', 'B', 'C', 'D', 'E', 'F'];

// "Start Attempt N" for a new attempt, "Continue" for one in progress.
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

// Submit exists only on the last question, next to a Save that does not finalize.
// Clicks can drop silently, so retry until the Yes confirm appears.
async function clickSubmit(driver, { maxSubmitClicks = 3, confirmWaitMs = 8000, enabledWaitMs = 4000, logger: injectedLogger = logger } = {}) {
  await driver.switchTo().defaultContent();

  for (let submitAttempt = 1; submitAttempt <= maxSubmitClicks; submitAttempt += 1) {
    // Submit can stay disabled briefly while the last save settles.
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
      if (clicked === null) {
        await driver.sleep(200);
        continue;
      }
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

    // No confirm, so the Submit click dropped. Retry.
    await driver.sleep(300);
  }

  return false;
}

// The score follows "Your Score:" with no separator, e.g. "Your Score:83Correct25".
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

// Pills are counted by class, not colour. A colour filter missed the tan-bordered ones.
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

// Only 15 pills render at first. The unlabeled chevron right of the grid expands the rest.
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

// Locked (correct) pills never turn active when clicked, so skip them before navigating.
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

// Pill navigation alone sometimes left an answer unsaved, so Save after each answer.
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

// Wait for the save to land. A pill click during its re-render can be swallowed.
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

// One shared letter per attempt. Correct pills lock and are skipped on the next attempt.
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
  // Pills can render well after the gate click, so poll.
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

    // "Failed to load Assessment type" is a transient glitch. One reload clears it.
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
  for (let completionPass = 1; completionPass <= maxCompletionPasses; completionPass += 1) {
    for (const { n } of pills) {
      await ensurePillsExpandedFn(driver);
      if (await isPillLockedFn(driver, n)) continue; // locked means already correct

      // A pill click sometimes doesn't register. Retry once.
      let settled = (await clickPillFn(driver, n)) && (await waitForPillActiveFn(driver, n));
      if (!settled) {
        settled = (await clickPillFn(driver, n)) && (await waitForPillActiveFn(driver, n));
      }
      if (!settled) {
        injectedLogger.warn('Checkpoint: pill navigation did not settle — stopping', { pill: n });
        return { status: 'unhandled', questionNum: answered };
      }

      // Questions render top-level, so use a short iframe wait.
      const dom = await getCurrentQuestionDomFn(driver, { iframeWaitMs: 600 });
      const handler = registry.findHandler(dom);
      if (!handler) {
        injectedLogger.warn('Checkpoint: unhandled question type — skipping pill', { pill: n });
        continue;
      }

      // Options can still be mid-render (all disabled) after the pill turns active.
      // Poll so they aren't mistaken for a locked question.
      let optionCount = 0;
      const enabledDeadline = Date.now() + 2000;
      while (Date.now() < enabledDeadline) {
        optionCount = await driver.executeScript(`return [...document.querySelectorAll('button.bl-w-full.justify-content-start')].filter((b) => !b.disabled).length`);
        if (optionCount > 0) break;
        await driver.sleep(150);
      }
      if (optionCount === 0) continue; // locked, already correct

      const attemptLetter = String(letter);
      try {
        const selected = await answerEveryGroupFn(driver, attemptLetter);
        if (selected === 0) throw new Error(`no option button found for letter "${attemptLetter}"`);
        await clickSaveButtonFn(driver);
        await waitForPillSavedFn(driver, n);
        answered += 1;
        injectedLogger.info('Checkpoint question answered', { pill: n, type: handler.name, letter: attemptLetter, selected, completionPass });
      } catch (err) {
        // Fewer options than this letter (e.g. a 3-option TFNG on an E round). A later letter may fit.
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
    // Leftover pills just don't fit this letter. Submit what is answered instead of failing.
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

  // Submit exists only on the last pill, and Save & Next may have jumped elsewhere.
  const lastPill = pills[pills.length - 1];
  if (lastPill) {
    await ensurePillsExpandedFn(driver);
    await clickPillFn(driver, lastPill.n);
    await waitForPillActiveFn(driver, lastPill.n);
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
    return { status: 'submit-failed', questionNum: answerResult.questionNum };
  }

  const score = await waitForScore(driver);
  logger.info('Checkpoint attempt graded', { questionNum: answerResult.questionNum, letter, score });
  return { status: 'complete', questionNum: answerResult.questionNum, score };
}

async function main(existingDriver, { runExerciseFn = runCheckpointAttempt } = {}) {
  const driver = existingDriver || (await attachToLms());
  const gateUrl = await driver.getCurrentUrl();

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
      // Structural, not a wrong guess. A retry would hit the same page.
      logger.warn('Unhandled question type inside checkpoint — stopping', { attempt, questionNum: result.questionNum });
      return { status: 'unhandled', attempt, questionNum: result.questionNum };
    }

    if (result.status === 'submit-failed') {
      // Saved answers survive, so Continue resumes the attempt. Three failed submits in a row stop the run.
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

module.exports = { run: main, waitForScore, ensurePillsExpanded, clickGateButton, clickSaveButton, waitForPillSaved, isPillLocked, clickSubmit, answerAllQuestions, readPills, clickPill };
