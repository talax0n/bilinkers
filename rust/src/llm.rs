//! Pure parts of `src/llm.js`: the system prompt, the user prompt and response parsing.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use crate::js;

pub const SYSTEM_PROMPT: &str = concat!(
    "You are completing an English-course exercise. Respond with strict JSON only, ",
    "matching the shape implied by the question data (e.g. { \"answer\": \"...\" } for a ",
    "single answer, or { \"answers\": [...] } for multiple blanks/pills). No prose outside the JSON."
);

static OPENING_FENCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"(?i)^```(?:json)?{}*", js::WS_CLASS)).unwrap());
static CLOSING_FENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r"```{}*$", js::WS_CLASS)).unwrap());

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UserPrompt<'a, D> {
    instruction: &'a str,
    question_data: &'a D,
    feedback: Option<&'a str>,
}

/// `JSON.stringify({ instruction, questionData, feedback })`.
pub fn build_user_prompt<D: Serialize>(instruction: &str, question_data: &D, feedback: Option<&str>) -> String {
    serde_json::to_string(&UserPrompt { instruction, question_data, feedback })
        .expect("question data serializes to JSON")
}

/// No response_format is sent (one provider returns 502 on it), so models may wrap JSON in a fence.
pub fn parse_json_content(content: Option<&str>, label: &str) -> Result<Value, String> {
    let content = match content {
        Some(c) if !c.is_empty() => c,
        _ => return Err(format!("answerQuestion: {label} response had no message content")),
    };
    let unfenced = OPENING_FENCE.replace(content, "");
    let unfenced = CLOSING_FENCE.replace(&unfenced, "");
    serde_json::from_str(js::trim(&unfenced))
        .map_err(|err| format!("answerQuestion: {label} response was not valid JSON: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn answer_question_returns_parsed_json_from_the_llm_response() {
        let content = serde_json::to_string(&json!({ "answer": "went" })).unwrap();
        assert_eq!(parse_json_content(Some(&content), "LLM").unwrap(), json!({ "answer": "went" }));
    }

    #[test]
    fn answer_question_includes_feedback_in_the_prompt_when_retrying() {
        let prompt =
            build_user_prompt("Fill the blank", &json!({ "text": "She ___ home." }), Some("previous answer was wrong"));
        assert!(prompt.contains("previous answer was wrong"));
    }

    #[test]
    fn answer_question_strips_a_json_fence_before_parsing() {
        let result = parse_json_content(Some("```json\n{\"answer\":\"went\"}\n```"), "LLM").unwrap();
        assert_eq!(result, json!({ "answer": "went" }));
    }

    #[test]
    fn answer_question_throws_a_clear_error_when_llm_response_is_not_valid_json() {
        let err = parse_json_content(Some("not json"), "LLM").unwrap_err();
        assert!(err.contains("LLM response was not valid JSON"), "{err}");
    }

    #[test]
    fn answer_question_throws_a_clear_error_when_llm_response_has_no_content() {
        let err = parse_json_content(None, "LLM").unwrap_err();
        assert!(err.contains("LLM response had no message content"), "{err}");
    }
}
