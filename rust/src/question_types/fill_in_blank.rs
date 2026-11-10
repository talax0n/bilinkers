//! Detect and parse from `src/questionTypes/fillInBlank.js`.

use std::sync::LazyLock;

use regex::{Captures, Regex};
use serde::Serialize;

use super::lti_quiz::extract_instruction_text;
use super::option_buttons::strip_tags;
use super::Dom;

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
}
