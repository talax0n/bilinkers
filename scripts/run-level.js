const { Builder, By } = require('selenium-webdriver');
const chrome = require('selenium-webdriver/chrome');
const { resolveChromedriverPath } = require('../src/chromedriver');
const logger = require('../src/logger');
const { run: runCourse } = require('./run-course');

// One level up from run-course.js: drives the whole island map (see the
// isometric island screenshot — hexagon nodes B1.1, B1.2, B2.1, B2.2, C1.1,
// C1.2, C2.1, C2.2 along a zig-zag road) end to end: open the next
// unattempted course node, run run-course.js against it on the same browser
// session, come back to the island map, repeat. Run via `node scripts/cli.js
// level`, or invoke this file directly if the browser is already up and
// sitting on the island map page.
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

// Order B1.1, B1.2, B2.1, B2.2, C1.1, C1.2, C2.1, C2.2 — derived from each
// label's own letter/level/sub-level value, not from pixel position. The
// course roadmap (run-course.js) gets away with a "top" pixel sort because
// that order was verified live against the header's progress counters; no
// such live verification exists for the island map, and the label already
// encodes the correct order unambiguously, so parsing it directly is both
// simpler and safer than trusting an unverified layout assumption. No
// per-node progress counter exists on this page either (unlike the course
// roadmap's "Unit 4/8" header), so completion is decided purely by "does
// clicking still navigate" — a locked node's URL never changes, same rule
// run-course.js already relies on for locked units.
async function readLevelNodes(driver) {
  await driver.switchTo().defaultContent();
  const nodes = await driver.executeScript(`
    return [...document.querySelectorAll('.hoverable-pointer')].map((el) =>
      el.textContent.replace(/\\s+/g, '').trim()
    );
  `);

  // Whitelisted, not blacklisted: only "B1.1"-shaped labels are ever
  // recognised.
  const parsed = nodes
    .map((label) => {
      const match = label.match(/^([BC])(\d)\.(\d)$/i);
      if (!match) return null;
      const [, letter, level, sub] = match;
      return {
        type: 'course',
        label: `${letter.toUpperCase()}${level}.${sub}`,
        order: `${letter.toUpperCase()}${level}${sub}`,
      };
    })
    .filter(Boolean);

  parsed.sort((a, b) => a.order.localeCompare(b.order));
  return parsed;
}

