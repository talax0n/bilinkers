const { answerQuestion } = require('./llm');

async function processQuestion({
  driver,
  dom,
  registry,
  llmClient,
  model,
  instruction,
  retryLimit,
  answerQuestionFn = answerQuestion,
}) {
  const handler = registry.findHandler(dom);
  if (!handler) {
    return { status: 'unhandled', attempts: 0 };
  }

  const questionData = handler.parse(dom);
  let attempts = 0;
  let feedback = null;
  let outcome = 'incorrect';

  while (attempts <= retryLimit) {
    const llmResult = await answerQuestionFn(llmClient, model, instruction, questionData, feedback);
    await handler.answer(driver, llmResult);
    outcome = await handler.checkResult(dom);
    attempts += 1;
    if (outcome === 'correct') break;
    feedback = 'previous answer was wrong, try again';
  }

  return { status: outcome, attempts };
}

module.exports = { processQuestion };
