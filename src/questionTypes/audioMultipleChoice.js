const { stripTags, parseOptions, selectOptionAndCheck, waitForCheckFeedback } = require('./_optionButtons');

const name = 'audioMultipleChoice';

function detect(dom) {
  const html = dom.outerHTML;
  // A stale (visually hidden) slider from an earlier listening question can
  // still be present in outerHTML — dom.hasVisibleSlider checks it's
  // actually on-screen instead of just anywhere in the markup. Falls back
  // to the substring check when hasVisibleSlider isn't set (e.g. a fixture
  // dom without live visibility info).
  const hasSlider = dom.hasVisibleSlider !== undefined ? dom.hasVisibleSlider : html.includes('MuiSlider-root');
  return hasSlider && html.includes('bl-w-full justify-content-start');
}

function parse(dom) {
  const html = dom.outerHTML;

  const rtfClassIdx = html.indexOf('bl-rtf undefined');
  const contentStart = html.indexOf('>', rtfClassIdx) + 1;
  const rtfEnd = html.indexOf('</p></div>', contentStart);
  const instructionBlock = html.slice(contentStart, rtfEnd);

  const instructionMatch = instructionBlock.match(/<strong>([\s\S]*?)<\/strong>/);
  const instruction = instructionMatch ? stripTags(instructionMatch[1]) : '';

  const afterStrong = instructionMatch ? instructionBlock.slice(instructionMatch.index + instructionMatch[0].length) : instructionBlock;
  const questionMatch = afterStrong.match(/<p>([\s\S]*?)<\/p>/);
  const questionText = questionMatch ? stripTags(questionMatch[1]) : stripTags(instructionBlock);

  return { instruction, questionText, options: parseOptions(html) };
}

async function answer(driver, llmResult) {
  await selectOptionAndCheck(driver, llmResult.answer);
}

async function checkResult(dom) {
  return waitForCheckFeedback(dom.driver);
}

module.exports = { name, detect, parse, answer, checkResult };
