require('dotenv').config();

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
      maxAnswerRetries: 10,
    },
    paths: {
      progressFile: './progress.json',
      unhandledLogDir: './logs/unhandled',
    },
  };
}

module.exports = { buildConfig, config: buildConfig(process.env) };
