function isGroupedReadingLayout({ pills = [], optionLetters = [] }) {
  const numericPills = new Set(pills.filter((pill) => /^\d+$/.test(String(pill).trim())));
  const groupCount = optionLetters.filter((letter) => String(letter).trim() === 'A').length;
  return numericPills.size >= 2 && groupCount >= 2;
}

function groupedReadingLayoutFromDocument() {
  const pills = [...document.querySelectorAll('.bl-button__container')].map((pill) => pill.textContent.trim());
  const optionLetters = [...document.querySelectorAll('button.bl-w-full.justify-content-start')]
    .filter((button) => button.offsetParent !== null)
    .map((button) => button.textContent.trim()[0]);
  return { pills, optionLetters };
}

module.exports = { isGroupedReadingLayout, groupedReadingLayoutFromDocument };
