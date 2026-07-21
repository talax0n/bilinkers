require('dotenv').config();

function buildConfig(env) {
  return {
    openai: {
      baseURL: env.OPENAI_BASE_URL,
      apiKey: env.OPENAI_API_KEY,
      model: env.OPENAI_MODEL || 'gpt-4o',
    },
    timeouts: {
      loginPollMs: 2000,
      loginTimeoutMs: 5 * 60 * 1000,
      elementWaitMs: 10000,
    },
    retry: {
      maxAnswerRetries: 1,
    },
    paths: {
      progressFile: './progress.json',
      unhandledLogDir: './logs/unhandled',
    },
  };
}

module.exports = { buildConfig, config: buildConfig(process.env) };
