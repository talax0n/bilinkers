const { extractInstructionText } = require('./_ltiQuiz');

const name = 'quizMatching';

function detect(dom) {
  if (!dom.insideIframe) return false;
  const html = dom.outerHTML;
  return html.includes('quiz-matching') && html.includes('dropzone') && html.includes('draggable');
}

function parse(dom) {
  return { instructionText: extractInstructionText(dom.outerHTML) };
}

// Synthetic drags don't register with interact.js. Grading only compares data-filled
// to data-drag-target, so write that end state directly.
async function answer(driver) {
  const filled = await driver.executeScript(`
    let count = 0;
    document.querySelectorAll('.quiz-matching').forEach((quiz) => {
      quiz.querySelectorAll(':scope > .dropzone').forEach((dropzone) => {
        const target = dropzone.getAttribute('data-drag-target');
        const dragEl = quiz.querySelector('.draggable[data-drag="' + target + '"]');
        if (!dragEl) return;
        const dz = dropzone.style;
        const de = dragEl.style;
        de.left = (parseFloat(dz.left) + (parseFloat(dz.width) - parseFloat(de.width)) / 2) + '%';
        de.top = (parseFloat(dz.top) + (parseFloat(dz.height) - parseFloat(de.height)) / 2) + '%';
        dropzone.setAttribute('data-filled', target);
        count += 1;
      });
    });
    const btn = document.querySelector('#quiz-submit-btn');
    if (btn) btn.removeAttribute('disabled');
    return count;
  `);

  if (!filled) {
    throw new Error('quizMatching.answer: no dropzone/draggable pairs found to fill');
  }

  await driver.sleep(100 + Math.random() * 100);

  await driver.executeScript(`
    const btn = document.querySelector('#quiz-submit-btn');
    if (btn && !btn.disabled) btn.click();
  `);
}

// The banners often stay hidden, so a visible .quiz-next-btn is the signal.
// Submit is re-clicked once in case the first click dropped.
async function checkResult(dom) {
  const driver = dom.driver;
  const deadline = Date.now() + 20000;
  let retried = false;
  while (Date.now() < deadline) {
    const state = await driver.executeScript(`
      const nextBtn = document.querySelector('.quiz-next-btn');
      if (nextBtn && getComputedStyle(nextBtn).display !== 'none') return 'correct';
      const incorrect = document.querySelector('.incorrect.timeout, .incorrect');
      if (incorrect && getComputedStyle(incorrect).display !== 'none') return 'incorrect';
      return null;
    `);
    if (state) return state;
    if (!retried && Date.now() > deadline - 2500) {
      retried = true;
      await driver.executeScript(`
        const btn = document.querySelector('#quiz-submit-btn');
        if (btn && !btn.disabled) btn.click();
      `);
    }
    await driver.sleep(200);
  }
  throw new Error('quizMatching checkResult: timed out waiting for Continue link or incorrect feedback');
}

module.exports = { name, detect, parse, answer, checkResult };
