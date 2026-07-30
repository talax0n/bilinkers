const { extractInstructionText } = require('./_ltiQuiz');

const name = 'fillInBlank';

function stripTags(html) {
  return html
    .replace(/<[^>]+>/g, ' ')
    .replace(/&nbsp;/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
}

function extractSentence(html) {
  const blockRegex = /<(p|div|li|td)\b[^>]*>([\s\S]*?)<\/\1>/gi;

  for (const match of html.matchAll(blockRegex)) {
    const text = stripTags(match[2]);
    if (!/_{2,}/.test(text)) continue;
    if (!/<(?:strong|b)\b/i.test(match[2])) continue;
    return text;
  }

  const boldRegex = /<(strong|b)\b[^>]*>([\s\S]*?)<\/\1>/gi;
  const chunks = Array.from(html.matchAll(boldRegex))
    .map((match) => stripTags(match[2]))
    .filter(Boolean);

  const blankIndex = chunks.findIndex((chunk) => /_{2,}/.test(chunk));
  if (blankIndex === -1) return '';

  const parts = [chunks[blankIndex]];
  const nextChunk = chunks[blankIndex + 1];
  if (nextChunk && /^[([]/.test(nextChunk)) parts.push(nextChunk);

  return parts.join(' ');
}

function detect(dom) {
  return Boolean(dom.insideIframe) && dom.outerHTML.includes('quiz-input-sa');
}

function parse(dom) {
  const html = dom.outerHTML;

  const sentence = extractSentence(html);
  const fullText = stripTags(html);
  const blankCount = (html.match(/quiz-input-sa/g) || []).length;
  const instructionText = extractInstructionText(html);

  return { sentence, fullText, blankCount, instructionText };
}

async function answer(driver, llmResult) {
  const answers = llmResult.answers || [llmResult.answer];

  await driver.executeScript(
    `
    const values = arguments[0];
    const inputs = document.querySelectorAll('.quiz-input-sa');
    inputs.forEach((input, i) => {
      const val = values[i] !== undefined ? values[i] : values[0];
      input.value = val;
      input.dispatchEvent(new Event('input', { bubbles: true }));
      input.dispatchEvent(new Event('change', { bubbles: true }));
    });
  `,
    answers
  );

  await driver.sleep(100 + Math.random() * 100);

  await driver.executeScript(`
    const btn = document.querySelector('#quiz-submit-btn');
    if (btn && !btn.disabled) btn.click();
  `);
}

// The generic .correct.timeout/.incorrect.timeout markers _ltiQuiz.js's
// waitForQuizFeedback watches don't reliably reflect this "sa" (short
// answer) quiz type's actual result — verified live: a genuinely correct
// answer ('were rainy', confirmed by the platform's own "Continue" link
// going live and the input staying filled with it) still flashed the
// .incorrect banner for ~2s on the way there, indistinguishable in timing
// from a truly wrong answer's flash. So the incorrect banner is treated as
// pure noise here, not a signal — .quiz-next-btn visibility (the same
// element advance()/clickContinue() already act on once a question passes)
// is the only thing trusted as "correct"; anything else by the deadline is
// "incorrect" (a timeout, not an exception — this quiz type has no reliable
// live signal for a wrong answer, only the absence of the right one). The
// submit click also occasionally doesn't register with the app's grading —
// a second click on the same button partway through the wait covers that,
// same fix already applied to quizMatching for the same click race.
async function checkResult(dom) {
  const driver = dom.driver;
  const deadline = Date.now() + 5000;
  let retried = false;
  while (Date.now() < deadline) {
    const correct = await driver.executeScript(`
      const nextBtn = document.querySelector('.quiz-next-btn');
      return !!nextBtn && getComputedStyle(nextBtn).display !== 'none';
    `);
    if (correct) return 'correct';
    if (!retried && Date.now() > deadline - 2500) {
      retried = true;
      await driver.executeScript(`
        const btn = document.querySelector('#quiz-submit-btn');
        if (btn && !btn.disabled) btn.click();
      `);
    }
    await driver.sleep(200);
  }
  return 'incorrect';
}

module.exports = { name, detect, parse, answer, checkResult };
