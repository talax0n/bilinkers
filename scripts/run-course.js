const { Builder } = require('selenium-webdriver');
const chrome = require('selenium-webdriver/chrome');
const { resolveChromedriverPath } = require('../src/chromedriver');
const logger = require('../src/logger');
const { run: runUnit } = require('./run-unit');
const { run: runCheckpoint } = require('./run-checkpoint');

// This script attaches to an already-running Chromium-based browser and
// drives a whole course level's roadmap (see the isometric city map
// screenshot: a zig-zag road of hexagon nodes — "Unit 1", "Unit 2",
// "Checkpoint 1", "Unit 3", ... — with a header readout "Unit 4/8  CP 1/3
// FT 0/1") end to end: open the next unfinished Unit or Checkpoint, run the
// matching bot (run-unit.js or run-checkpoint.js) on the same browser
// session, come back to the map, repeat. Run via `node scripts/cli.js
// course`, or invoke this file directly if the browser is already up and
// sitting on a course's roadmap page (the URL with no /session/{id} suffix).
// Final Test is deliberately never opened — excluded by construction (the
// node whitelist below only ever matches "Unit N" / "Checkpoint N").
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

// Each roadmap node is a rasterized PNG (hexagon colour/lock-state/checkmark
// baked into the image itself, verified live: no colour or state ever shows
// up as a class or attribute) sitting inside a ".hoverable-pointer" div, with
// the plain-text label ("Unit8", "Checkpoint3", ...) as a sibling text node
// in the same container — so state can't be read from the DOM directly.
// Decoding it isn't needed though: the header readout ("Unit 4/8  CP 1/3")
// already gives exact completed counts, and course order is fixed and
// derivable from each node's absolute "top" pixel position (verified live:
// sorting descending by top produced exactly Unit1, Unit2, Checkpoint1,
// Unit3, Unit4, Checkpoint2, Unit5, Unit6, Checkpoint3, Unit7, Unit8 — which
// matches the roadmap screenshot's visual order and the header's progress
// counts). Locked nodes are still safe to click "by mistake": verified live,
// clicking a locked node's URL never changes, which the driving loop below
// already treats as "nothing more is reachable, stop".
async function readCourseNodes(driver) {
  await driver.switchTo().defaultContent();
  const nodes = await driver.executeScript(`
    return [...document.querySelectorAll('.hoverable-pointer')].map((el) => {
      const label = el.textContent.replace(/\\s+/g, '').trim();
      const positioned = el.querySelector('[style*="top"]');
      const top = positioned ? parseFloat(positioned.style.top) : NaN;
      return { label, top };
    });
  `);

  // Whitelisted, not blacklisted: only "Unit N" / "Checkpoint N" labels are
  // ever recognised, so a "Final Test" node (or anything else) is excluded
  // by construction regardless of what it's labelled or whether it even
  // exists in the DOM yet.
  const parsed = nodes
    .map((n) => {
      const unitMatch = n.label.match(/^Unit(\d+)$/i);
      if (unitMatch) return { type: 'unit', number: Number(unitMatch[1]), top: n.top, label: n.label };
      const cpMatch = n.label.match(/^Checkpoint(\d+)$/i);
      if (cpMatch) return { type: 'checkpoint', number: Number(cpMatch[1]), top: n.top, label: n.label };
      return null;
    })
    .filter(Boolean);

  // Descending top = ascending course order (verified live — see comment
  // above).
  parsed.sort((a, b) => b.top - a.top);
  return parsed;
}

async function readProgress(driver) {
  await driver.switchTo().defaultContent();
  return driver.executeScript(`
    const text = document.body.innerText;
    const parse = (re) => {
      const m = text.match(re);
      return m ? { done: Number(m[1]), total: Number(m[2]) } : null;
    };
    return {
      unit: parse(/Unit\\s*(\\d+)\\s*\\/\\s*(\\d+)/),
      checkpoint: parse(/CP\\s*(\\d+)\\s*\\/\\s*(\\d+)/),
    };
  `);
}

