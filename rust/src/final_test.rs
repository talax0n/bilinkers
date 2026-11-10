//! Pure parts of `src/runners/final-test.js`. File reads and writes are left to the caller.

use std::fmt;
use std::num::NonZeroU32;

use serde_json::Value;

use crate::answer_loop::{Answerer, Sleeper};
use crate::js;
use crate::question_types::{OptionButton, ReadingComprehensionData};

const NEEDS_REVIEW: &str = "NEEDS REVIEW";

pub const ANSWER_INSTRUCTION: &str = concat!(
    "Determine the single correct option letter for this question, using the passage/question text and options provided. ",
    "Respond as JSON: { \"answer\": \"<letter>\", \"confidence\": <0.0 to 1.0> }."
);

pub const DEFAULT_ANSWER_ATTEMPTS: NonZeroU32 = NonZeroU32::new(3).unwrap();

/// An answer for one question, or the JS sentinel string `"NEEDS REVIEW"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinalAnswer {
    NeedsReview,
    Given(String),
}

impl FinalAnswer {
    pub fn from_text(text: &str) -> FinalAnswer {
        if text == NEEDS_REVIEW {
            FinalAnswer::NeedsReview
        } else {
            FinalAnswer::Given(text.to_string())
        }
    }
}

impl fmt::Display for FinalAnswer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FinalAnswer::NeedsReview => f.write_str(NEEDS_REVIEW),
            FinalAnswer::Given(text) => f.write_str(text),
        }
    }
}

/// One review pass's view of a question. Records loaded from disk may lack either number.
#[derive(Debug, Clone, PartialEq)]
pub struct Review {
    pub answer: FinalAnswer,
    pub confidence: Option<f64>,
    pub agreements: Option<u32>,
}

