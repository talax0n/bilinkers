//! Port of `src/answerLoop.js`.
//! A sign-in error aborts at once: no call retries and no further answer attempts.

use std::num::NonZeroU32;

use serde_json::{json, Value};

use crate::js;
use crate::logger;
use crate::llm::{LlmError, NotSignedIn};
use crate::question_types::Dom;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Correct,
    Incorrect,
}

impl Outcome {
    pub fn name(self) -> &'static str {
        match self {
            Outcome::Correct => "correct",
            Outcome::Incorrect => "incorrect",
        }
    }
}

/// What a handler's `checkResult` resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub outcome: Outcome,
    pub hint: Option<String>,
}

impl Check {
    pub fn new(outcome: Outcome) -> Self {
        Check { outcome, hint: None }
    }

    /// Validates a raw outcome string read off the page, as processQuestion does.
    pub fn from_raw(raw: &str) -> Result<Check, String> {
        match raw {
            "correct" => Ok(Check::new(Outcome::Correct)),
            "incorrect" => Ok(Check::new(Outcome::Incorrect)),
            other => Err(format!(
                "handler.checkResult returned unexpected value: {}",
                serde_json::to_string(other).expect("a string serializes")
            )),
        }
    }
}

/// A question type bound to the live page.
#[allow(async_fn_in_trait)]
pub trait Handler {
    type Data;
    /// The JS module's `name`, for log lines.
    fn name(&self) -> &'static str {
        "handler"
    }
    fn parse(&self, dom: &Dom) -> Self::Data;
    async fn answer(&mut self, answer: &Value) -> Result<(), String>;
    async fn check_result(&mut self, dom: &Dom) -> Result<Check, String>;
}

/// Produces an answer object such as `{"answer": "A"}` or `{"answers": [...]}`.
#[allow(async_fn_in_trait)]
pub trait Answerer<D> {
    async fn answer(&mut self, instruction: &str, question_data: &D, feedback: Option<&str>)
        -> Result<Value, LlmError>;
}

/// Plain closures answer at once, which keeps tests and fixed answers short.
impl<D, F> Answerer<D> for F
where
    F: FnMut(&str, &D, Option<&str>) -> Result<Value, LlmError>,
{
    async fn answer(&mut self, instruction: &str, question_data: &D, feedback: Option<&str>) -> Result<Value, LlmError> {
        self(instruction, question_data, feedback)
    }
}

#[allow(async_fn_in_trait)]
pub trait Sleeper {
    /// Sleeps `base_ms` plus a random share of `jitter_ms` (`base + Math.random() * jitter`).
    async fn sleep(&mut self, base_ms: u64, jitter_ms: u64);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestionResult {
    Unhandled,
    Finished { outcome: Outcome, attempts: u32 },
}

pub const DEFAULT_CALL_ATTEMPTS: NonZeroU32 = NonZeroU32::new(3).unwrap();

/// Retries transient LLM or proxy failures so they don't use up an answer attempt.
pub async fn call_with_retry<T, S: Sleeper>(
    mut f: impl AsyncFnMut() -> Result<T, LlmError>,
    sleeper: &mut S,
    max_attempts: NonZeroU32,
) -> Result<T, LlmError> {
    let max_attempts = max_attempts.get();
    let mut i = 0;
    loop {
        match f().await {
            Ok(value) => return Ok(value),
            Err(err @ LlmError::NotSignedIn) => return Err(err),
            Err(err) if i + 1 >= max_attempts => return Err(err),
            Err(_) => sleeper.sleep(1000, 1000).await,
        }
        i += 1;
    }
}

/// `llmResult.answer !== undefined ? llmResult.answer : llmResult.answers`, null for undefined.
fn attempted_answer(result: &Value) -> Value {
    match result.get("answer") {
        Some(answer) => answer.clone(),
        None => result.get("answers").cloned().unwrap_or(Value::Null),
    }
}

/// `llmResult.answer !== undefined ? [llmResult.answer] : llmResult.answers || []`.
fn tried_answers(result: &Value) -> Result<Vec<Value>, String> {
    let Value::Object(fields) = result else {
        return Ok(Vec::new());
    };
    if let Some(answer) = fields.get("answer") {
        return Ok(vec![answer.clone()]);
    }
    match fields.get("answers") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => Ok(Vec::new()),
        Some(Value::String(s)) if s.is_empty() => Ok(Vec::new()),
        Some(Value::Number(n)) if n.as_f64() == Some(0.0) => Ok(Vec::new()),
        Some(Value::Array(items)) => Ok(items.clone()),
        Some(_) => Err("tried.forEach is not a function".to_string()),
    }
}

