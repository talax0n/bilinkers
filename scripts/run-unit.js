const { Builder, By } = require('selenium-webdriver');
const chrome = require('selenium-webdriver/chrome');
const { resolveChromedriverPath } = require('../src/chromedriver');
const logger = require('../src/logger');
const { run: runExercise } = require('./run-exercise');
const { run: runIframeExercise } = require('./run-iframe-exercise');
const { run: runReadingExercise } = require('./run-reading-exercise');
const { isGroupedReadingLayout, groupedReadingLayoutFromDocument } = require('./grouped-reading');

// This script attaches to an already-running Chromium-based browser (Chrome,
// Brave, Edge) launched with --remote-debugging-port=9222, and drives a
// whole unit's activity list (see the "Unit 2" screenshot: a stack of rows —
// video, notes, Interactive Vocabulary/Grammar Activity, Multiple Choice
// Grammar Activity, Listening, Reading — each locked/in-progress/done) end
// to end: open the next unfinished row, run the matching bot
// (run-exercise.js or run-iframe-exercise.js) on the same browser session,
// go back to the list, repeat until nothing unlocked is left. Run via
// `node scripts/cli.js unit`, or invoke this file directly if the browser
// is already up and sitting on a unit's activity-list page (the URL with no
// /content/{id} suffix).
// CHROMEDRIVER_PATH overrides auto-detection below (matches chromedriver to
// whatever's actually listening on the debug port) — only needed if that
// fails for your setup.
const CHROMEDRIVER_PATH = process.env.CHROMEDRIVER_PATH;

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

// Each row is a plain MUI button with no href (React onClick) — title text
// lives in its ".bl-text-ellipsis" label, and a locked row carries both the
// "disabled" attribute and the Mui-disabled class (verified live against the
// "Unit 2" page: Reading/Listening/etc. are locked exactly this way until
// the row above them is completed). A completed row's status icon is the
// same checkmark SVG every time (verified on the already-done "Interactive
// Vocabulary Activity" row) — matched by its path data so finished rows get
// skipped instead of being re-run from scratch.
const CHECKMARK_PATH = 'M14.2021 1.52344L5.45215 10.273L1.07715 5.89844';

async function readRows(driver) {
  await driver.switchTo().defaultContent();
  return driver.executeScript(
    `
    const checkmarkPath = arguments[0];
    return Array.from(document.querySelectorAll('button.bl-w-full')).map((btn) => {
      const label = btn.querySelector('.bl-text-ellipsis');
      return {
        title: label ? label.textContent.trim() : '',
        disabled: btn.disabled || btn.classList.contains('Mui-disabled'),
        completed: btn.innerHTML.includes(checkmarkPath),
      };
    }).filter((r) => r.title);
  `,
    CHECKMARK_PATH
  );
}

async function clickRow(driver, title) {
  await driver.switchTo().defaultContent();
  return driver.executeScript(
    `
    const title = arguments[0];
    const buttons = Array.from(document.querySelectorAll('button.bl-w-full'));
    const target = buttons.find((b) => {
      const label = b.querySelector('.bl-text-ellipsis');
      return label && label.textContent.trim() === title && !b.disabled;
    });
    if (target) { target.click(); return true; }
    return false;
  `,
    title
  );
}

async function waitForUrlChange(driver, previousUrl, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const url = await driver.getCurrentUrl();
    if (url !== previousUrl) return url;
    await driver.sleep(200);
  }
  return null;
}

// Both native and iframe-backed activities open behind a native top-level
// "Exercise" gate first (URL suffix "/BlExercise?status=0" — Total
// Question/Passing Score/Cancel+Start buttons, no iframe yet) — verified
// live on "Interactive Grammar Activity": the real content (native quiz or
// LTI iframe) only loads after clicking through. A prior attempt already on
// record relabels the button "Continue" instead of "Start" (verified live,
// same gate, second visit) — both are accepted. Polls for a few seconds
// since the button isn't mounted the instant the URL changes (verified
// live: querying right after the URL update found zero buttons at all,
// React still rendering the route) — a single immediate check silently
// finds nothing and moves on, never clicking to enter the exercise.
// Resolves false (no retry needed) when the gate genuinely isn't present,
// since not every activity type has one.
async function clickStartGate(driver, timeoutMs = 8000) {
  await driver.switchTo().defaultContent();
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const clicked = await driver.executeScript(`
      const buttons = Array.from(document.querySelectorAll('button'));
      const btn = buttons.find((b) => ['Start', 'Continue'].includes(b.textContent.trim()));
      if (btn && !btn.disabled) { btn.click(); return true; }
      return false;
    `);
    if (clicked) return true;
    await driver.sleep(300);
  }
  return false;
}

