const { answerQuestion } = require('./llm');
const logger = require('./logger');

// A rejected sign-in fails the same way every time, so retrying only wastes minutes.
function isSignInError(err) {
  return err.status === 401 || err.signIn === true;
}

// Retries transient LLM/proxy failures so they don't use up an answer attempt.
async function callWithRetry(fn, args, driver, maxAttempts = 3) {
  let lastErr;
  for (let i = 0; i < maxAttempts; i += 1) {
    try {
      return await fn(...args);
    } catch (err) {
      if (isSignInError(err)) throw err;
      lastErr = err;
      if (i < maxAttempts - 1) await driver.sleep(1000 + Math.random() * 1000);
    }
  }
  throw lastErr;
}

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
  // Some pages repeat one hint for every wrong option, and each LLM call is stateless,
  // so ruled-out answers are listed explicitly.
  const triedAnswers = [];

  while (attempts <= retryLimit) {
    if (attempts > 0) {
      await driver.sleep(100 + Math.random() * 100);
    }
    try {
      const llmResult = await callWithRetry(answerQuestionFn, [llmClient, model, instruction, questionData, feedback], driver);
      const attemptedAnswer = llmResult.answer !== undefined ? llmResult.answer : llmResult.answers;
      logger.info('Attempt: submitting answer', { handler: handler.name, attempt: attempts + 1, answer: attemptedAnswer });
      await handler.answer(driver, llmResult);
      const checkResult = await handler.checkResult(dom);
      const { outcome: resolvedOutcome, hint } = typeof checkResult === 'string' ? { outcome: checkResult, hint: null } : checkResult;
      outcome = resolvedOutcome;
      if (outcome !== 'correct' && outcome !== 'incorrect') {
        throw new Error(`handler.checkResult returned unexpected value: ${JSON.stringify(checkResult)}`);
      }
      logger.info('Attempt: result', { handler: handler.name, attempt: attempts + 1, answer: attemptedAnswer, outcome, hint: hint || undefined });
      if (outcome === 'incorrect') {
        const tried = llmResult.answer !== undefined ? [llmResult.answer] : llmResult.answers || [];
        tried.forEach((t) => {
          if (!triedAnswers.includes(t)) triedAnswers.push(t);
        });
        const triedNote = triedAnswers.length ? ` Already tried and confirmed wrong: ${triedAnswers.join(', ')} — do not repeat these.` : '';
        feedback = hint ? `previous answer was wrong. Hint: ${hint}${triedNote}` : `previous answer was wrong, try again.${triedNote}`;
      }
    } catch (err) {
      if (isSignInError(err)) throw err;
      outcome = 'incorrect';
      feedback = `previous attempt failed with an error: ${err.message}`;
      logger.warn('Attempt: failed with an error', { handler: handler.name, attempt: attempts + 1, error: err.message });
      attempts += 1;
      continue;
    }
    attempts += 1;
    if (outcome === 'correct') break;
  }

  return { status: outcome, attempts };
}

module.exports = { processQuestion, callWithRetry };
