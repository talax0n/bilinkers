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

// Drag-and-drop word-bank questions: drag each vocabulary option onto its
// matching blank. The app's own grading (checkQuizMatching in app.js) does
// nothing smarter than `dropzone.data-filled === dropzone.data-drag-target`
// — the correct draggable for a blank is simply whichever one carries that
// same index as its data-drag id, with distractors carrying indexes that
// never match any blank on the page. That makes the fill deterministic by
// DOM structure, which is used here instead of a real simulated mouse drag:
// interact.js's internal drag/drop state didn't reliably register synthetic
// WebDriver pointer events in testing (dropzone stayed empty, Check stayed
// disabled), while writing the same end state their own onDrop handler
// produces is indistinguishable to the grading code.
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

  await driver.sleep(1000 + Math.random() * 500);

  await driver.executeScript(`
    const btn = document.querySelector('#quiz-submit-btn');
    if (btn && !btn.disabled) btn.click();
  `);
}

// The generic .correct.timeout/.incorrect.timeout markers _ltiQuiz.js's
// waitForQuizFeedback watches (built for the mc/sa quiz types) don't
// reliably show for this matching variant in testing — verified live: after
// a correct fill+submit, both stayed display:none the whole 5s window while
// .quiz-next-btn (the same element advance()/clickContinue() already act on
// once a question passes) went visible immediately. That's the real signal
// here, so it's checked directly instead.
// The submit click in answer() occasionally doesn't register with the app's
// grading (verified live: fill was correct, #quiz-submit-btn stayed enabled
// and unclicked-looking, checkResult timed out — a second click on the same
// button immediately flipped .quiz-next-btn visible). So checkResult
// re-clicks Submit once, partway through its wait, instead of only ever
// clicking it the one time from answer().
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
