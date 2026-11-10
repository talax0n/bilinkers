//! Pure parts of `src/questionTypes/_optionButtons.js`.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

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
