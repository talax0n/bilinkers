const fs = require('node:fs');

function loadProgress(filePath) {
  if (!fs.existsSync(filePath)) {
    return { lessonIndex: 0, sectionIndex: 0 };
  }
  try {
    return JSON.parse(fs.readFileSync(filePath, 'utf8'));
  } catch {
    return { lessonIndex: 0, sectionIndex: 0 };
  }
}

function saveProgress(filePath, progress) {
  fs.writeFileSync(filePath, JSON.stringify(progress, null, 2));
}

module.exports = { loadProgress, saveProgress };
