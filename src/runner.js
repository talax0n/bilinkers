const { answerQuestion } = require('./llm');

// LLM/proxy calls occasionally fail transiently (verified live: an
// OpenAI-compatible proxy intermittently 403'd with a voucher/billing
// error, then succeeded on the very next identical call seconds later) —
// retried here, separately from the question-level retry loop below, so a
// blip like that doesn't burn one of the exercise's limited answer attempts
// while never actually clicking a new answer.
async function callWithRetry(fn, args, driver, maxAttempts = 3) {
  let lastErr;
  for (let i = 0; i < maxAttempts; i += 1) {
    try {
      return await fn(...args);
    } catch (err) {
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
  // Some quiz pages show the same static hint text regardless of which
  // wrong option was tried (verified live: a True/False/Not-Given question
  // gave the identical hint after both "True" and "False" were tried and
  // marked incorrect) — each retry call is a stateless, independent LLM
  // request with no memory of earlier attempts, so without tracking this
  // itself the model can re-guess an option it already tried and got wrong.
  // Ruled-out answers are listed explicitly in the feedback so elimination
  // actually converges instead of only working by chance.
  const triedAnswers = [];

  while (attempts <= retryLimit) {
    if (attempts > 0) {
      await driver.sleep(1500 + Math.random() * 1500);
    }
    try {
      const llmResult = await callWithRetry(answerQuestionFn, [llmClient, model, instruction, questionData, feedback], driver);
      await handler.answer(driver, llmResult);
      const checkResult = await handler.checkResult(dom);
      // checkResult may be a bare outcome string, or { outcome, hint } when
      // the handler can surface the page's own explanation for why an
      // answer was wrong (see _optionButtons.js's waitForCheckFeedback).
      const { outcome: resolvedOutcome, hint } = typeof checkResult === 'string' ? { outcome: checkResult, hint: null } : checkResult;
      outcome = resolvedOutcome;
      if (outcome !== 'correct' && outcome !== 'incorrect') {
        throw new Error(`handler.checkResult returned unexpected value: ${JSON.stringify(checkResult)}`);
      }
      if (outcome === 'incorrect') {
        const tried = llmResult.answer !== undefined ? [llmResult.answer] : llmResult.answers || [];
        tried.forEach((t) => {
          if (!triedAnswers.includes(t)) triedAnswers.push(t);
        });
        const triedNote = triedAnswers.length ? ` Already tried and confirmed wrong: ${triedAnswers.join(', ')} — do not repeat these.` : '';
        feedback = hint ? `previous answer was wrong. Hint: ${hint}${triedNote}` : `previous answer was wrong, try again.${triedNote}`;
      }
    } catch (err) {
      outcome = 'incorrect';
      feedback = `previous attempt failed with an error: ${err.message}`;
      attempts += 1;
      continue;
    }
    attempts += 1;
    if (outcome === 'correct') break;
  }

  return { status: outcome, attempts };
}

module.exports = { processQuestion };
