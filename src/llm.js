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
  });

  const content = response.choices?.[0]?.message?.content;
  if (!content) {
    throw new Error('answerQuestion: LLM response had no message content');
  }
  // Some providers don't support response_format: json_object (one observed
  // provider returns a 502 for the whole request when it's set), so JSON is
  // enforced via the system prompt only — models occasionally wrap the
  // reply in a ```json fence anyway, so strip that before parsing.
  const unfenced = content.replace(/^```(?:json)?\s*/i, '').replace(/```\s*$/, '').trim();
  try {
    return JSON.parse(unfenced);
  } catch (err) {
    throw new Error(`answerQuestion: LLM response was not valid JSON: ${err.message}`);
  }
}

module.exports = { createClient, answerQuestion };
