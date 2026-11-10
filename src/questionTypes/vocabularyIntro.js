const name = 'vocabularyIntro';

// Quiz pages also have a hidden #/n Continue, so exclude them. Intro slides embed a quiz
// manifest with quiz-matching ids, so use quizMatching's full condition.
function detect(dom) {
  if (!dom.insideIframe) return false;
  const html = dom.outerHTML;
  if (html.includes('quiz-input-radio') || html.includes('quiz-input-sa')) return false;
  if (html.includes('quiz-matching') && html.includes('dropzone') && html.includes('draggable')) return false;
  return /href="#\/n"/.test(html);
}

function parse() {
  return {};
}

async function answer(driver) {
  const clicked = await driver.executeScript(`
    const link = document.querySelector('a[href="#/n"]');
    if (link) { link.click(); return true; }
    return false;
  `);
  if (!clicked) {
    throw new Error('vocabularyIntro.answer: no "#/n" next link found');
  }
  await driver.sleep(100 + Math.random() * 100);
}

async function checkResult() {
  return 'correct';
}

module.exports = { name, detect, parse, answer, checkResult };