/// One answer attempt. `Ok` carries the check; `Err` is the JS catch path.
async fn attempt<H: Handler, A: Answerer<H::Data>, S: Sleeper>(
    handler: &mut H,
    dom: &Dom,
    question_data: &H::Data,
    instruction: &str,
    feedback: Option<&str>,
    answerer: &mut A,
    sleeper: &mut S,
    number: u32,
) -> Result<(Value, Check), LlmError> {
    let result = call_with_retry(
        async || answerer.answer(instruction, question_data, feedback).await,
        sleeper,
        DEFAULT_CALL_ATTEMPTS,
    )
    .await?;
    if result.is_null() {
        return Err(LlmError::Failed("null is not an object (evaluating 'llmResult.answer')".to_string()));
    }
    let attempted = attempted_answer(&result);
    logger::info(
        "Attempt: submitting answer",
        [("handler", json!(handler.name())), ("attempt", json!(number)), ("answer", attempted.clone())],
    );
    handler.answer(&result).await?;
    let check = handler.check_result(dom).await?;
    let mut fields =
        vec![("handler", json!(handler.name())), ("attempt", json!(number)), ("answer", attempted)];
    fields.push(("outcome", json!(check.outcome.name())));
    if let Some(hint) = check.hint.as_deref().filter(|h| !h.is_empty()) {
        fields.push(("hint", json!(hint)));
    }
    logger::info("Attempt: result", fields);
    Ok((result, check))
}

