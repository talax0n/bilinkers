const { attachToLms, waitForUrlChange } = require('../browser');
const logger = require('../logger');
const { run: runUnit } = require('./unit');
const { run: runCheckpoint } = require('./checkpoint');

// Site behavior this relies on: docs/platform-notes.md#course-roadmap

// Node state is baked into PNGs. Order comes from each node's top offset.
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

  // Whitelist, so the Final Test node is never opened.
  const parsed = nodes
    .map((n) => {
      const unitMatch = n.label.match(/^Unit(\d+)$/i);
      if (unitMatch) return { type: 'unit', number: Number(unitMatch[1]), top: n.top, label: n.label };
      const cpMatch = n.label.match(/^Checkpoint(\d+)$/i);
      if (cpMatch) return { type: 'checkpoint', number: Number(cpMatch[1]), top: n.top, label: n.label };
      return null;
    })
    .filter(Boolean);

  // Descending top is course order.
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
  const driver = existingDriver || (await attachToLms());
  const courseUrl = await driver.getCurrentUrl();

  const attempted = new Set();

  for (;;) {
    // The roadmap may still be loading after navigating back, so poll before giving up.
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
      // Locked node, or the click hit decoration. Nothing more is reachable.
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
