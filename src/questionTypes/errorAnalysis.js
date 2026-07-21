const { waitForQuizFeedback } = require('./_ltiQuiz');

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

  const underlineMatches = [...html.matchAll(/<u>([\s\S]*?)<\/u>/g)];
  const words = underlineMatches.map((m) => stripTags(m[1])).filter((w) => w.length > 0);

  const candidates = words.map((word, i) => ({ letter: LETTERS[i], word }));
  const boldMatches = [...html.matchAll(/<b>([\s\S]*?)<\/b>/g)].filter((m) => m[1].includes('<u>'));
  const sentence = stripTags(boldMatches.map((m) => m[1]).join(' '));

  return { sentence, candidates };
}

async function answer(driver, llmResult) {
  const letter = llmResult.answer;
  const index = LETTERS.indexOf(letter);

  const clicked = await driver.executeScript(
    `
    const index = arguments[0];
    const inputs = document.querySelectorAll('.quiz-input-radio');
    const target = inputs[index];
    if (!target) return false;
    target.click();
    return true;
  `,
    index
  );

  if (!clicked) {
    throw new Error(`errorAnalysis.answer: no radio option found for letter "${letter}"`);
  }

  await driver.executeScript(`
    const btn = document.querySelector('#quiz-submit-btn');
    if (btn && !btn.disabled) btn.click();
  `);
}

async function checkResult(dom) {
  return waitForQuizFeedback(dom.driver);
}

module.exports = { name, detect, parse, answer, checkResult };
