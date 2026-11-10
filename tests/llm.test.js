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

test('answerQuestion routes to Gemini when client has getGenerativeModel', async () => {
  const calls = [];
  const client = {
    getGenerativeModel: (opts) => {
      calls.push(opts);
      return {
        generateContent: async () => ({ response: { text: () => '{"answer":"went"}' } }),
      };
    },
  };
  const result = await answerQuestion(client, 'gemini-1.5-flash', 'Fill the blank', { text: 'She ___ home.' });
  assert.deepEqual(result, { answer: 'went' });
  assert.equal(calls[0].model, 'gemini-1.5-flash');
});

test('answerQuestion with the codex client runs `codex exec` and parses its last message', async () => {
  const fs = require('node:fs');
  const os = require('node:os');
  const path = require('node:path');
  const { createClient } = require('../src/llm');
  const bin = fs.mkdtempSync(path.join(os.tmpdir(), 'fake-codex-'));
  const log = path.join(bin, 'calls.txt');
  fs.writeFileSync(
    path.join(bin, 'codex'),
    `#!/bin/sh\necho "$@" > ${log}\ncat >> ${log}\nwhile [ "$1" != "-o" ]; do shift; done\nprintf '{"answer":"had started"}' > "$2"\n`,
    { mode: 0o755 }
  );
  const savedPath = process.env.PATH;
  process.env.PATH = `${bin}:${savedPath}`;
  try {
    const client = createClient({ llmProvider: 'codex', codex: { model: 'gpt-5' } });
    const result = await answerQuestion(client, 'ignored', 'Fill the blank', { text: 'the film ___ (start)' });
    assert.deepEqual(result, { answer: 'had started' });
    const call = fs.readFileSync(log, 'utf8');
    assert.match(call, /^exec --skip-git-repo-check --ephemeral --sandbox read-only -C \S+ --color never -o \S+ -m gpt-5 -\n/);
    assert.match(call, /"instruction":"Fill the blank"/);
  } finally {
    process.env.PATH = savedPath;
  }
});
