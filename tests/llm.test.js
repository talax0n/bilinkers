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

test('answerQuestion sends model and json response_format', async () => {
  const client = makeFakeClient({ answer: 'x' });
  await answerQuestion(client, 'my-model', 'instr', { text: 'q' });
  assert.equal(client.calls[0].model, 'my-model');
  assert.deepEqual(client.calls[0].response_format, { type: 'json_object' });
});
