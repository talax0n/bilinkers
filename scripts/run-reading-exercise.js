const { Builder } = require('selenium-webdriver');
const chrome = require('selenium-webdriver/chrome');
const { getCurrentQuestionDom } = require('../src/browser');
const { resolveChromedriverPath } = require('../src/chromedriver');
const logger = require('../src/logger');

// Brute-forces a Beelingua reading BlExercise (the numbered 1..N tile layout:
// passage on the left, multiple option-button sub-questions on the right)
// with NO LLM. Verified live against "ENG-B2.2 Unit 1" (15 questions, mixed
// reading-comprehension / true-false-not-given / paraphrase items):
//   - A wrong answer re-enables its options (you can re-pick), a CORRECT
//     answer locks them (all disabled). So each question is just cycled
//     A -> B -> C -> D until it sticks — no reasoning needed.
//   - Each tile's background colour is the per-question verdict, updated the
//     moment an answer is graded (both the "Check" and "Save & Next" item
//     types colour the tile), so it's the one signal that works for every
//     item type here:
//       correct   -> rgba(54, 192, 203, .3)  (faint teal)
//       wrong     -> rgba(232, 166, 38, .3)  (faint orange)
//       unanswered-> rgb(127, 202, 212)      (solid teal)
//       current   -> solid orange
//   - Submit is gated: the confirm modal says "All answers must be correct
//     to submit this exercise" and refuses until every tile is teal, so the
//     loop just keeps fixing wrong tiles until Submit goes through.
// CHROMEDRIVER_PATH overrides chromedriver auto-detection if needed.
const CHROMEDRIVER_PATH = process.env.CHROMEDRIVER_PATH;

// A question is keyed by its first option's text (stable + unique per item),
// so tried-letter memory survives the item scrolling in and out of view as
// the loop navigates between passages.
const TRIED = new Map();
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

// How many numbered tiles the exercise has (1..N across the top).
async function countTiles(driver) {
  return driver.executeScript(`
    const seen = {};
    for (const el of document.querySelectorAll('*')) {
      const t = (el.textContent || '').trim();
      if (!/^\\d{1,2}$/.test(t) || el.children.length > 1) continue;
      const bg = getComputedStyle(el).backgroundColor;
      if (!bg || bg === 'rgba(0, 0, 0, 0)') continue;
      seen[t] = 1;
    }
    return Object.keys(seen).length;
  `);
}

async function clickTile(driver, n) {
  await driver.executeScript(
    `
    const target = String(arguments[0]);
    const els = [...document.querySelectorAll('*')].filter(el => el.textContent.trim() === target && el.children.length <= 1);
    const t = els.find(el => getComputedStyle(el).cursor === 'pointer') || els[0];
    if (t) t.click();
  `,
    String(n)
  );
}

// Answers every still-open option-group on the current screen (a group whose
// options are enabled = not yet correct). Options are one flat list of
// buttons; a new group starts each time the letter resets to "A". Picks the
// first letter not already tried for that group, so repeated visits walk
// A -> B -> C -> D instead of re-picking the same wrong option.
async function answerOpenGroups(driver, triedObj) {
  const acted = await driver.executeScript(
    `
    const tried = arguments[0] || {};
    const btns = [...document.querySelectorAll('button.bl-w-full.justify-content-start')];
    const groups = [];
    let cur = null;
    for (const b of btns) {
      const letter = b.textContent.trim()[0];
      if (letter === 'A') { cur = []; groups.push(cur); }
      if (cur) cur.push(b);
    }
    const acted = [];
    for (const g of groups) {
      const enabled = g.filter(b => !b.disabled);
      if (!enabled.length) continue; // locked = already correct
      const key = g[0].textContent.trim().slice(0, 60);
      const done = tried[key] || [];
      const pick = enabled.find(b => !done.includes(b.textContent.trim()[0])) || enabled[0];
      const letter = pick.textContent.trim()[0];
      pick.click();
      acted.push({ key, letter });
    }
    return acted;
  `,
    triedObj
  );
  acted.forEach(({ key, letter }) => {
    const set = TRIED.get(key) || new Set();
    set.add(letter);
    TRIED.set(key, set);
  });
  return acted;
}