// cli.js's own MuiButtonBase/bl-w-full top-level check can't be reused here:
// this dashboard's chrome (header, XP badge, the row list itself) is built
// from the same MUI components, so that marker is present on every page
// regardless of what activity is open. An iframe is the reliable signal for
// the LTI (Bits) player; its absence plus the option-button class the
// native quiz types (readingComprehension/audioMultipleChoice) key off of
// distinguishes native "exercise" pages from non-exercise content
// (video, notes) that this bot has nothing to do with.
async function detectMode(driver) {
  await driver.switchTo().defaultContent();
  const deadline = Date.now() + 8000;
  let iframes = await driver.findElements(By.css('iframe'));
  while (iframes.length === 0 && Date.now() < deadline) {
    await driver.sleep(200);
    iframes = await driver.findElements(By.css('iframe'));
  }
  if (iframes.length > 0) {
    // Two different activities live inside an iframe: the Bits/LTI player
    // (quiz-input-* markup, or a #/n slide-nav link) is 'iframe'; a reading
    // BlExercise (passage + numbered tiles + MUI 'bl-w-full
    // justify-content-start' option buttons) is 'reading'. Look inside to
    // tell them apart — treating every iframe as 'iframe' ran the wrong
    // runner on reading exercises.
    await driver.switchTo().frame(iframes[0]);
    const iframeHtml = await driver.executeScript('return document.documentElement.outerHTML');
    await driver.switchTo().defaultContent();
    if (iframeHtml.includes('bl-w-full justify-content-start')) return 'reading';
    return 'iframe';
  }

  const topLevelLayout = await driver.executeScript(`return (${groupedReadingLayoutFromDocument.toString()})()`);
  if (isGroupedReadingLayout(topLevelLayout)) return 'reading';

  const topHtml = await driver.executeScript('return document.documentElement.outerHTML');
  if (topHtml.includes('bl-w-full justify-content-start')) return 'exercise';
  return null;
}

const RUNNERS = { exercise: runExercise, iframe: runIframeExercise, reading: runReadingExercise };

// Non-quiz rows (video, reading material / Bee Notes) have no gate and no
// questions — they complete just by being viewed. A video only counts once it
// reaches the end, so any <video> on the page (top-level or inside an iframe)
// is muted and seeked to its final moment, then played so the player fires
// its "ended" progress event. Static content (notes) needs nothing beyond the
// open the caller already did. Returns 'video' | 'static'.
const SEEK_VIDEOS = `
  const vids = [...document.querySelectorAll('video')];
  vids.forEach((v) => {
    try {
      v.muted = true;
      if (isFinite(v.duration) && v.duration > 0) v.currentTime = Math.max(0, v.duration - 0.25);
      const p = v.play();
      if (p && p.catch) p.catch(() => {});
    } catch (e) {}
  });
  return vids.length;
`;

async function completeMediaActivity(driver) {
  await driver.switchTo().defaultContent();
  let seeked = await driver.executeScript(SEEK_VIDEOS);
  if (seeked === 0) {
    const iframes = await driver.findElements(By.css('iframe'));
    for (const frame of iframes) {
      await driver.switchTo().frame(frame);
      seeked += await driver.executeScript(SEEK_VIDEOS);
      await driver.switchTo().defaultContent();
      if (seeked > 0) break;
    }
  }
  if (seeked > 0) {
    // Let the last fraction of a second play out so "ended" fires and the
    // platform records the view.
    await driver.sleep(4000);
    return 'video';
  }
  // Static reading material / notes — opening it is enough to mark it viewed.
  await driver.sleep(1500);
  return 'static';
}

async function main() {
  const driver = await attachToBrave();
  const unitUrl = await driver.getCurrentUrl();

  // Titles we've already opened this run. A row that can't be auto-completed
  // (an unsupported activity, or a video/notes the platform didn't mark done)
  // must not be re-picked forever — once attempted, it's skipped so the loop
  // can move on or finish instead of spinning on the same row.
  const attempted = new Set();

  for (;;) {
    const rows = await readRows(driver);
    const next = rows.find((r) => !r.disabled && !r.completed && !attempted.has(r.title));
    if (!next) {
      const stuck = rows.filter((r) => !r.disabled && !r.completed).map((r) => r.title);
      if (stuck.length) {
        logger.warn('Remaining activities could not be auto-completed — done', { stuck });
      } else {
        logger.info('No unfinished activities left in this unit — done', { total: rows.length });
      }
      break;
    }
    attempted.add(next.title);

    logger.info('Opening activity', { title: next.title });
    const beforeUrl = await driver.getCurrentUrl();
    const clicked = await clickRow(driver, next.title);
    if (!clicked) {
      logger.warn('Could not click activity row — stopping', { title: next.title });
      break;
    }

    const navigated = await waitForUrlChange(driver, beforeUrl, 10000);
    if (!navigated) {
      logger.warn('Activity did not open (URL never changed) — stopping', { title: next.title });
      break;
    }

    const startedGate = await clickStartGate(driver);
    if (startedGate) {
      logger.info('Clicked past the Start gate', { title: next.title });
      await driver.sleep(1500);
    }

    const mode = await detectMode(driver);
    if (!mode) {
      // Not a quiz — a video or static reading material / notes. Complete it
      // by viewing: videos are seeked to the end, notes just need the open.
      const kind = await completeMediaActivity(driver);
      logger.info('Completed media activity', { title: next.title, kind });
      await driver.switchTo().defaultContent();
      await driver.get(unitUrl);
      await driver.sleep(1500);
      continue;
    }

    logger.info('Running activity', { title: next.title, mode });
    const result = await RUNNERS[mode](driver);
    logger.info('Activity finished', { title: next.title, mode, ...result });

    if (result && (result.status === 'unhandled' || result.status === 'incorrect')) {
      logger.warn('Activity did not finish cleanly — stopping the unit run so it can be inspected', {
        title: next.title,
        status: result.status,
      });
      break;
    }

    await driver.switchTo().defaultContent();
    await driver.get(unitUrl);
    await driver.sleep(1500);
  }
}

if (require.main === module) {
  main().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}

module.exports = { run: main };