async function clickNode(driver, label) {
  await driver.switchTo().defaultContent();
  return driver.executeScript(
    `
    const target = String(arguments[0]);
    const nodes = [...document.querySelectorAll('.hoverable-pointer')];
    const el = nodes.find((n) => n.textContent.replace(/\\s+/g, '').trim() === target);
    if (el) { el.click(); return true; }
    return false;
  `,
    label
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

// existingDriver lets a caller (or a future higher-level orchestrator) drive
// one continuous browser session; standalone invocation still attaches its
// own as before. The rest are injectable for testing without a real driver.
async function main(
  existingDriver,
  {
    readCourseNodesFn = readCourseNodes,
    readProgressFn = readProgress,
    clickNodeFn = clickNode,
    waitForUrlChangeFn = waitForUrlChange,
    runUnitFn = runUnit,
    runCheckpointFn = runCheckpoint,
    readRetryMs = 5000,
  } = {}
) {
  const driver = existingDriver || (await attachToBrave());
  const courseUrl = await driver.getCurrentUrl();

  const attempted = new Set();

  for (;;) {
    // The roadmap is still loading for a beat right after navigating back to
    // it (live-verified: a fixed 1500ms post-navigation sleep wasn't always
    // enough — an immediate read here caught 0 nodes and null progress even
    // though the same page read fine moments later), so a single miss isn't
    // trusted as "genuinely missing" — poll for a few seconds first.
    const readDeadline = Date.now() + readRetryMs;
    let nodes;
    let progress;
    for (;;) {
      [nodes, progress] = await Promise.all([readCourseNodesFn(driver), readProgressFn(driver)]);
      const ok = Array.isArray(nodes) && nodes.length && progress && progress.unit && progress.checkpoint;
      if (ok || Date.now() > readDeadline) break;
      await driver.sleep(300);
    }
    if (!Array.isArray(nodes) || !nodes.length || !progress || !progress.unit || !progress.checkpoint) {
      logger.warn('Course roadmap or progress could not be read — stopping', { nodes: Array.isArray(nodes) ? nodes.length : null, progress });
      return { status: 'stuck', reason: 'missing-roadmap' };
    }
    const unitsDone = progress.unit ? progress.unit.done : 0;
    const checkpointsDone = progress.checkpoint ? progress.checkpoint.done : 0;

    const next = nodes.find((n) => {
      if (attempted.has(n.label)) return false;
      const done = n.type === 'unit' ? unitsDone : checkpointsDone;
      return n.number > done;
    });

    if (!next) {
      logger.info('No unfinished units or checkpoints left in this course — done', {
        unit: progress.unit,
        checkpoint: progress.checkpoint,
      });
      return { status: 'complete', unit: progress.unit, checkpoint: progress.checkpoint };
    }
    attempted.add(next.label);

    logger.info('Opening course node', { label: next.label, type: next.type });
    const beforeUrl = await driver.getCurrentUrl();
    const clicked = await clickNodeFn(driver, next.label);
    if (!clicked) {
      logger.warn('Could not click course node — stopping', { label: next.label });
      return { status: 'stuck', label: next.label };
    }

    const navigated = await waitForUrlChangeFn(driver, beforeUrl, 10000);
    if (!navigated) {
      // Either genuinely locked (the frontier the header counters implied
      // doesn't match what the roadmap will actually let us open — the
      // platform's own lock rules win), or a click landed on the road/deco
      // image instead of a real node. Either way nothing more is reachable
      // right now.
      logger.warn('Course node did not open (URL never changed) — stopping', { label: next.label });
      return { status: 'stuck', label: next.label };
    }
    await driver.sleep(1500);

    const runner = next.type === 'unit' ? runUnitFn : runCheckpointFn;
    logger.info('Running course node', { label: next.label, type: next.type });
    const result = await runner(driver);
    logger.info('Course node finished', { label: next.label, type: next.type, ...result });

    const badStatus = next.type === 'unit' ? result.status === 'stuck' : result.status === 'unhandled' || result.status === 'no-gate';
    if (badStatus) {
      logger.warn('Course node did not finish cleanly — stopping the course run so it can be inspected', {
        label: next.label,
        type: next.type,
        status: result.status,
      });
      return { status: 'stuck', label: next.label, nodeResult: result };
    }

    await driver.switchTo().defaultContent();
    await driver.get(courseUrl);
    await driver.sleep(1500);
  }
}

if (require.main === module) {
  main().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}

module.exports = { run: main, readCourseNodes, readProgress };