/// The result of `answerWithRetry`.
#[derive(Debug, Clone, PartialEq)]
pub struct Fresh {
    pub answer: FinalAnswer,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreviousAnswers {
    /// The stored records, as parsed; the JS keeps them untyped too.
    pub answers: Vec<Value>,
    pub stale: bool,
    pub stored_test_id: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum QuestionRecord {
    Listening {
        number: u32,
        answer: Option<String>,
        auto_answered: bool,
    },
    Unhandled {
        number: u32,
    },
    Mcq {
        number: u32,
        text: String,
        options: Vec<OptionButton>,
        answer: FinalAnswer,
        confidence: Option<f64>,
        agreements: Option<u32>,
        auto_answered: bool,
    },
}

/// Which branch of `formatQuestionRecord` a question takes, decided by its handler.
#[derive(Debug, Clone)]
pub enum RecordSource<'a> {
    /// No handler matched.
    Unhandled,
    /// audioMultipleChoice, with the guessed letter when one was clicked.
    Listening { answer: Option<&'a str> },
    /// Any other handler; the final test only registers readingComprehension.
    Mcq { data: &'a ReadingComprehensionData, review: Review },
}

/// The path without the `#/question-N` hash identifies the test.
pub fn test_id_from_url(url: &str) -> String {
    url.split('#').next().unwrap_or_default().to_string()
}

fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0 && !x.is_nan()),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// `contents` is the answers file's text, or `None` when it could not be read.
pub fn load_previous_answers(test_id: &str, contents: Option<&str>) -> PreviousAnswers {
    let unreadable = PreviousAnswers { answers: Vec::new(), stale: false, stored_test_id: None };
    let Some(parsed) = contents.and_then(|text| serde_json::from_str::<Value>(text).ok()) else {
        return unreadable;
    };
    // Reading a property of null throws in JS, which lands in the catch.
    if parsed.is_null() {
        return unreadable;
    }
    let field = |key: &str| match &parsed {
        Value::Object(fields) => fields.get(key).cloned(),
        _ => None,
    };
    let answers = match field("answers") {
        Some(Value::Array(items)) => items,
        _ => Vec::new(),
    };
    let stored = field("testId").filter(is_truthy);
    if let Some(stored_id) = &stored {
        if !test_id.is_empty() && stored_id.as_str() != Some(test_id) {
            return PreviousAnswers { answers: Vec::new(), stale: true, stored_test_id: stored };
        }
    }
    PreviousAnswers { answers, stale: false, stored_test_id: stored }
}

/// `x || 0` for a number that may be missing.
fn or_zero(x: Option<f64>) -> f64 {
    match x {
        Some(v) if v != 0.0 && !v.is_nan() => v,
        _ => 0.0,
    }
}

/// Agreeing passes raise confidence and count agreements. A disagreement halves the lower confidence.
pub fn combine_review(previous: Option<&Review>, current: &Fresh) -> Review {
    let previous = match previous {
        Some(p) if p.answer != FinalAnswer::NeedsReview => p,
        _ => {
            return Review { answer: current.answer.clone(), confidence: Some(current.confidence), agreements: Some(1) }
        }
    };
    if current.answer == FinalAnswer::NeedsReview {
        return previous.clone();
    }
    let previous_confidence = or_zero(previous.confidence);
    let current_confidence = or_zero(Some(current.confidence));
    if previous.answer == current.answer {
        let previous_agreements = match previous.agreements {
            Some(n) if n != 0 => n,
            _ => 1,
        };
        return Review {
            answer: current.answer.clone(),
            confidence: Some(0.99_f64.min(previous_confidence.max(current_confidence) + 0.2)),
            agreements: Some(previous_agreements + 1),
        };
    }
    Review {
        answer: current.answer.clone(),
        confidence: Some(current_confidence.min(previous_confidence) * 0.5),
        agreements: Some(1),
    }
}

pub fn format_question_record(number: u32, source: RecordSource<'_>, auto_answered: bool) -> QuestionRecord {
    match source {
        RecordSource::Listening { answer } => QuestionRecord::Listening {
            number,
            answer: answer.filter(|a| !a.is_empty()).map(String::from),
            auto_answered,
        },
        RecordSource::Unhandled => QuestionRecord::Unhandled { number },
        RecordSource::Mcq { data, review } => QuestionRecord::Mcq {
            number,
            text: data.text.clone(),
            options: data.options.clone(),
            answer: review.answer,
            confidence: review.confidence,
            agreements: review.agreements,
            auto_answered,
        },
    }
}

fn letter_text(letter: Option<char>) -> String {
    letter.map_or_else(|| "undefined".to_string(), String::from)
}

pub fn build_markdown(records: &[QuestionRecord]) -> String {
    records
        .iter()
        .map(|record| match record {
            QuestionRecord::Listening { number, answer: Some(answer), .. } => {
                format!("## Q{number} (listening — guessed: {answer})\n")
            }
            QuestionRecord::Listening { number, answer: None, .. } => {
                format!("## Q{number} (listening — skipped, jawab manual)\n")
            }
            QuestionRecord::Unhandled { number } => format!("## Q{number} (unhandled question type — jawab manual)\n"),
            QuestionRecord::Mcq { number, text, options, answer, confidence, auto_answered, .. } => {
                let option_lines: Vec<String> =
                    options.iter().map(|opt| format!("{}. {}", letter_text(opt.letter), opt.text)).collect();
                let option_lines = option_lines.join("\n");
                let conf = confidence.map_or_else(String::new, |c| format!(" (confidence {})", js::to_fixed_2(c)));
                let answered = if *auto_answered {
                    "**Jawaban: {AUTO-CLICKED}**".to_string()
                } else {
                    format!("**Jawaban: {answer}**")
                };
                format!("## Q{number}\n{text}\n{option_lines}\n{answered}{conf}\n")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The review prompt stays neutral; combine_review does the weighting.
pub fn review_instruction(previous: &Review) -> String {
    format!(
        "A previous review pass suggested answer \"{}\". Re-derive the answer yourself from the question \
and options; do not assume the previous answer is correct. If your own \
reasoning reaches the same letter, keep it; otherwise give the letter you \
now believe is correct. Report your confidence in your OWN judgment alone.",
        previous.answer
    )
}

/// One answer per question. Retries cover transient call failures only.
pub fn answer_with_retry<D, A: Answerer<D>, S: Sleeper>(
    answerer: &mut A,
    question_data: &D,
    max_attempts: NonZeroU32,
    sleeper: &mut S,
    previous: Option<&Review>,
) -> Fresh {
    let instruction = match previous {
        Some(p) => format!("{ANSWER_INSTRUCTION} {}", review_instruction(p)),
        None => ANSWER_INSTRUCTION.to_string(),
    };
    let max_attempts = max_attempts.get();
    for attempt in 1..=max_attempts {
        match answerer.answer(&instruction, question_data, None) {
            Ok(result) => {
                let field = |key: &str| match &result {
                    Value::Object(fields) => fields.get(key),
                    _ => None,
                };
                let confidence = js::to_number(field("confidence"));
                let answer = match field("answer") {
                    Some(Value::String(text)) => FinalAnswer::from_text(text),
                    Some(other) => FinalAnswer::Given(js::to_string(other)),
                    None => FinalAnswer::Given("undefined".to_string()),
                };
                return Fresh { answer, confidence: if confidence.is_finite() { confidence } else { 0.0 } };
            }
            Err(_) if attempt < max_attempts => sleeper.sleep(1000, 0),
            Err(_) => {}
        }
    }
    Fresh { answer: FinalAnswer::NeedsReview, confidence: 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct NoSleep;

    impl Sleeper for NoSleep {
        fn sleep(&mut self, _base_ms: u64, _jitter_ms: u64) {}
    }

    fn given(text: &str) -> FinalAnswer {
        FinalAnswer::Given(text.to_string())
    }

    fn question() -> Value {
        json!({ "text": "q", "options": [] })
    }

    #[test]
    fn format_question_record_builds_an_mcq_record_from_reading_comprehension_shaped_data() {
        let data = ReadingComprehensionData {
            text: "Which sentence uses the past perfect correctly?".to_string(),
            options: vec![OptionButton::new('A', "She go."), OptionButton::new('B', "She had gone.")],
        };
        let review = Review { answer: given("B"), confidence: Some(0.9), agreements: Some(2) };
        let record = format_question_record(3, RecordSource::Mcq { data: &data, review }, false);
        assert_eq!(
            record,
            QuestionRecord::Mcq {
                number: 3,
                text: "Which sentence uses the past perfect correctly?".to_string(),
                options: vec![OptionButton::new('A', "She go."), OptionButton::new('B', "She had gone.")],
                answer: given("B"),
                confidence: Some(0.9),
                agreements: Some(2),
                auto_answered: false,
            }
        );
    }

    #[test]
    fn format_question_record_builds_a_listening_record_for_audio_multiple_choice_no_answer_field_needed() {
        let record = format_question_record(7, RecordSource::Listening { answer: None }, false);
        assert_eq!(record, QuestionRecord::Listening { number: 7, answer: None, auto_answered: false });
    }

    #[test]
    fn format_question_record_builds_an_unhandled_record_when_no_handler_matched() {
        let record = format_question_record(12, RecordSource::Unhandled, false);
        assert_eq!(record, QuestionRecord::Unhandled { number: 12 });
    }

    #[test]
    fn build_markdown_renders_an_mcq_question_with_its_options_and_answer() {
        let md = build_markdown(&[QuestionRecord::Mcq {
            number: 3,
            text: "Which sentence uses the past perfect correctly?".to_string(),
            options: vec![OptionButton::new('A', "She go."), OptionButton::new('B', "She had gone.")],
            answer: given("B"),
            confidence: Some(0.95),
            agreements: None,
            auto_answered: false,
        }]);
        assert_eq!(
            md,
            "## Q3\nWhich sentence uses the past perfect correctly?\nA. She go.\nB. She had gone.\n**Jawaban: B** (confidence 0.95)\n"
        );
    }

    #[test]
    fn build_markdown_renders_a_listening_question_as_a_skip_line() {
        let md = build_markdown(&[QuestionRecord::Listening { number: 7, answer: None, auto_answered: false }]);
        assert_eq!(md, "## Q7 (listening — skipped, jawab manual)\n");
    }

    #[test]
    fn build_markdown_renders_an_unhandled_question_as_a_manual_review_line() {
        let md = build_markdown(&[QuestionRecord::Unhandled { number: 12 }]);
        assert_eq!(md, "## Q12 (unhandled question type — jawab manual)\n");
    }

    #[test]
    fn build_markdown_joins_multiple_questions_with_a_blank_line_between_them() {
        let md = build_markdown(&[
            QuestionRecord::Mcq {
                number: 1,
                text: "Q one?".to_string(),
                options: vec![OptionButton::new('A', "x")],
                answer: given("A"),
                confidence: Some(0.8),
                agreements: None,
                auto_answered: false,
            },
            QuestionRecord::Listening { number: 2, answer: None, auto_answered: false },
        ]);
        assert_eq!(
            md,
            "## Q1\nQ one?\nA. x\n**Jawaban: A** (confidence 0.80)\n\n## Q2 (listening — skipped, jawab manual)\n"
        );
    }

    #[test]
    fn answer_with_retry_returns_the_llm_answer_on_the_first_success() {
        let mut answerer = |_: &str, _: &Value, _: Option<&str>| Ok(json!({ "answer": "C", "confidence": 0.95 }));
        let result = answer_with_retry(&mut answerer, &question(), DEFAULT_ANSWER_ATTEMPTS, &mut NoSleep, None);
        assert_eq!(result, Fresh { answer: given("C"), confidence: 0.95 });
    }

    #[test]
    fn answer_with_retry_retries_up_to_max_attempts_then_returns_needs_review() {
        let mut calls = 0;
        let mut answerer = |_: &str, _: &Value, _: Option<&str>| -> Result<Value, String> {
            calls += 1;
            Err("boom".to_string())
        };
        let result = answer_with_retry(&mut answerer, &question(), NonZeroU32::new(3).unwrap(), &mut NoSleep, None);
        assert_eq!(result, Fresh { answer: FinalAnswer::NeedsReview, confidence: 0.0 });
        assert_eq!(calls, 3);
    }

    #[test]
    fn answer_with_retry_recovers_if_a_later_attempt_succeeds() {
        let mut calls = 0;
        let mut answerer = |_: &str, _: &Value, _: Option<&str>| {
            calls += 1;
            if calls < 2 {
                return Err("boom".to_string());
            }
            Ok(json!({ "answer": "D", "confidence": 0.8 }))
        };
        let result = answer_with_retry(&mut answerer, &question(), NonZeroU32::new(3).unwrap(), &mut NoSleep, None);
        assert_eq!(result, Fresh { answer: given("D"), confidence: 0.8 });
        assert_eq!(calls, 2);
    }

    #[test]
    fn answer_with_retry_defaults_missing_confidence_to_0() {
        let mut answerer = |_: &str, _: &Value, _: Option<&str>| Ok(json!({ "answer": "A" }));
        let result = answer_with_retry(&mut answerer, &question(), DEFAULT_ANSWER_ATTEMPTS, &mut NoSleep, None);
        assert_eq!(result, Fresh { answer: given("A"), confidence: 0.0 });
    }

    #[test]
    fn combine_review_returns_the_fresh_answer_when_there_is_no_previous_record() {
        let combined = combine_review(None, &Fresh { answer: given("C"), confidence: 0.6 });
        assert_eq!(combined, Review { answer: given("C"), confidence: Some(0.6), agreements: Some(1) });
    }

    #[test]
    fn combine_review_keeps_the_previous_answer_when_the_review_call_failed() {
        let previous = Review { answer: given("B"), confidence: Some(0.9), agreements: None };
        let combined = combine_review(Some(&previous), &Fresh { answer: FinalAnswer::NeedsReview, confidence: 0.0 });
        assert_eq!(combined, Review { answer: given("B"), confidence: Some(0.9), agreements: None });
    }

    #[test]
    fn test_id_from_url_strips_the_question_n_hash_so_the_same_test_always_matches() {
        assert_eq!(
            test_id_from_url("https://lms.binus.ac.id/exam/123#/question-4"),
            "https://lms.binus.ac.id/exam/123"
        );
        assert_eq!(test_id_from_url("https://lms.binus.ac.id/exam/123"), "https://lms.binus.ac.id/exam/123");
    }

    #[test]
    fn load_previous_answers_flags_a_different_test_as_stale_instead_of_reviewing() {
        let contents = serde_json::to_string(
            &json!({ "testId": "https://lms.binus.ac.id/exam/OLD", "answers": [{ "number": 1, "type": "mcq", "answer": "A" }] }),
        )
        .unwrap();
        let result = load_previous_answers("https://lms.binus.ac.id/exam/NEW", Some(&contents));
        assert!(result.stale);
        assert_eq!(result.answers, Vec::<Value>::new());
        assert_eq!(result.stored_test_id, Some(json!("https://lms.binus.ac.id/exam/OLD")));
    }

    #[test]
    fn load_previous_answers_returns_answers_for_the_same_test() {
        let contents = serde_json::to_string(
            &json!({ "testId": "https://lms.binus.ac.id/exam/SAME", "answers": [{ "number": 1, "type": "mcq", "answer": "A" }] }),
        )
        .unwrap();
        let result = load_previous_answers("https://lms.binus.ac.id/exam/SAME", Some(&contents));
        assert!(!result.stale);
        assert_eq!(result.answers.len(), 1);
    }
}
