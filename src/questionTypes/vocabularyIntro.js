const name = 'vocabularyIntro';

// Vocabulary-unit "presentation" pages (word cards, meaning/example cards,
// instruction screens) sit between the quiz pages inside the same iframe SPA.
// They're not graded and have no options to pick — just a right-arrow link
// (href "#/n") that walks the built-in slide sequence forward. The visible
// "Continue" button on these pages sometimes carries a stale href (equal to
// the page's own hash, a leftover from the authoring template) that never
// advances, so "#/n" is used instead since it reliably moves to the next
// slide (verified live: word card -> meaning/example card -> next word ...).
//
// Quiz pages (quiz-matching in particular) also carry an "#/n" link — it's
// their post-answer Continue button, just hidden (style="display:none")
// until answered — so matching on the href alone previously misfired on
// real questions and skipped them unanswered. Excluding known quiz markers
// closes that gap.
function detect(dom) {
  if (!dom.insideIframe) return false;
  const html = dom.outerHTML;
  if (html.includes('quiz-input-radio') || html.includes('quiz-input-sa') || html.includes('quiz-matching')) return false;
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
  await driver.sleep(900 + Math.random() * 600);
}

async function checkResult() {
  return 'correct';
}

module.exports = { name, detect, parse, answer, checkResult };
