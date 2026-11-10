// Author instructions are always styled in this teal in the Bits editor, whatever the wording.
function extractInstructionText(html) {
  const matches = [...html.matchAll(/color:rgb\(26, 106, 132\)">([\s\S]*?)<\/span>/g)];
  const stripTags = (fragment) =>
    fragment
      .replace(/<[^>]+>/g, ' ')
      .replace(/&nbsp;/g, ' ')
      .replace(/​/g, '')
      .replace(/\s+/g, ' ')
      .trim();
  return matches
    .map((m) => stripTags(m[1]))
    .filter(Boolean)
    .join(' ')
    .trim();
}

async function waitForQuizFeedback(driver) {
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline) {
    const state = await driver.executeScript(`
      const correct = document.querySelector('.correct.timeout');
      const incorrect = document.querySelector('.incorrect.timeout');
      const correctVisible = correct && getComputedStyle(correct).display !== 'none';
      const incorrectVisible = incorrect && getComputedStyle(incorrect).display !== 'none';
      if (correctVisible) return 'correct';
      if (incorrectVisible) return 'incorrect';
      return null;
    `);
    if (state) return state;
    await driver.sleep(200);
  }
  throw new Error('waitForQuizFeedback: timed out waiting for correct/incorrect feedback');
}

module.exports = { waitForQuizFeedback, extractInstructionText };
