const OPTION_BUTTON_SELECTOR = 'button.bl-w-full.justify-content-start';

function stripTags(html) {
  return html
    .replace(/<[^>]+>/g, ' ')
    .replace(/&nbsp;/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
}

function parseOptions(html) {
  const letters = ['A', 'B', 'C', 'D', 'E', 'F'];
  const buttonRegex = new RegExp(`<button class="MuiButtonBase-root bl-w-full justify-content-start[^"]*"[\\s\\S]*?<\\/button>`, 'g');
  const options = [];
  let match;
  let i = 0;
  while ((match = buttonRegex.exec(html)) !== null) {
    const rtfMatch = match[0].match(/bl-rtf undefined"[^>]*>([\s\S]*?)<\/p><\/div>/);
    options.push({ letter: letters[i], text: rtfMatch ? stripTags(rtfMatch[1]) : '' });
    i += 1;
  }
  return options;
}

async function selectOptionAndCheck(driver, letter) {
  const clicked = await driver.executeScript(
    `
    const letter = arguments[0];
    const buttons = document.querySelectorAll('${OPTION_BUTTON_SELECTOR}');
    const target = Array.from(buttons).find((btn) => btn.textContent.trim().startsWith(letter));
    if (!target) return false;
    target.click();
    return true;
  `,
    letter
  );

  if (!clicked) {
    throw new Error(`selectOptionAndCheck: no option button found for letter "${letter}"`);
  }

  await driver.executeScript(`
    const buttons = Array.from(document.querySelectorAll('button'));
    const btn = buttons.find((b) => b.textContent.trim() === 'Check');
    if (btn) btn.click();
  `);
}

// Returns { outcome, hint }, not a bare string: an incorrect answer here
// (verified live on a True/False/Not-Given reading question) comes with a
// specific explanation right next to "Incorrect!" — e.g. "Revisit the
// passage to determine if Jenna's role in organizing the pantry was
// mentioned." — sitting in a sibling div right after the heading's own
// wrapper. Surfacing it lets a retry attempt be told exactly what to
// reconsider instead of just "try again", which otherwise tends to produce
// the same guess repeatedly on subtler true/false/not-given judgment calls.
async function waitForCheckFeedback(driver) {
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline) {
    const state = await driver.executeScript(`
      const headings = Array.from(document.querySelectorAll('h5'));
      const heading = headings.find((h) => h.textContent.trim() === 'Correct!' || h.textContent.trim() === 'Incorrect!');
      if (!heading) return null;
      const hintEl = heading.parentElement && heading.parentElement.nextElementSibling;
      return {
        outcome: heading.textContent.trim(),
        hint: hintEl ? hintEl.textContent.trim() : null,
      };
    `);
    if (state && state.outcome === 'Correct!') return { outcome: 'correct', hint: null };
    if (state && state.outcome === 'Incorrect!') return { outcome: 'incorrect', hint: state.hint || null };
    await driver.sleep(200);
  }
  throw new Error('waitForCheckFeedback: timed out waiting for Correct!/Incorrect! feedback');
}

module.exports = { stripTags, parseOptions, selectOptionAndCheck, waitForCheckFeedback, OPTION_BUTTON_SELECTOR };
