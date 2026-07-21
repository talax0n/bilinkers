const assert = require('node:assert/strict');
const { test } = require('node:test');
const { createRegistry } = require('../src/questionTypes/registry');

test('findHandler returns the first module whose detect() matches', () => {
  const registry = createRegistry();
  const mcq = { name: 'multipleChoice', detect: (dom) => dom.kind === 'mcq' };
  const fillBlank = { name: 'fillBlank', detect: (dom) => dom.kind === 'blank' };
  registry.register(mcq);
  registry.register(fillBlank);

  assert.equal(registry.findHandler({ kind: 'blank' }), fillBlank);
  assert.equal(registry.findHandler({ kind: 'mcq' }), mcq);
});

test('findHandler returns null when nothing matches', () => {
  const registry = createRegistry();
  registry.register({ name: 'mcq', detect: () => false });
  assert.equal(registry.findHandler({ kind: 'unknown' }), null);
});

test('list returns registered modules in registration order', () => {
  const registry = createRegistry();
  const a = { name: 'a', detect: () => false };
  const b = { name: 'b', detect: () => false };
  registry.register(a);
  registry.register(b);
  assert.deepEqual(registry.list(), [a, b]);
});