/// `retry_limit` is signed because the exercise runner passes `options.length - 1`,
/// which is -1 for a page with no options and then makes no attempt at all.
/// `Err` is the JS rethrow of a sign-in error.
pub async fn process_question<H: Handler, A: Answerer<H::Data>, S: Sleeper>(
    handler: Option<&mut H>,
    dom: &Dom,
    instruction: &str,
    retry_limit: i64,
    answerer: &mut A,
    sleeper: &mut S,
) -> Result<QuestionResult, NotSignedIn> {
    let Some(handler) = handler else {
        return Ok(QuestionResult::Unhandled);
    };

    let question_data = handler.parse(dom);
    let mut attempts: u32 = 0;
    let mut feedback: Option<String> = None;
    let mut outcome = Outcome::Incorrect;
    // Some pages repeat one hint for every wrong option, and each LLM call is stateless,
    // so ruled-out answers are listed explicitly.
    let mut tried: Vec<Value> = Vec::new();

    while i64::from(attempts) <= retry_limit {
        if attempts > 0 {
            sleeper.sleep(100, 100).await;
        }
        let result = attempt(handler, dom, &question_data, instruction, feedback.as_deref(), answerer, sleeper, attempts + 1)
            .await
            .and_then(|(answer, check)| {
                if check.outcome == Outcome::Incorrect {
                    for t in tried_answers(&answer)? {
                        if !tried.iter().any(|seen| js::same_value_zero(seen, &t)) {
                            tried.push(t);
                        }
                    }
                }
                Ok(check)
            });
        match result {
            Ok(check) => {
                outcome = check.outcome;
                if outcome == Outcome::Incorrect {
                    let tried_note = if tried.is_empty() {
                        String::new()
                    } else {
                        format!(" Already tried and confirmed wrong: {} — do not repeat these.", js::join(&tried, ", "))
                    };
                    feedback = Some(match check.hint.as_deref() {
                        Some(hint) if !hint.is_empty() => {
                            format!("previous answer was wrong. Hint: {hint}{tried_note}")
                        }
                        _ => format!("previous answer was wrong, try again.{tried_note}"),
                    });
                }
            }
            Err(LlmError::NotSignedIn) => return Err(NotSignedIn),
            Err(LlmError::Failed(message)) => {
                logger::warn(
                    "Attempt: failed with an error",
                    [("handler", json!(handler.name())), ("attempt", json!(attempts + 1)), ("error", json!(message))],
                );
                outcome = Outcome::Incorrect;
                feedback = Some(format!("previous attempt failed with an error: {message}"));
                attempts += 1;
                continue;
            }
        }
        attempts += 1;
        if outcome == Outcome::Correct {
            break;
        }
    }

    Ok(QuestionResult::Finished { outcome, attempts })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct NoSleep;

    impl Sleeper for NoSleep {
        async fn sleep(&mut self, _base_ms: u64, _jitter_ms: u64) {}
    }

    /// A test handler built from closures, like the JS object-literal modules.
    struct FakeHandler<A, C> {
        answer: A,
        check: C,
    }

    impl<A, C> Handler for FakeHandler<A, C>
    where
        A: FnMut(&Value) -> Result<(), String>,
        C: FnMut() -> Result<Check, String>,
    {
        type Data = Value;

        fn parse(&self, _dom: &Dom) -> Value {
            json!({ "text": "q" })
        }

        async fn answer(&mut self, answer: &Value) -> Result<(), String> {
            (self.answer)(answer)
        }

        async fn check_result(&mut self, _dom: &Dom) -> Result<Check, String> {
            (self.check)()
        }
    }

    type NoHandler = FakeHandler<fn(&Value) -> Result<(), String>, fn() -> Result<Check, String>>;

    fn always_a(_: &str, _: &Value, _: Option<&str>) -> Result<Value, LlmError> {
        Ok(json!({ "answer": "A" }))
    }

    #[tokio::test]
    async fn process_question_returns_unhandled_when_no_type_module_matches() {
        let result =
            process_question::<NoHandler, _, _>(None, &Dom::default(), "instr", 1, &mut always_a, &mut NoSleep).await;
        assert_eq!(result, Ok(QuestionResult::Unhandled));
    }

    #[tokio::test]
    async fn process_question_succeeds_on_first_attempt_when_correct() {
        let mut answer_calls = Vec::new();
        let mut handler = FakeHandler {
            answer: |a: &Value| {
                answer_calls.push(a.clone());
                Ok(())
            },
            check: || Check::from_raw("correct"),
        };
        let result = process_question(Some(&mut handler), &Dom::default(), "instr", 1, &mut always_a, &mut NoSleep).await;
        assert_eq!(result, Ok(QuestionResult::Finished { outcome: Outcome::Correct, attempts: 1 }));
        assert_eq!(answer_calls.len(), 1);
    }

    #[tokio::test]
    async fn process_question_retries_once_with_feedback_then_gives_up() {
        let mut feedbacks: Vec<Option<String>> = Vec::new();
        let mut handler = FakeHandler { answer: |_: &Value| Ok(()), check: || Check::from_raw("incorrect") };
        let mut answerer = |_: &str, _: &Value, feedback: Option<&str>| {
            feedbacks.push(feedback.map(String::from));
            Ok(json!({ "answer": "A" }))
        };
        let result = process_question(Some(&mut handler), &Dom::default(), "instr", 1, &mut answerer, &mut NoSleep).await;
        assert_eq!(result, Ok(QuestionResult::Finished { outcome: Outcome::Incorrect, attempts: 2 }));
        assert_eq!(
            feedbacks,
            vec![
                None,
                Some(
                    "previous answer was wrong, try again. Already tried and confirmed wrong: A — do not repeat these."
                        .to_string()
                ),
            ]
        );
    }

    #[tokio::test]
    async fn process_question_surfaces_a_hint_and_lists_prior_wrong_guesses_so_retries_do_not_repeat_them() {
        let mut feedbacks: Vec<Option<String>> = Vec::new();
        let mut answers_tried: Vec<Value> = Vec::new();
        let mut handler = FakeHandler {
            answer: |a: &Value| {
                answers_tried.push(a["answer"].clone());
                Ok(())
            },
            check: || Ok(Check { outcome: Outcome::Incorrect, hint: Some("Reread paragraph 3.".to_string()) }),
        };
        let mut call = 0;
        let mut answerer = |_: &str, _: &Value, feedback: Option<&str>| {
            feedbacks.push(feedback.map(String::from));
            call += 1;
            Ok(json!({ "answer": if call == 1 { "A" } else { "B" } }))
        };
        let result = process_question(Some(&mut handler), &Dom::default(), "instr", 1, &mut answerer, &mut NoSleep).await;
        assert_eq!(result, Ok(QuestionResult::Finished { outcome: Outcome::Incorrect, attempts: 2 }));
        assert_eq!(answers_tried, vec![json!("A"), json!("B")]);
        assert_eq!(
            feedbacks,
            vec![
                None,
                Some("previous answer was wrong. Hint: Reread paragraph 3. Already tried and confirmed wrong: A — do not repeat these.".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn process_question_treats_a_throwing_handler_as_incorrect_and_still_respects_retry_limit() {
        let mut handler =
            FakeHandler { answer: |_: &Value| Err("stale element".to_string()), check: || Check::from_raw("correct") };
        let result = process_question(Some(&mut handler), &Dom::default(), "instr", 1, &mut always_a, &mut NoSleep).await;
        assert_eq!(result, Ok(QuestionResult::Finished { outcome: Outcome::Incorrect, attempts: 2 }));
    }

    #[tokio::test]
    async fn process_question_treats_an_unexpected_check_result_value_as_incorrect() {
        let mut handler = FakeHandler { answer: |_: &Value| Ok(()), check: || Check::from_raw("not-a-real-status") };
        let result = process_question(Some(&mut handler), &Dom::default(), "instr", 0, &mut always_a, &mut NoSleep).await;
        assert_eq!(result, Ok(QuestionResult::Finished { outcome: Outcome::Incorrect, attempts: 1 }));
    }

    #[tokio::test]
    async fn process_question_stops_on_the_first_sign_in_error_instead_of_retrying() {
        let mut handler = FakeHandler { answer: |_: &Value| Ok(()), check: || Check::from_raw("correct") };
        let mut calls = 0;
        let mut answerer = |_: &str, _: &Value, _: Option<&str>| -> Result<Value, LlmError> {
            calls += 1;
            Err(LlmError::NotSignedIn)
        };
        let result = process_question(Some(&mut handler), &Dom::default(), "i", 25, &mut answerer, &mut NoSleep).await;
        assert_eq!(result, Err(NotSignedIn));
        assert!(result.unwrap_err().to_string().contains("codex login"));
        assert_eq!(calls, 1);
    }

    #[tokio::test]
    async fn call_with_retry_retries_other_failures_up_to_max_attempts() {
        let mut calls = 0;
        let result: Result<(), LlmError> = call_with_retry(
            async || {
                calls += 1;
                Err(LlmError::Failed("403 voucher".to_string()))
            },
            &mut NoSleep,
            DEFAULT_CALL_ATTEMPTS,
        )
        .await;
        assert_eq!(result, Err(LlmError::Failed("403 voucher".to_string())));
        assert_eq!(calls, 3);
    }
}
