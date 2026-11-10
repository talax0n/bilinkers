//! Pure parts of `src/questionTypes/_ltiQuiz.js`.

use std::sync::LazyLock;

use regex::Regex;

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