// Grades what was just answered. "Check" and "Save & Next" both grade the
// screen (and colour the tiles); either one is fine since the outer loop
// re-navigates by tile afterwards. Returns true if something was clicked.
async function gradeCurrent(driver) {
  return driver.executeScript(`
    const b = [...document.querySelectorAll('button')];
    const check = b.find(x => x.textContent.trim() === 'Check' && !x.disabled);
    if (check) { check.click(); return true; }
    const sn = b.find(x => x.textContent.trim() === 'Save & Next' && !x.disabled);
    if (sn) { sn.click(); return true; }
    const next = b.find(x => x.textContent.trim() === 'Next' && !x.disabled);
    if (next) { next.click(); return true; }
    return false;
  `);
}

// Is there a per-question "Check" (a.k.a. "Check My Answer") button on the
// current screen? Those questions grade one option at a time in place, so
// they can be brute-forced without leaving the question.
async function hasCheckButton(driver) {
  return driver.executeScript(`
    return !![...document.querySelectorAll('button')].find(b => /^Check( My Answer)?$/i.test(b.textContent.trim()) && !b.disabled);
  `);
}

async function countEnabledOptions(driver) {
  return driver.executeScript(`
    return [...document.querySelectorAll('button.bl-w-full.justify-content-start')].filter(b => !b.disabled).length;
  `);
}

// In-place brute force for a "Check My Answer" question: pick A, Check, and
// if the feedback says "Incorrect!" the options stay enabled (verified live)
// so pick B, Check, ... through E until "Correct!" locks the question. Fully
// resolves the question in a single visit. Returns 'correct' | 'exhausted'.
async function answerCheckQuestion(driver) {
  for (const letter of LETTERS) {
    const state = await driver.executeScript(
      `
      const letter = arguments[0];
      const opts = [...document.querySelectorAll('button.bl-w-full.justify-content-start')];
      if (!opts.some(b => !b.disabled)) return 'locked';
      const target = opts.find(b => !b.disabled && b.textContent.trim()[0] === letter);
      if (!target) return 'no-letter';
      target.click();
      return 'selected';
    `,
      letter
    );
    if (state === 'locked') return 'correct'; // already solved this visit
    if (state === 'no-letter') continue; // fewer than 5 options — skip missing letters

    await driver.sleep(400);
    await driver.executeScript(`const c = [...document.querySelectorAll('button')].find(b => /^Check( My Answer)?$/i.test(b.textContent.trim()) && !b.disabled); if (c) c.click();`);
    await driver.sleep(1400);

    const outcome = await driver.executeScript(`
      const h = [...document.querySelectorAll('h1,h2,h3,h4,h5,h6')].find(x => /^(Correct!|Incorrect!)$/i.test(x.textContent.trim()));
      return h ? h.textContent.trim() : null;
    `);
    logger.info('Reading exercise: check attempt', { letter, outcome });
    if (outcome === 'Correct!') return 'correct';
    // 'Incorrect!' — options stay enabled, loop to the next letter.
  }
  return 'exhausted';
}

// Finalises once every tile is teal. Opens the Submit confirm and clicks the
// modal's own Submit; if the "all answers must be correct" guard fires, backs
// out via Close so the loop keeps fixing. Returns 'done' | 'blocked' | 'no-submit'.
async function trySubmit(driver) {
  const opened = await driver.executeScript(`
    const s = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Submit' && !b.disabled);
    if (s) { s.click(); return true; }
    return false;
  `);
  if (!opened) return 'no-submit';
  await driver.sleep(1200);

  const blocked = await driver.executeScript(`
    return /All answers must be correct/i.test(document.body.innerText);
  `);
  if (blocked) {
    await driver.executeScript(`const c = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Close'); if (c) c.click();`);
    await driver.sleep(600);
    return 'blocked';
  }

  // All-correct path: Submit opens an "Are you sure?" confirmation with
  // Yes/No — click Yes to actually record the attempt.
  const confirmed = await driver.executeScript(`
    const y = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Yes' && b.offsetParent !== null);
    if (y) { y.click(); return true; }
    return false;
  `);
  if (!confirmed) {
    // No Yes/No and not blocked — close whatever opened and treat as not done.
    await driver.executeScript(`const c = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Close'); if (c) c.click();`);
    return 'blocked';
  }
  await driver.sleep(3000);
  return 'done';
}

