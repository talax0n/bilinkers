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
// label's own letter/level/sub-level value. Course completion is not encoded
// in the rasterized node image or DOM attributes. Clicking an available node
// opens a panel: completed courses expose only "View Course Detail", while an
// unfinished course exposes "View Unit List". Locked nodes open no matching
// panel. openNode() uses that panel contract before run-course.js takes over.
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

// Island nodes require a trusted pointer event. An available node opens a
// panel whose ENG-B1.1-style heading identifies the selected course. Completed
// courses expose only "View Course Detail"; unfinished courses additionally
// expose "View Unit List". Locked nodes do not replace the current panel, so
// the heading must match before any button is trusted.
async function openNode(driver, label) {
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
  if (!nodeTarget) return { status: 'missing' };

  await driver.executeScript("arguments[0].scrollIntoView({block: 'center'})", nodeTarget);
  await driver.sleep(200);
  await driver.actions({ bridge: true }).move({ origin: nodeTarget }).click().perform();
  await driver.sleep(500);

  const expectedHeading = `ENG-${label}`;
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline) {
    const panel = await driver.executeScript(
      `
      const expectedHeading = arguments[0];
      const bodyText = document.body.innerText;
      if (!bodyText.includes(expectedHeading)) return null;
      const buttons = [...document.querySelectorAll('button')];
      const viewUnitList = buttons.find((button) => button.textContent.trim() === 'View Unit List');
      const viewCourseDetail = buttons.some((button) => button.textContent.trim() === 'View Course Detail');
      return { unfinished: Boolean(viewUnitList), completed: !viewUnitList && viewCourseDetail };
    `,
      expectedHeading
    );

    if (panel && panel.unfinished) {
      const buttons = await driver.findElements(By.css('button'));
      for (const button of buttons) {
        if ((await button.getText()).trim() !== 'View Unit List') continue;
        await driver.executeScript("arguments[0].scrollIntoView({block: 'center'})", button);
        await driver.sleep(500);
        await driver.actions({ bridge: true }).move({ origin: button }).click().perform();
        return { status: 'opened' };
      }
    }
    if (panel && panel.completed) return { status: 'completed' };
    await driver.sleep(200);
  }

  return { status: 'locked' };
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
    openNodeFn = openNode,
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
      logger.info('No unfinished courses left on this island — done', { completed });
      return { status: 'complete', completed };
    }
    attempted.add(next.label);

    logger.info('Inspecting course node', { label: next.label });
    const beforeUrl = await driver.getCurrentUrl();
    const opened = await openNodeFn(driver, next.label);
    if (opened.status === 'completed') {
      logger.info('Skipping completed course', { label: next.label });
      await driver.get(levelUrl);
      await driver.sleep(500);
      continue;
    }
    if (opened.status !== 'opened') {
      logger.warn('Course node is unavailable — stopping', { label: next.label, nodeStatus: opened.status });
      return { status: 'stuck', label: next.label, completed };
    }

    const navigated = await waitForUrlChangeFn(driver, beforeUrl, 10000);
    if (!navigated) {
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

module.exports = { run: main, readLevelNodes, openNode };
