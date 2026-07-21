const OpenAI = require('openai');

function createClient(config) {
  return new OpenAI({ baseURL: config.openai.baseURL, apiKey: config.openai.apiKey });
}

async function answerQuestion(client, model, instruction, questionData, feedback = null) {
  const systemPrompt =
    'You are completing an English-course exercise. Respond with strict JSON only, ' +
    'matching the shape implied by the question data (e.g. { "answer": "..." } for a ' +
    'single answer, or { "answers": [...] } for multiple blanks/pills). No prose outside the JSON.';
  const userPrompt = JSON.stringify({ instruction, questionData, feedback });

  const response = await client.chat.completions.create({
    model,
    messages: [
      { role: 'system', content: systemPrompt },
      { role: 'user', content: userPrompt },
    ],
    response_format: { type: 'json_object' },
  });

  const content = response.choices?.[0]?.message?.content;
  if (!content) {
    throw new Error('answerQuestion: LLM response had no message content');
  }
  try {
    return JSON.parse(content);
  } catch (err) {
    throw new Error(`answerQuestion: LLM response was not valid JSON: ${err.message}`);
  }
}

module.exports = { createClient, answerQuestion };
