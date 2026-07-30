const { extractInstructionText } = require('./_ltiQuiz');
const { getCurrentQuestionDom } = require('../browser');

const name = 'errorAnalysis';

const LETTERS = ['A', 'B', 'C', 'D', 'E', 'F'];

function stripTags(html) {
  return html
    .replace(/<[^>]+>/g, ' ')
    .replace(/&nbsp;/g, ' ')
    .replace(/​/g, '')
    .replace(/\s+/g, ' ')
    .trim();
}

function detect(dom) {
  return Boolean(dom.insideIframe) && dom.outerHTML.includes('quiz-input-radio');
}

function parse(dom) {
  const html = dom.outerHTML;

  // Candidate words are the underlined ones. Some questions wrap each in
  // <b><u>..</u></b>, others use a bare <u>..</u> with no <b> (verified live:
  // the "Although the intern tried..." A-E error-ID question) — so scan the
  // bold-wrapped underlines first, and if there are none, fall back to every
  // <u> in the markup. Either way the letter count then matches the
  // .quiz-input-radio options the brute-forcer clicks through.
  const boldMatches = [...html.matchAll(/<b>([\s\S]*?)<\/b>/g)].filter((m) => m[1].includes('<u>'));
  const sourceHtml = boldMatches.length ? boldMatches.map((m) => m[1]).join(' ') : html;

  // Candidate words are marked inline (e.g. "...help [D]they[/D] improve...")
  // instead of being reported as a bare word list — a plain word list loses
  // its position when the same word occurs more than once in the sentence
  // (verified live: "their" appearing in both "their job" and "their
  // skills", with only one of them the actual candidate), which left the
  // LLM unable to tell which occurrence a given letter referred to.
  let letterIndex = 0;
  const candidates = [];
  const markedHtml = sourceHtml.replace(/<u>([\s\S]*?)<\/u>/g, (_, inner) => {
    const word = stripTags(inner);
    if (!word) return inner;
    const letter = LETTERS[letterIndex];
    letterIndex += 1;
    candidates.push({ letter, word });
    return ` [${letter}]${word}[/${letter}] `;
  });

  // Some question templates mark candidates as absolutely-positioned overlay
  // labels on top of the sentence (data-options: "...label: A...", one per
  // .quiz-input-radio) instead of inline <u> tags — verified live: a
  // "By the time the guests arrived..." question had zero <u> anywhere in
  // its markup, which left candidates empty and the brute-forcer exhausted
  // on attempt 1 every time. The overlay word isn't recoverable without
  // real layout math, but the brute-forcer only needs the letters and how
  // many there are — one per radio input, same as the <u> path.
  //
  // Padded up to the real radio count, not just filled when zero <u> tags
  // were found — verified live: a question with 5 .quiz-input-radio options
  // had only 4 <u> tags in its markup (one candidate word wasn't underlined
  // at all), so candidates came out non-empty but one short. The
  // brute-forcer only ever tried A-D and exhausted every single attempt,
  // because E — the actual correct answer — was never a candidate to try.
  const radioCount = (html.match(/quiz-input-radio/g) || []).length;
  for (let i = candidates.length; i < radioCount; i += 1) candidates.push({ letter: LETTERS[i], word: '' });

  const sentence = stripTags(markedHtml);
  const instructionText = extractInstructionText(html);

  return { sentence, candidates, instructionText };
}

// This quiz sub-type only honors ONE click-through-Submit per page load —
// once Check My Answer has been clicked and graded, every radio input
// carries `disabled` and further clicks silently no-op (verified live: a
// stuck question already showed its Continue link with Check My Answer
// hidden — i.e. genuinely graded — yet re-clicking any radio, including via
// this exact script, changed nothing; the platform doesn't re-evaluate a
// locked question, unlike the mc/sa quiz types elsewhere in this codebase
// that do re-grade on every click). So a retry here reloads the activity
// and re-navigates back to the same question's hash first, to land on a
// fresh, actually-clickable radio group before trying the next letter —
// otherwise every attempt after the first silently resubmits whatever
// letter happened to lock in initially.
async function resetIfLocked(driver) {
  const locked = await driver.executeScript(`
    const inputs = document.querySelectorAll('.quiz-input-radio');
    return inputs.length > 0 && [...inputs].every((i) => i.disabled);
  `);
  if (!locked) return;

  const hash = await driver.executeScript('return window.location.hash');
  await driver.switchTo().defaultContent();
  await driver.navigate().refresh();
  await getCurrentQuestionDom(driver);
  if (hash) {
    await driver.executeScript('window.location.hash = arguments[0];', hash);
    await driver.sleep(1500);
  }
}

async function answer(driver, llmResult) {
  await resetIfLocked(driver);

  const requestedLetters = [...new Set(
    (Array.isArray(llmResult.answers) && llmResult.answers.length > 0 ? llmResult.answers : [llmResult.answer])
      .filter((letter) => LETTERS.includes(letter))
  )];
  const indexes = requestedLetters.map((letter) => LETTERS.indexOf(letter));

  // Polled instead of a single check — verified live: right after opening a
  // fresh activity, the very first attempt's radios weren't rendered yet,
  // threw here, and the brute-forcer's candidate index still advances on
  // any failure (it doesn't retry the same letter), permanently skipping
  // that letter for the rest of the question. That's how a genuinely
  // correct first letter (A) got silently skipped forever while B-E (all
  // wrong) were dutifully tried and exhausted.
  const clickScript = `
    const indexes = new Set(arguments[0]);
    const inputs = document.querySelectorAll('.quiz-input-radio');
    // ".quiz-input-radio" doesn't behave like an exclusive radio group here —
    // multiple checkmarks can stay visible at once. Select exactly requested
    // letters and clear every other checked option first.
    inputs.forEach((input, i) => {
      const shouldBeChecked = indexes.has(i);
      if (input.checked !== shouldBeChecked) input.click();
    });
    return [...indexes].every((index) => inputs[index]);
  `;
  let clicked = false;
  const deadline = Date.now() + 8000;
  while (Date.now() < deadline) {
    clicked = await driver.executeScript(clickScript, indexes);
    if (clicked) break;
    await driver.sleep(300);
  }

  if (!clicked) {
    throw new Error(`errorAnalysis.answer: no radio option found for requested letters "${requestedLetters.join(',')}"`);
  }

  await driver.sleep(100 + Math.random() * 100);

  await driver.executeScript(`
    const btn = document.querySelector('#quiz-submit-btn');
    if (btn && !btn.disabled) btn.click();
  `);
}

// The generic .correct.timeout/.incorrect.timeout markers waitForQuizFeedback
// watches aren't reliable here either (same unreliability already found and
// fixed for fillInBlank and quizMatching) — verified live: a genuinely
// correct answer (letter E, confirmed by .quiz-next-btn going visible and
// that radio staying checked/locked) still timed out waiting for either
// banner to show. .quiz-next-btn visibility is the only signal trusted for
// "correct"; anything else by the deadline is "incorrect" (a timeout, not
// an exception — resetIfLocked's reload-on-next-attempt already handles a
// wrong single-shot answer needing a fresh page for its next try). The
// submit click also occasionally doesn't register — a second click
// partway through the wait covers that, same fix as the other quiz types.
async function checkResult(dom) {
  const driver = dom.driver;
  const deadline = Date.now() + 5000;
  let retried = false;
  while (Date.now() < deadline) {
    const correct = await driver.executeScript(`
      const nextBtn = document.querySelector('.quiz-next-btn');
      if (nextBtn && getComputedStyle(nextBtn).display !== 'none') return true;
      return document.querySelectorAll('.quiz-input-radio').length === 0;
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
