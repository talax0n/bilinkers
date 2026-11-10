//! Port of `src/questionTypes/_optionButtons.js`.

use std::sync::LazyLock;

use std::time::{Duration, Instant};

use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};

use crate::answer_loop::{Check, Outcome};
use crate::browser::Driver;
use crate::js;

pub const OPTION_BUTTON_SELECTOR: &str = "button.bl-w-full.justify-content-start";

const LETTERS: [char; 6] = ['A', 'B', 'C', 'D', 'E', 'F'];

static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());
static WHITESPACE_RUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!("{}+", js::WS_CLASS)).unwrap());
static BUTTON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<button class="MuiButtonBase-root bl-w-full justify-content-start[^"]*"[\s\S]*?</button>"#).unwrap()
});
static RTF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"bl-rtf undefined"[^>]*>([\s\S]*?)</p></div>"#).unwrap());

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OptionButton {
    /// `None` past the sixth button, where the JS indexes past its letter list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub letter: Option<char>,
    pub text: String,
}

impl OptionButton {
    pub fn new(letter: char, text: &str) -> Self {
        OptionButton { letter: Some(letter), text: text.to_string() }
    }
}

pub fn letter_at(index: usize) -> Option<char> {
    LETTERS.get(index).copied()
}

pub fn strip_tags(html: &str) -> String {
    let text = TAG.replace_all(html, " ");
    let text = text.replace("&nbsp;", " ");
    let text = WHITESPACE_RUN.replace_all(&text, " ");
    js::trim(&text).to_string()
}

pub fn parse_options(html: &str) -> Vec<OptionButton> {
    BUTTON
        .find_iter(html)
        .enumerate()
        .map(|(i, button)| OptionButton {
            letter: letter_at(i),
            text: RTF.captures(button.as_str()).map_or_else(String::new, |c| strip_tags(&c[1])),
        })
        .collect()
}

/// The first `bl-rtf undefined` block up to `</p></div>`, with the JS `indexOf` and `slice`
/// fallbacks when either marker is missing.
pub fn first_rtf_block(html: &str) -> &str {
    let search_from = html.find("bl-rtf undefined").unwrap_or(0);
    let content_start = html[search_from..].find('>').map_or(0, |i| search_from + i + 1);
    let rtf_end = html[content_start..].find("</p></div>").map(|i| content_start + i);
    js::slice_to_index_of(html, content_start, rtf_end)
}

const CLICK_OPTION: &str = r#"
    const letter = arguments[0];
    const buttons = document.querySelectorAll('button.bl-w-full.justify-content-start');
    const target = Array.from(buttons).find((btn) => btn.textContent.trim().startsWith(letter));
    if (!target) return false;
    target.click();
    return true;
  "#;

const CLICK_CHECK: &str = r#"
    const buttons = Array.from(document.querySelectorAll('button'));
    const btn = buttons.find((b) => b.textContent.trim() === 'Check');
    if (btn) btn.click();
  "#;

const CHECK_FEEDBACK: &str = r#"
    const headings = Array.from(document.querySelectorAll('h5'));
    const heading = headings.find((h) => h.textContent.trim() === 'Correct!' || h.textContent.trim() === 'Incorrect!');
    if (!heading) return null;
    const hintEl = heading.parentElement && heading.parentElement.nextElementSibling;
    return {
      outcome: heading.textContent.trim(),
      hint: hintEl ? hintEl.textContent.trim() : null,
    };
  "#;

/// Clicks the option button whose text starts with `letter`, then "Check".
pub async fn select_option_and_check<D: Driver>(driver: &D, letter: &Value) -> Result<(), String> {
    let clicked = driver.execute_script(CLICK_OPTION, std::slice::from_ref(letter)).await.map_err(|e| e.0)?;
    if clicked != json!(true) {
        return Err(format!("selectOptionAndCheck: no option button found for letter \"{}\"", js::to_string(letter)));
    }
    driver.execute_script(CLICK_CHECK, &[]).await.map_err(|e| e.0)?;
    Ok(())
}

/// The hint next to "Incorrect!" is returned so the retry can use it.
pub async fn wait_for_check_feedback<D: Driver>(driver: &D) -> Result<Check, String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let state = driver.execute_script(CHECK_FEEDBACK, &[]).await.map_err(|e| e.0)?;
        match state["outcome"].as_str() {
            Some("Correct!") => return Ok(Check::new(Outcome::Correct)),
            Some("Incorrect!") => {
                let hint = state["hint"].as_str().filter(|h| !h.is_empty()).map(String::from);
                return Ok(Check { outcome: Outcome::Incorrect, hint });
            }
            _ => {}
        }
        driver.sleep(200).await;
    }
    Err("waitForCheckFeedback: timed out waiting for Correct!/Incorrect! feedback".to_string())
}
