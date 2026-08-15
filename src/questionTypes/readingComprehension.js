const { stripTags, parseOptions, selectOptionAndCheck, waitForCheckFeedback } = require('./_optionButtons');

const name = 'readingComprehension';

function detect(dom) {
  const html = dom.outerHTML;
  // See audioMultipleChoice.detect: a stale hidden slider from an earlier
  // listening question must not disqualify a real, current mcq/reading
  // question just because its markup lingers in outerHTML.
  const hasSlider = dom.hasVisibleSlider !== undefined ? dom.hasVisibleSlider : html.includes('MuiSlider-root');
  return !hasSlider && html.includes('bl-w-full justify-content-start');
}

function parse(dom) {
  const html = dom.outerHTML;

  const rtfClassIdx = html.indexOf('bl-rtf undefined');
  const contentStart = html.indexOf('>', rtfClassIdx) + 1;
  const rtfEnd = html.indexOf('</p></div>', contentStart);
  const block = html.slice(contentStart, rtfEnd);

  const text = stripTags(block);

  return { text, options: parseOptions(html) };
}

async function answer(driver, llmResult) {
  await selectOptionAndCheck(driver, llmResult.answer);
}

async function checkResult(dom) {
  return waitForCheckFeedback(dom.driver);
}

module.exports = { name, detect, parse, answer, checkResult };