// Live-verified: unlike the course roadmap's Unit/Checkpoint nodes (which
// navigate straight on a JS-synthetic `el.click()`), an island node's click
// handler only fires on a real, trusted pointer event — a synthetic
// `el.click()` here silently no-ops (returns true, changes nothing). It
// also doesn't navigate directly: clicking the hex opens a side panel with
// two buttons, "Continue Unit N" and "View Unit List". "Continue Unit N"
// deep-links straight into that unit's own activity row list, skipping the
// roadmap entirely — "View Unit List" is the one that actually lands on the
// course roadmap (the "Unit 0/8 CP 0/3 FT 0/1" header + Unit/Checkpoint hex
// nodes) that run-course.js expects, so that's the one this clicks. A
// locked/unavailable course's node opens no panel at all (no buttons beyond
// the header's own XP/notification ones) — "no matching button appears
// within the timeout" is this level's equivalent of run-course.js's "URL
// never changed" locked-node signal.
async function clickNode(driver, label) {
  await driver.switchTo().defaultContent();
  const nodeEls = await driver.findElements(By.css('.hoverable-pointer'));
  let nodeTarget = null;
  for (const el of nodeEls) {
    const text = (await el.getText()).replace(/\s+/g, '');
    if (text === label) {
      nodeTarget = el;
      break;
    }
  }
  if (!nodeTarget) return false;
  // A node can sit below the fold (island map is taller than the viewport),
  // and a click whose target point lands outside the viewport gets clipped
  // onto whatever fixed-position chrome (e.g. the nav sidebar) happens to
  // occupy that screen coordinate instead of erroring — live-verified this
  // silently navigated to the ViCon nav page rather than opening the node's
  // panel. scrollIntoView({block:'center'}) first so the click point is
  // always actually on-screen.
  await driver.executeScript("arguments[0].scrollIntoView({block: 'center'})", nodeTarget);
  await driver.sleep(200);
  await driver.actions({ bridge: true }).move({ origin: nodeTarget }).click().perform();
  // The panel slides/fades in (MUI-style transition) rather than appearing
  // instantly — clicking its button the moment it's found in the DOM lands
  // mid-animation and silently misses (live-verified: click "succeeds" per
  // WebDriver but the panel stays open, no navigation). Give it a beat to
  // settle before the button becomes a valid click target.
  await driver.sleep(500);

  const deadline = Date.now() + 5000;
  let buttonTarget = null;
  while (Date.now() < deadline && !buttonTarget) {
    const buttons = await driver.findElements(By.css('button'));
    for (const button of buttons) {
      const text = (await button.getText()).trim();
      if (text === 'View Unit List') {
        buttonTarget = button;
        break;
      }
    }
    if (!buttonTarget) await driver.sleep(200);
  }
  if (!buttonTarget) return false;

  await driver.executeScript("arguments[0].scrollIntoView({block: 'center'})", buttonTarget);
  await driver.sleep(500);
  await driver.actions({ bridge: true }).move({ origin: buttonTarget }).click().perform();
  return true;
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

async function main(
  existingDriver,
  {
    readLevelNodesFn = readLevelNodes,
    clickNodeFn = clickNode,
    waitForUrlChangeFn = waitForUrlChange,
    runCourseFn = runCourse,
    readRetryMs = 8000,
  } = {}
) {
  const driver = existingDriver || (await attachToBrave());
  const levelUrl = await driver.getCurrentUrl();

  const attempted = new Set();
  const completed = [];

  for (;;) {
    // Same race as run-course.js: the island map can still be loading right
    // after navigating back to it, so a single empty read isn't trusted as
    // "genuinely missing" — poll for a few seconds first.
    const readDeadline = Date.now() + readRetryMs;
    let nodes;
    for (;;) {
      nodes = await readLevelNodesFn(driver);
      if ((Array.isArray(nodes) && nodes.length) || Date.now() > readDeadline) break;
      await driver.sleep(300);
    }
    if (!Array.isArray(nodes) || !nodes.length) {
      logger.warn('Island map could not be read — stopping', { nodes: Array.isArray(nodes) ? nodes.length : null });
      return { status: 'stuck', reason: 'missing-roadmap', completed };
    }

    const next = nodes.find((n) => !attempted.has(n.label));
    if (!next) {
      logger.info('No unattempted courses left on this island — done', { completed });
      return { status: 'complete', completed };
    }
    attempted.add(next.label);

    logger.info('Opening course node', { label: next.label });
    const beforeUrl = await driver.getCurrentUrl();
    const clicked = await clickNodeFn(driver, next.label);
    if (!clicked) {
      logger.warn('Could not click course node — stopping', { label: next.label });
      return { status: 'stuck', label: next.label, completed };
    }

    const navigated = await waitForUrlChangeFn(driver, beforeUrl, 10000);
    if (!navigated) {
      // Locked (frontier the platform hasn't opened yet) or a click landed
      // on the road/deco image instead of a real node. Either way nothing
      // more is reachable right now.
      logger.warn('Course node did not open (URL never changed) — stopping', { label: next.label });
      return { status: 'stuck', label: next.label, completed };
    }
    await driver.sleep(1500);

    logger.info('Running course', { label: next.label });
    const result = await runCourseFn(driver);
    logger.info('Course finished', { label: next.label, ...result });

    if (result.status !== 'complete') {
      logger.warn('Course did not finish cleanly — stopping the level run so it can be inspected', {
        label: next.label,
        courseResult: result,
      });
      return { status: 'stuck', label: next.label, courseResult: result, completed };
    }
    completed.push(next.label);

    await driver.switchTo().defaultContent();
    await driver.get(levelUrl);
    await driver.sleep(1500);
  }
}

if (require.main === module) {
  main().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}

module.exports = { run: main, readLevelNodes };
