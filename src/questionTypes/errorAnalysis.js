const { waitForQuizFeedback, extractInstructionText } = require('./_ltiQuiz');

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

  const sentence = stripTags(markedHtml);
  const instructionText = extractInstructionText(html);

  return { sentence, candidates, instructionText };
}

async function answer(driver, llmResult) {
  // Single radio group — always exactly one correct letter, so only the
  // first one the LLM names is ever clicked, regardless of whether it
  // ignores the single-answer instruction and returns more.
  const letter = llmResult.answer || (llmResult.answers && llmResult.answers[0]);
  const index = LETTERS.indexOf(letter);

  const clicked = await driver.executeScript(
    `
    const index = arguments[0];
    const inputs = document.querySelectorAll('.quiz-input-radio');
    // ".quiz-input-radio" doesn't behave like an exclusive radio group here —
    // a retry's click leaves the previous letter still checked (verified
    // live: multiple checkmarks visible at once) — so any other checked
    // option is explicitly unclicked before the new one is picked.
    inputs.forEach((input, i) => {
      if (i !== index && input.checked) input.click();
    });
    const target = inputs[index];
    if (target) { if (!target.checked) target.click(); return true; }
    return false;
  `,
    index
  );

  if (!clicked) {
    throw new Error(`errorAnalysis.answer: no radio option found for letter "${letter}"`);
  }

  await driver.sleep(2000 + Math.random() * 1000);

  await driver.executeScript(`
    const btn = document.querySelector('#quiz-submit-btn');
    if (btn && !btn.disabled) btn.click();
  `);
}

async function checkResult(dom) {
  return waitForQuizFeedback(dom.driver);
}

module.exports = { name, detect, parse, answer, checkResult };
