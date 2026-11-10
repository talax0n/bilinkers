const { attachToLms, getCurrentQuestionDom } = require('../browser');
const logger = require('../logger');

// Site behavior this relies on: docs/platform-notes.md#grouped-reading-exercise

const LETTERS = ['A', 'B', 'C', 'D', 'E', 'F'];

async function readPills(driver) {
  return driver.executeScript(`
    return [...document.querySelectorAll('.bl-button__container')]
      .map((pill) => Number(pill.textContent.trim()))
      .filter((pill) => Number.isInteger(pill) && pill > 0)
      .filter((pill, index, all) => all.indexOf(pill) === index)
      .sort((a, b) => a - b);
  `);
}

async function clickPill(driver, n) {
  return driver.executeScript(
    `
    const target = String(arguments[0]);
    const pill = [...document.querySelectorAll('.bl-button__container')]
      .find((element) => element.textContent.trim() === target);
    if (pill) { pill.click(); return true; }
    return false;
  `,
    String(n)
  );
}

async function leaveByNextPill(driver, pills, currentPill) {
  const currentIndex = pills.indexOf(currentPill);
  const nextPill = pills[(currentIndex + 1) % pills.length];
  if (nextPill === undefined || nextPill === currentPill) return false;
  const clicked = await clickPill(driver, nextPill);
  if (clicked) await driver.sleep(1000);
  return clicked;
}

// Options are one flat list. A new group starts at each "A". Tried letters are keyed
// by pill, group index and option text, so repeat visits walk A, B, C, D.
async function answerOpenGroups(driver, triedMap, pill) {
  const triedObj = Object.fromEntries([...triedMap].map(([key, value]) => [key, [...value]]));
  const acted = await driver.executeScript(
    `
    const tried = arguments[0] || {};
    const btns = [...document.querySelectorAll('button.bl-w-full.justify-content-start')]
      .filter((button) => button.offsetParent !== null);
    const groups = [];
    let cur = null;
    for (const b of btns) {
      const letter = b.textContent.trim()[0];
      if (letter === 'A') { cur = []; groups.push(cur); }
      if (cur) cur.push(b);
    }
    const acted = [];
    for (const [groupIndex, g] of groups.entries()) {
      const enabled = g.filter(b => !b.disabled);
      if (!enabled.length) continue; // locked = already correct
      const key = [arguments[1], groupIndex, g[0].textContent.trim().slice(0, 60)].join(':');
      const done = tried[key] || [];
      const pick = enabled.find(b => !done.includes(b.textContent.trim()[0])) || enabled[0];
      const letter = pick.textContent.trim()[0];
      pick.click();
      acted.push({ key, letter });
    }
    return acted;
  `,
    triedObj,
    pill
  );
  acted.forEach(({ key, letter }) => {
    const set = triedMap.get(key) || new Set();
    set.add(letter);
    triedMap.set(key, set);
  });
  return acted;
}

// A Check button means the question grades in place.
async function hasCheckButton(driver) {
  return driver.executeScript(`
    return !![...document.querySelectorAll('button')].find(b => /^Check( My Answer)?$/i.test(b.textContent.trim()) && !b.disabled);
  `);
}

async function countEnabledOptions(driver) {
  return driver.executeScript(`
    return [...document.querySelectorAll('button.bl-w-full.justify-content-start')]
      .filter((button) => button.offsetParent !== null && !button.disabled).length;
  `);
}

// Wrong answers keep options enabled, so cycle letters until "Correct!" locks them.
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
    if (state === 'no-letter') continue; // fewer options than letters

    await driver.sleep(400);
    await driver.executeScript(`const c = [...document.querySelectorAll('button')].find(b => /^Check( My Answer)?$/i.test(b.textContent.trim()) && !b.disabled); if (c) c.click();`);
    await driver.sleep(1400);

    const outcome = await driver.executeScript(`
      const h = [...document.querySelectorAll('h1,h2,h3,h4,h5,h6')].find(x => /^(Correct!|Incorrect!)$/i.test(x.textContent.trim()));
      return h ? h.textContent.trim() : null;
    `);
    logger.info('Reading exercise: check attempt', { letter, outcome });
    if (outcome === 'Correct!') return 'correct';
    // Incorrect. Options stay enabled, so try the next letter.
  }
  return 'exhausted';
}

// Submit refuses with "All answers must be correct" until every item is right.
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

  const confirmed = await driver.executeScript(`
    const y = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Yes' && b.offsetParent !== null);
    if (y) { y.click(); return true; }
    return false;
  `);
  if (!confirmed) {
    await driver.executeScript(`const c = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Close'); if (c) c.click();`);
    return 'blocked';
  }
  await driver.sleep(3000);
  return 'done';
}

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
  const driver = existingDriver || (await attachToLms());
  const tried = new Map();

  // Skip the long LTI iframe wait.
  await getCurrentQuestionDom(driver, { iframeWaitMs: 300 });
  const pills = await readPills(driver);
  const tileCount = pills.length;
  if (tileCount < 2) {
    logger.warn('Reading exercise: grouped pill layout not found', { tileCount });
    return { status: 'unhandled' };
  }
  logger.info('Reading exercise: starting', { tileCount });

  // Correct answers lock and wrong ones re-enable, so all-locked means correct. Passes are capped.
  const MAX_PASSES = 12;

  for (let pass = 0; pass < MAX_PASSES; pass += 1) {
    let answeredThisPass = 0;

    for (const n of pills) {
      await clickPill(driver, n);
      await driver.sleep(800);

      if ((await countEnabledOptions(driver)) === 0) continue; // all locked = correct

      // Check questions resolve in one visit. Others are graded by leaving via a pill
      // and seen locked on a later pass.
      if (await hasCheckButton(driver)) {
        const outcome = await answerCheckQuestion(driver);
        answeredThisPass += 1;
        await driver.sleep(600);
        await leaveByNextPill(driver, pills, n);
        logger.info('Reading exercise: check question', { pass, tile: n, outcome });
        continue;
      }

      const acted = await answerOpenGroups(driver, tried, n);
      if (acted.length === 0) continue;

      answeredThisPass += acted.length;
      await driver.sleep(400);
      await leaveByNextPill(driver, pills, n);
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
      // Everything is locked but Submit still refuses, so a locked item is wrong.
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

module.exports = { run: main, readPills, clickPill, leaveByNextPill, answerOpenGroups };
