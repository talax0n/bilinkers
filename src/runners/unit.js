const { attachToLms, waitForUrlChange } = require('../browser');
const { By } = require('selenium-webdriver');
const logger = require('../logger');
const { run: runExercise } = require('./exercise');
const { run: runIframeExercise } = require('./iframe');
const { run: runReadingExercise } = require('./reading');
const { isGroupedReadingLayout, groupedReadingLayoutFromDocument } = require('../groupedReading');

// Site behavior this relies on: docs/platform-notes.md#unit-activity-list

// Completed rows show this checkmark SVG path.
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

// A synthetic click() sometimes doesn't navigate on these rows, so use a real pointer click.
async function clickRow(driver, title) {
  await driver.switchTo().defaultContent();
  const buttons = await driver.findElements(By.css('button.bl-w-full'));
  for (const btn of buttons) {
    // textContent, not getText(), to match readRows (getText collapses non-breaking spaces).
    const text = await driver.executeScript(
      "const label = arguments[0].querySelector('.bl-text-ellipsis'); return label ? label.textContent.trim() : '';",
      btn
    );
    if (!text || text !== title) continue;
    const disabled = await driver.executeScript(
      'return arguments[0].disabled || arguments[0].classList.contains("Mui-disabled");',
      btn
    );
    if (disabled) continue;
    await driver.executeScript("arguments[0].scrollIntoView({block: 'center'})", btn);
    await driver.sleep(200);
    await driver.actions({ bridge: true }).move({ origin: btn }).click().perform();
    return true;
  }
  return false;
}

// Activities open behind a Start/Continue gate that mounts after the URL changes.
// Not every activity has one.
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

// Checks iframe and top-level markers each tick. Native exercises never get an iframe.
async function detectMode(driver) {
  await driver.switchTo().defaultContent();
  const deadline = Date.now() + 8000;
  for (;;) {
    const iframes = await driver.findElements(By.css('iframe'));
    if (iframes.length > 0) {
      // Reading BlExercises also live in an iframe, with MUI option buttons instead of quiz-input-*.
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

    if (Date.now() >= deadline) return null;
    await driver.sleep(200);
  }
}

const RUNNERS = { exercise: runExercise, iframe: runIframeExercise, reading: runReadingExercise };

// Video and notes rows complete by being viewed. Videos must reach their end.
const PLAY_VIDEOS = `
  const vids = [...document.querySelectorAll('video')];
  vids.forEach((v) => {
    try {
      v.muted = true;
      const p = v.play();
      if (p && p.catch) p.catch(() => {});
    } catch (e) {}
  });
  return vids.length;
`;

async function completeMediaActivity(driver) {
  await driver.switchTo().defaultContent();
  let playing = await driver.executeScript(PLAY_VIDEOS);
  if (playing === 0) {
    const iframes = await driver.findElements(By.css('iframe'));
    for (const frame of iframes) {
      await driver.switchTo().frame(frame);
      playing += await driver.executeScript(PLAY_VIDEOS);
      if (playing > 0) break;
      await driver.switchTo().defaultContent();
    }
  }
  if (playing > 0) {
    // 15 minutes covers real-time playback when duration loads too late to seek.
    const deadline = Date.now() + 15 * 60 * 1000;
    while (Date.now() < deadline) {
      // Seeking exactly to duration never sets ended. Seek to duration - 1 and let it play out.
      const ended = await driver.executeScript(`
        const vids = [...document.querySelectorAll('video')];
        vids.forEach((v) => {
          if (!v.ended && v.duration && isFinite(v.duration) && v.currentTime < v.duration - 1) {
            v.currentTime = Math.max(0, v.duration - 1);
          }
          if (!v.ended && v.paused) {
            const p = v.play();
            if (p && p.catch) p.catch(() => {});
          }
        });
        return vids.length > 0 && vids.every((v) => v.ended);
      `);
      if (ended) return 'video';
      await driver.sleep(300);
    }
    throw new Error('Video did not finish within 15 minutes.');
  }
  await driver.sleep(300);
  return 'static';
}

async function main(existingDriver) {
  const driver = existingDriver || (await attachToLms());
  const unitUrl = await driver.getCurrentUrl();

  // Rows opened once this run are skipped, so an uncompletable row can't loop forever.
  const attempted = new Set();

  for (;;) {
    const rows = await readRows(driver);
    if (rows.length === 0) {
      logger.warn('No unit activity rows found — stopping');
      return { status: 'stuck', reason: 'no-rows' };
    }
    const next = rows.find((r) => !r.disabled && !r.completed && !attempted.has(r.title));
    if (!next) {
      const stuck = rows.filter((r) => !r.disabled && !r.completed).map((r) => r.title);
      if (stuck.length) {
        logger.warn('Remaining activities could not be auto-completed — done', { stuck });
        return { status: 'stuck', stuck };
      }
      logger.info('No unfinished activities left in this unit — done', { total: rows.length });
      return { status: 'complete', total: rows.length };
    }
    attempted.add(next.title);

    logger.info('Opening activity', { title: next.title });
    const beforeUrl = await driver.getCurrentUrl();
    const clicked = await clickRow(driver, next.title);
    if (!clicked) {
      logger.warn('Could not click activity row — stopping', { title: next.title });
      return { status: 'stuck', title: next.title };
    }

    const navigated = await waitForUrlChange(driver, beforeUrl, 10000);
    if (!navigated) {
      logger.warn('Activity did not open (URL never changed) — stopping', { title: next.title });
      return { status: 'stuck', title: next.title };
    }

    const startedGate = await clickStartGate(driver);
    if (startedGate) {
      logger.info('Clicked past the Start gate', { title: next.title });
      await driver.sleep(1500);
    }

    // Reading rows can show several option groups at once. Only the reading runner scopes clicks per group.
    const mode = next.title === 'Reading' ? 'reading' : await detectMode(driver);
    if (!mode) {
      const kind = await completeMediaActivity(driver);
      logger.info('Completed media activity', { title: next.title, kind });
      // Give the SPA time to POST the video's ended event before reloading.
      if (kind === 'video') await driver.sleep(3000);
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
      return { status: 'stuck', title: next.title, activityStatus: result.status };
    }

    // Same backend-sync grace before leaving a finished activity.
    await driver.switchTo().defaultContent();
    await driver.sleep(3000);
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

module.exports = { run: main, completeMediaActivity };