// After a passing submit the result screen shows "You Passed! Your Score:
// 100" with a "Next" button back to the unit's activity list — read the
// score and click Next so the activity is fully closed out.
async function readResultAndAdvance(driver) {
  await driver.switchTo().defaultContent();
  const deadline = Date.now() + 8000;
  let score;
  while (Date.now() < deadline) {
    const result = await driver.executeScript(`
      const m = document.body.textContent.match(/Your Score:\\s*(\\d+)/);
      const btn = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Next');
      const clicked = Boolean(btn && !btn.disabled);
      if (clicked) btn.click();
      return { score: m ? Number(m[1]) : null, clicked };
    `);
    if (result.score !== null && score === undefined) score = result.score;
    if (result.clicked) break;
    await driver.sleep(300);
  }
  return score;
}

async function main(existingDriver) {
  const driver = existingDriver || (await attachToBrave());

  await getCurrentQuestionDom(driver);
  const tileCount = (await countTiles(driver)) || 15;
  logger.info('Reading exercise: starting', { tileCount });

  // A question is done when every option on it is locked (Mui-disabled) —
  // wrong answers always re-enable, so "all locked" can only mean correct.
  // Each pass walks all tiles answering whatever's still open with a fresh
  // letter; passes repeat until Submit goes through. Bounded so a genuinely
  // stuck item can't spin forever (worst case ~ options-per-question passes).
  const MAX_PASSES = 12;

  for (let pass = 0; pass < MAX_PASSES; pass += 1) {
    let answeredThisPass = 0;

    for (let n = 1; n <= tileCount; n += 1) {
      await clickTile(driver, n);
      await driver.sleep(800);

      if ((await countEnabledOptions(driver)) === 0) continue; // all locked = correct

      // Branch on the question's own UI: a "Check My Answer" button grades one
      // option at a time in place, so cycle it to a correct answer without
      // leaving the question. Otherwise it's a deferred "Save & Next" item —
      // fall back to the blocking method (answer, advance, revisit wrongs on a
      // later pass; correctness comes from the option lock).
      if (await hasCheckButton(driver)) {
        const outcome = await answerCheckQuestion(driver);
        answeredThisPass += 1;
        await driver.sleep(600);
        logger.info('Reading exercise: check question', { pass, tile: n, outcome });
        continue;
      }

      const acted = await answerOpenGroups(driver, Object.fromEntries([...TRIED].map(([k, v]) => [k, [...v]])));
      if (acted.length === 0) continue;

      answeredThisPass += acted.length;
      await driver.sleep(400);
      await gradeCurrent(driver);
      await driver.sleep(1100);
      logger.info('Reading exercise: answered', { pass, tile: n, groups: acted.length, letters: acted.map((a) => a.letter) });
    }

    const result = await trySubmit(driver);
    logger.info('Reading exercise: pass done', { pass, answeredThisPass, submit: result });
    if (result === 'done') {
      const score = await readResultAndAdvance(driver);
      logger.info('Reading exercise: complete', { questions: tileCount, score });
      return { status: 'complete', questions: tileCount, score };
    }

    if (answeredThisPass === 0 && result !== 'done') {
      // Nothing left to answer but Submit still refuses — a locked item is
      // actually wrong (breaks the re-enable assumption). Stop rather than spin.
      logger.warn('Reading exercise: stuck — all items locked but Submit blocked', { pass });
      return { status: 'stuck' };
    }
  }

  logger.warn('Reading exercise: hit pass cap without finishing', { passes: MAX_PASSES });
  return { status: 'incomplete' };
}

if (require.main === module) {
  main().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}

module.exports = { run: main };
