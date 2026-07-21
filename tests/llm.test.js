const assert = require('node:assert/strict');
const { test } = require('node:test');
const { answerQuestion } = require('../src/llm');

function makeFakeClient(responseObj) {
  const calls = [];
  return {
    calls,
    chat: {
      completions: {
        create: async (params) => {
          calls.push(params);
          return { choices: [{ message: { content: JSON.stringify(responseObj) } }] };
        },
      },
    },
  };
}

test('answerQuestion returns parsed JSON from the LLM response', async () => {
  const client = makeFakeClient({ answer: 'went' });
  const result = await answerQuestion(client, 'gpt-4o', 'Fill the blank', { text: 'She ___ home.' });
  assert.deepEqual(result, { answer: 'went' });
});

test('answerQuestion includes feedback in the prompt when retrying', async () => {
  const client = makeFakeClient({ answer: 'goes' });
  await answerQuestion(client, 'gpt-4o', 'Fill the blank', { text: 'She ___ home.' }, 'previous answer was wrong');
  const userMessage = client.calls[0].messages.find((m) => m.role === 'user');
  assert.match(userMessage.content, /previous answer was wrong/);
});

test('answerQuestion sends model and no response_format (unsupported by some providers)', async () => {
  const client = makeFakeClient({ answer: 'x' });
  await answerQuestion(client, 'my-model', 'instr', { text: 'q' });
  assert.equal(client.calls[0].model, 'my-model');
  assert.equal(client.calls[0].response_format, undefined);
});

test('answerQuestion strips a ```json fence before parsing', async () => {
  const client = {
    chat: {
      completions: {
        create: async () => ({ choices: [{ message: { content: '```json\n{"answer":"went"}\n```' } }] }),
      },
    },
  };
  const result = await answerQuestion(client, 'gpt-4o', 'instr', { text: 'q' });
  assert.deepEqual(result, { answer: 'went' });
});

test('answerQuestion throws a clear error when LLM response is not valid JSON', async () => {
  const client = {
    chat: { completions: { create: async () => ({ choices: [{ message: { content: 'not json' } }] }) } },
  };
  await assert.rejects(
    () => answerQuestion(client, 'gpt-4o', 'instr', { text: 'q' }),
    /LLM response was not valid JSON/
  );
});

test('answerQuestion throws a clear error when LLM response has no content', async () => {
  const client = {
    chat: { completions: { create: async () => ({ choices: [] }) } },
  };
  await assert.rejects(
    () => answerQuestion(client, 'gpt-4o', 'instr', { text: 'q' }),
    /LLM response had no message content/
  );
});
