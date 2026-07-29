require('dotenv').config();

function parseNonNegativeInteger(value, fallback) {
  if (value === undefined) return fallback;
  const text = String(value).trim();
  if (!/^\d+$/.test(text)) return fallback;
  const parsed = Number(text);
  return Number.isSafeInteger(parsed) ? parsed : fallback;
}

function buildConfig(env) {
  return {
    llmProvider: env.LLM_PROVIDER || 'openai',
    openai: {
      baseURL: env.OPENAI_BASE_URL,
      apiKey: env.OPENAI_API_KEY,
      model: env.OPENAI_MODEL || 'gpt-4o',
    },
    gemini: {
      apiKey: env.GEMINI_API_KEY,
      model: env.GEMINI_MODEL || 'gemini-1.5-flash',
    },
    timeouts: {
      loginPollMs: 2000,
      loginTimeoutMs: 5 * 60 * 1000,
      elementWaitMs: 10000,
    },
    retry: {
      maxAnswerRetries: parseNonNegativeInteger(env.MAX_ANSWER_RETRIES, 25),
    },
    paths: {
      progressFile: './progress.json',
      unhandledLogDir: './logs/unhandled',
    },
  };
}

module.exports = { buildConfig, config: buildConfig(process.env) };
