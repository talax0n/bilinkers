const assert = require('node:assert/strict');
const { test } = require('node:test');
const { isGroupedReadingLayout } = require('../scripts/grouped-reading');

test('recognizes top-level grouped reading only with multiple numeric pills and A-restarted option groups', () => {
  assert.equal(
    isGroupedReadingLayout({
      pills: ['1', '2', '3'],
      optionLetters: ['A', 'B', 'C', 'D', 'A', 'B', 'C', 'D'],
    }),
    true
  );

  assert.equal(
    isGroupedReadingLayout({
      pills: ['1', '2', '3'],
      optionLetters: ['A', 'B', 'C', 'D'],
    }),
    false,
    'an ordinary one-group native exercise is not reading'
  );

  assert.equal(
    isGroupedReadingLayout({
      pills: ['1'],
      optionLetters: ['A', 'B', 'A', 'B'],
    }),
    false,
    'a single numeric pill is not reading'
  );
});
