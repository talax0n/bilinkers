const { execFile } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const OpenAI = require('openai');

function createClient(config) {
  if (config.llmProvider === 'codex') return { codex: { model: config.codex.model } };
  if ((config.llmProvider || 'openai') === 'gemini') {
    const { GoogleGenerativeAI } = require('@google/generative-ai');
    return new GoogleGenerativeAI(config.gemini.apiKey);
  }
  return new OpenAI({ baseURL: config.openai.baseURL, apiKey: config.openai.apiKey });
}

const SYSTEM_PROMPT =
  'You are completing an English-course exercise. Respond with strict JSON only, ' +
  'matching the shape implied by the question data (e.g. { "answer": "..." } for a ' +
  'single answer, or { "answers": [...] } for multiple blanks/pills). No prose outside the JSON.';

// No response_format. One provider returns 502 when it is set.
// Models may still wrap the JSON in a ```json fence.
function parseJsonContent(content, label) {
  if (!content) {
    throw new Error(`answerQuestion: ${label} response had no message content`);
  }
  const unfenced = content.replace(/^```(?:json)?\s*/i, '').replace(/```\s*$/, '').trim();
  try {
    return JSON.parse(unfenced);
  } catch (err) {
    throw new Error(`answerQuestion: ${label} response was not valid JSON: ${err.message}`);
  }
}

// Goes through the Codex CLI so the call rides the user's own `codex login`
// (ChatGPT OAuth). The bot never sees or stores a token.
function askCodex(model, prompt) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'beelingua-codex-'));
  const out = path.join(dir, 'answer.txt');
  const args = ['exec', '--skip-git-repo-check', '--ephemeral', '--sandbox', 'read-only', '-C', dir, '--color', 'never', '-o', out];
  if (model) args.push('-m', model);
  args.push('-');
  return new Promise((resolve, reject) => {
    const child = execFile('codex', args, { timeout: 120000, maxBuffer: 10 * 1024 * 1024 }, (err, _stdout, stderr) => {
      try {
        if (err) {
          const detail = String(stderr || err.message).trim();
          // ponytail: text match on the CLI's message; switch to an exit code if codex ever documents one.
          const signIn = /log ?in|logged out|unauthori[sz]ed|401/i.test(detail);
          const message = signIn ? 'Codex is not signed in. Run `codex login`, then rerun the bot.' : `codex exec failed: ${detail.split('\n').pop()}`;
          reject(Object.assign(new Error(message), { signIn }));
        }
        else resolve(fs.readFileSync(out, 'utf8'));
      } finally {
        fs.rmSync(dir, { recursive: true, force: true });
      }
    });
    child.stdin.end(prompt);
  });
}

async function answerQuestion(client, model, instruction, questionData, feedback = null) {
  const userPrompt = JSON.stringify({ instruction, questionData, feedback });

  if (client.codex) {
    return parseJsonContent(await askCodex(client.codex.model, `${SYSTEM_PROMPT}\n\n${userPrompt}`), 'LLM');
  }

  // GoogleGenerativeAI client, detected by shape (Gemini has no chat.completions API).
  if (typeof client.getGenerativeModel === 'function') {
    const genModel = client.getGenerativeModel({ model, systemInstruction: SYSTEM_PROMPT });
    const result = await genModel.generateContent(userPrompt);
    return parseJsonContent(result.response.text(), 'LLM');
  }

  const response = await client.chat.completions.create({
    model,
    messages: [
      { role: 'system', content: SYSTEM_PROMPT },
      { role: 'user', content: userPrompt },
    ],
  });

  const content = response.choices?.[0]?.message?.content;
  return parseJsonContent(content, 'LLM');
}

module.exports = { createClient, answerQuestion };
