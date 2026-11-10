const MAX_LOG_LINES = 200;

// @opentui/core is ESM under Bun, so it needs a dynamic import() from CommonJS.
async function createDashboard({ title }) {
  const { createCliRenderer, BoxRenderable, TextRenderable, ScrollBoxRenderable } = await import('@opentui/core');
  const renderer = await createCliRenderer({ exitOnCtrlC: true });

  // Explicit colors keep text readable on light terminal themes.
  const header = new TextRenderable(renderer, {
    id: 'header',
    content: title,
    fg: '#fabd2f',
    bg: '#1d2021',
  });

  const stats = new TextRenderable(renderer, {
    id: 'stats',
    content: 'Questions: 0   Correct: 0   Incorrect: 0',
    fg: '#ebdbb2',
    bg: '#1d2021',
  });

  const logBox = new ScrollBoxRenderable(renderer, {
    id: 'log-box',
    flexGrow: 1,
    border: true,
    title: 'Log',
    borderColor: '#928374',
    backgroundColor: '#1d2021',
  });

  const logText = new TextRenderable(renderer, { id: 'log-text', content: '', fg: '#ebdbb2', bg: '#1d2021' });
  logBox.add(logText);

  const root = new BoxRenderable(renderer, {
    id: 'root',
    width: '100%',
    height: '100%',
    flexDirection: 'column',
    padding: 1,
    backgroundColor: '#1d2021',
  });
  root.add(header);
  root.add(stats);
  root.add(logBox);
  renderer.root.add(root);

  const lines = [];
  const counts = { questions: 0, correct: 0, incorrect: 0 };

  function pushLine(level, message, meta) {
    const metaStr = meta && Object.keys(meta).length ? ' ' + JSON.stringify(meta) : '';
    lines.push(`[${level}] ${message}${metaStr}`);
    if (lines.length > MAX_LOG_LINES) lines.shift();
    logText.content = lines.join('\n');
    logBox.scrollTop = Number.MAX_SAFE_INTEGER;
  }

  function updateStats(meta) {
    if (meta && meta.status === 'correct') counts.correct += 1;
    if (meta && meta.status === 'incorrect') counts.incorrect += 1;
    if (meta && typeof meta.questionNum === 'number') counts.questions = Math.max(counts.questions, meta.questionNum);
    stats.content = `Questions: ${counts.questions}   Correct: ${counts.correct}   Incorrect: ${counts.incorrect}`;
  }

  function onLog(level, message, meta) {
    pushLine(level, message, meta);
    updateStats(meta);
  }

  renderer.start();

  function stop() {
    renderer.stop();
  }

  return { onLog, stop };
}

module.exports = { createDashboard };
