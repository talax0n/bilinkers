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

  // Candidates are <b><u> words on some pages and bare <u> on others.
  const boldMatches = [...html.matchAll(/<b>([\s\S]*?)<\/b>/g)].filter((m) => m[1].includes('<u>'));
  const sourceHtml = boldMatches.length ? boldMatches.map((m) => m[1]).join(' ') : html;

  // Mark candidates inline so a repeated word keeps its position.
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

  // Some pages mark candidates with overlay labels or skip a <u>, so pad to the radio count.
  const radioCount = (html.match(/quiz-input-radio/g) || []).length;
  for (let i = candidates.length; i < radioCount; i += 1) candidates.push({ letter: LETTERS[i], word: '' });

  const sentence = stripTags(markedHtml);
  const instructionText = extractInstructionText(html);

  return { sentence, candidates, instructionText };
}

// Radios lock after one graded submit per page load. Reload and restore the hash to retry.
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

  // Radios may not be rendered yet on the first attempt, and a throw would skip this letter.
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

// Only a visible .quiz-next-btn is trusted. The banners are unreliable and Submit clicks sometimes drop.
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
