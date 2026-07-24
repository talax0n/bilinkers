const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { test } = require('node:test');
const { leaveByNextPill, answerOpenGroups } = require('../scripts/run-reading-exercise');

test('deferred grouped-reading traversal leaves each pill through the next numbered pill, including the final pill', async () => {
  const visits = [];
  const submissions = [];
  const driver = {
    async executeScript(_code, pill) {
      visits.push(Number(pill));
      return true;
    },
    async sleep() {},
  };

  visits.push(1);
  await leaveByNextPill(driver, [1, 2, 3], 1);
  await leaveByNextPill(driver, [1, 2, 3], 2);
  await leaveByNextPill(driver, [1, 2, 3], 3);
  submissions.push([...visits]);

  assert.deepEqual(visits, [1, 2, 3, 1]);
  assert.deepEqual(submissions, [[1, 2, 3, 1]], 'the final pill is left before submit');
});

test('deferred grouped-reading path contains no Save or generic Next progression controls', () => {
  const source = fs.readFileSync(path.join(__dirname, '../scripts/run-reading-exercise.js'), 'utf8');

  assert.equal(source.includes('gradeCurrent'), false);
  assert.equal(source.includes("=== 'Save'"), false);
  assert.equal(source.includes("=== 'Save & Next'"), false);
});

test('grouped-reading tried letters stay isolated per run-local map', async () => {
  const picks = [];
  const driver = {
    async executeScript(_code, triedObj, pill) {
      const key = `${pill}:0:A option`;
      const done = triedObj[key] || [];
      const letter = done.includes('A') ? 'B' : 'A';
      picks.push({ pill, done: [...done], letter });
      return [{ key, letter }];
    },
  };

  const firstRunTried = new Map();
  await answerOpenGroups(driver, firstRunTried, 1);
  await answerOpenGroups(driver, firstRunTried, 1);

  const secondRunTried = new Map();
  await answerOpenGroups(driver, secondRunTried, 1);

  assert.deepEqual(picks, [
    { pill: 1, done: [], letter: 'A' },
    { pill: 1, done: ['A'], letter: 'B' },
    { pill: 1, done: [], letter: 'A' },
  ]);
});
