//! Port of `src/questionTypes/_ltiQuiz.js`. waitForQuizFeedback is not ported: no handler uses it.

use std::sync::LazyLock;

use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::Value;

use crate::answer_loop::Outcome;
use crate::browser::{Driver, PageError};
use crate::js;

static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());
static WHITESPACE_RUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!("{}+", js::WS_CLASS)).unwrap());
// Author instructions are always styled in this teal in the Bits editor, whatever the wording.
static INSTRUCTION_SPAN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"color:rgb\(26, 106, 132\)">([\s\S]*?)</span>"#).unwrap());

/// The `stripTags` variant that also drops zero-width spaces.
pub fn strip_tags(fragment: &str) -> String {
    let text = TAG.replace_all(fragment, " ");
    let text = text.replace("&nbsp;", " ").replace('\u{200B}', "");
    let text = WHITESPACE_RUN.replace_all(&text, " ");
    js::trim(&text).to_string()
}

pub fn extract_instruction_text(html: &str) -> String {
    let parts: Vec<String> =
        INSTRUCTION_SPAN.captures_iter(html).map(|c| strip_tags(&c[1])).filter(|text| !text.is_empty()).collect();
    js::trim(&parts.join(" ")).to_string()
}

/// Clicks "Check My Answer" when it is enabled.
pub const CLICK_SUBMIT: &str = r#"
    const btn = document.querySelector('#quiz-submit-btn');
    if (btn && !btn.disabled) btn.click();
  "#;

/// Polls `poll` every 200 ms until it decides or `timeout` passes, re-clicking Submit once
/// 2.5 s before the deadline because the first click occasionally does not register.
pub async fn poll_with_submit_retry<D: Driver>(
    driver: &D,
    timeout: Duration,
    poll: &str,
    decide: impl Fn(&Value) -> Option<Outcome>,
) -> Result<Option<Outcome>, PageError> {
    let deadline = Instant::now() + timeout;
    let mut retried = false;
    while Instant::now() < deadline {
        if let Some(outcome) = decide(&driver.execute_script(poll, &[]).await?) {
            return Ok(Some(outcome));
        }
        if !retried && Instant::now() + Duration::from_millis(2500) > deadline {
            retried = true;
            driver.execute_script(CLICK_SUBMIT, &[]).await?;
        }
        driver.sleep(200).await;
    }
    Ok(None)
}
