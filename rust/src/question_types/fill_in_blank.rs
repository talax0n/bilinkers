//! Port of `src/questionTypes/fillInBlank.js`.

use std::sync::LazyLock;

use regex::{Captures, Regex};
use serde::Serialize;

use std::time::Duration;

use serde_json::{json, Value};

use super::lti_quiz::{extract_instruction_text, poll_with_submit_retry, CLICK_SUBMIT};
use super::option_buttons::strip_tags;
use super::Dom;
use crate::answer_loop::{Check, Outcome};
use crate::browser::{jittered, Driver};
use crate::js;

// The JS uses a backreference `<\/\1>`; the regex crate has none, so each tag is its own branch.
// No two tag names share a prefix, so at most one branch can open at any position.
static BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)<(?:p(?-u:\b)[^>]*>([\s\S]*?)</p>|div(?-u:\b)[^>]*>([\s\S]*?)</div>|li(?-u:\b)[^>]*>([\s\S]*?)</li>|td(?-u:\b)[^>]*>([\s\S]*?)</td>)",
    )
    .unwrap()
});
static BOLD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)<(?:strong(?-u:\b)[^>]*>([\s\S]*?)</strong>|b(?-u:\b)[^>]*>([\s\S]*?)</b>)").unwrap()
});
static BOLD_OPEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<(?:strong|b)(?-u:\b)").unwrap());
static BLANK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"_{2,}").unwrap());

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FillInBlankData {
    pub sentence: String,
    pub full_text: String,
    pub blank_count: usize,
    pub instruction_text: String,
}

/// The content group of whichever tag branch matched.
fn inner<'h>(c: &Captures<'h>) -> &'h str {
    c.iter().skip(1).flatten().next().map_or("", |m| m.as_str())
}

fn extract_sentence(html: &str) -> String {
    for c in BLOCK.captures_iter(html) {
        let content = inner(&c);
        let text = strip_tags(content);
        if !BLANK.is_match(&text) {
            continue;
        }
        if !BOLD_OPEN.is_match(content) {
            continue;
        }
        return text;
    }

    let chunks: Vec<String> =
        BOLD.captures_iter(html).map(|c| strip_tags(inner(&c))).filter(|chunk| !chunk.is_empty()).collect();

    let Some(blank_index) = chunks.iter().position(|chunk| BLANK.is_match(chunk)) else {
        return String::new();
    };

    let mut parts = vec![chunks[blank_index].as_str()];
    if let Some(next) = chunks.get(blank_index + 1) {
        if next.starts_with('(') || next.starts_with('[') {
            parts.push(next);
        }
    }
    parts.join(" ")
}

pub fn detect(dom: &Dom) -> bool {
    dom.inside_iframe && dom.outer_html.contains("quiz-input-sa")
}

pub fn parse(dom: &Dom) -> FillInBlankData {
    let html = &dom.outer_html;
    FillInBlankData {
        sentence: extract_sentence(html),
        full_text: strip_tags(html),
        blank_count: html.matches("quiz-input-sa").count(),
        instruction_text: extract_instruction_text(html),
    }
}

/// Sets each blank and submits. Extra inputs reuse the first answer.
pub async fn answer<D: Driver>(driver: &D, llm_result: &Value) -> Result<(), String> {
    // `llmResult.answers || [llmResult.answer]`.
    let answers = match llm_result.get("answers") {
        Some(answers) if js::truthy(answers) => answers.clone(),
        _ => json!([llm_result.get("answer").cloned().unwrap_or(Value::Null)]),
    };
    driver
        .execute_script(
            r#"
    const values = arguments[0];
    const inputs = document.querySelectorAll('.quiz-input-sa');
    inputs.forEach((input, i) => {
      const val = values[i] !== undefined ? values[i] : values[0];
      input.value = val;
      input.dispatchEvent(new Event('input', { bubbles: true }));
      input.dispatchEvent(new Event('change', { bubbles: true }));
    });
  "#,
            &[answers],
        )
        .await
        .map_err(|e| e.0)?;
    driver.sleep(jittered(100, 100)).await;
    driver.execute_script(CLICK_SUBMIT, &[]).await.map_err(|e| e.0)?;
    Ok(())
}

/// The .incorrect banner flashes even on correct answers, so only .quiz-next-btn counts.
pub async fn check_result<D: Driver>(driver: &D) -> Result<Check, String> {
    let poll = r#"
      const nextBtn = document.querySelector('.quiz-next-btn');
      return !!nextBtn && getComputedStyle(nextBtn).display !== 'none';
    "#;
    let outcome = poll_with_submit_retry(driver, Duration::from_secs(5), poll, |v| {
        (*v == json!(true)).then_some(Outcome::Correct)
    })
    .await
    .map_err(|e| e.0)?;
    Ok(Check::new(outcome.unwrap_or(Outcome::Incorrect)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dom(html: &str) -> Dom {
        Dom { outer_html: html.to_string(), inside_iframe: true, has_visible_slider: None }
    }

    const SAMPLE: &str = r#"
<div><strong>I wish it __________ (rainy).</strong></div>
<input class="quiz-input-sa">
"#;

    const LIVE_SHAPED: &str = r#"
<p class="question-text">
  <b data-x="1"><span>I don't know ___.</span></b>
  <b class="cue"><span>(they, join us)</span></b>
</p>
<input class="quiz-input-sa">
"#;

    #[test]
    fn detect_matches_a_quiz_input_sa_page() {
        assert!(detect(&dom(SAMPLE)));
    }

    #[test]
    fn detect_rejects_a_page_with_no_quiz_input_sa() {
        assert!(!detect(&dom("<div>no blanks here</div>")));
    }

    #[test]
    fn parse_keeps_existing_strong_fixture_sentence_extraction() {
        let parsed = parse(&dom(SAMPLE));
        assert_eq!(parsed.sentence, "I wish it __________ (rainy).");
        assert_eq!(parsed.blank_count, 1);
    }

    #[test]
    fn parse_extracts_live_shaped_b_markup_with_nested_spans_and_cue_text() {
        let parsed = parse(&dom(LIVE_SHAPED));
        assert_eq!(parsed.sentence, "I don't know ___. (they, join us)");
        assert_eq!(parsed.blank_count, 1);
    }

    use crate::browser::fake::FakePage;
    use crate::browser::{PageResult, Scope};

    // A correct answer still flashed the .incorrect banner on the way to .quiz-next-btn going visible.
    #[tokio::test]
    async fn check_result_resolves_correct_once_quiz_next_btn_becomes_visible_even_after_an_incorrect_banner_flash() {
        let mut calls = 0;
        let page = FakePage::new(|_: Scope, code: &str, _: &[Value]| -> PageResult<Value> {
            calls += 1;
            if code.contains("quiz-next-btn") {
                return Ok(json!(calls >= 3));
            }
            Ok(Value::Null)
        });
        assert_eq!(check_result(&page).await, Ok(Check::new(Outcome::Correct)));
    }

    #[tokio::test]
    async fn check_result_resolves_incorrect_after_the_deadline_if_quiz_next_btn_never_appears_retrying_submit_once() {
        let mut submit_clicks = 0;
        let page = FakePage::new(|_: Scope, code: &str, _: &[Value]| -> PageResult<Value> {
            if code.contains("quiz-next-btn") {
                return Ok(json!(false));
            }
            if code.contains("quiz-submit-btn") {
                submit_clicks += 1;
            }
            Ok(Value::Null)
        });
        assert_eq!(check_result(&page).await, Ok(Check::new(Outcome::Incorrect)));
        drop(page);
        assert_eq!(submit_clicks, 1);
    }
}
