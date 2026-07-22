const OpenAI = require('openai');

function createClient(config) {
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

// Some providers don't support response_format: json_object (one observed
// provider returns a 502 for the whole request when it's set), so JSON is
// enforced via the system prompt only — models occasionally wrap the
// reply in a ```json fence anyway, so strip that before parsing.
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

async function answerQuestion(client, model, instruction, questionData, feedback = null) {
  const userPrompt = JSON.stringify({ instruction, questionData, feedback });

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
