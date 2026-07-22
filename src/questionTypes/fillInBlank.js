const { waitForQuizFeedback, extractInstructionText } = require('./_ltiQuiz');

const name = 'fillInBlank';

function stripTags(html) {
  return html
    .replace(/<[^>]+>/g, ' ')
    .replace(/&nbsp;/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
}

function detect(dom) {
  return Boolean(dom.insideIframe) && dom.outerHTML.includes('quiz-input-sa');
}

function parse(dom) {
  const html = dom.outerHTML;

  const sentenceMatch = html.match(/<strong>([^<]*_{2,}[^<]*)<\/strong>/);
  const sentence = sentenceMatch ? stripTags(sentenceMatch[1]) : '';
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
