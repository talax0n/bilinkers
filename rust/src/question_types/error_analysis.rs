//! Detect and parse from `src/questionTypes/errorAnalysis.js`.

use std::sync::LazyLock;

use regex::{Captures, Regex};
use serde::Serialize;

use super::lti_quiz::{extract_instruction_text, strip_tags};
use super::option_buttons::letter_at;
use super::Dom;

static BOLD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<b>([\s\S]*?)</b>").unwrap());
static UNDERLINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<u>([\s\S]*?)</u>").unwrap());

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Candidate {
    /// `None` past the sixth candidate, where the JS indexes past its letter list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub letter: Option<char>,
    pub word: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorAnalysisData {
    pub sentence: String,
    pub candidates: Vec<Candidate>,
    pub instruction_text: String,
}

pub fn detect(dom: &Dom) -> bool {
    dom.inside_iframe && dom.outer_html.contains("quiz-input-radio")
}

fn letter_text(letter: Option<char>) -> String {
    letter.map_or_else(|| "undefined".to_string(), String::from)
}

pub fn parse(dom: &Dom) -> ErrorAnalysisData {
    let html = &dom.outer_html;

    // Candidates are <b><u> words on some pages and bare <u> on others.
    let bold_inners: Vec<&str> =
        BOLD.captures_iter(html).map(|c| c.get(1).unwrap().as_str()).filter(|inner| inner.contains("<u>")).collect();
    let source_html = if bold_inners.is_empty() { html.clone() } else { bold_inners.join(" ") };

    // Mark candidates inline so a repeated word keeps its position.
    let mut candidates = Vec::new();
    let marked_html = UNDERLINE.replace_all(&source_html, |c: &Captures| {
        let inner = &c[1];
        let word = strip_tags(inner);
        if word.is_empty() {
            return inner.to_string();
        }
        let letter = letter_at(candidates.len());
        let marked = format!(" [{0}]{word}[/{0}] ", letter_text(letter));
        candidates.push(Candidate { letter, word });
        marked
    });

    // Some pages mark candidates with overlay labels or skip a <u>, so pad to the radio count.
    let radio_count = html.matches("quiz-input-radio").count();
    for i in candidates.len()..radio_count {
        candidates.push(Candidate { letter: letter_at(i), word: String::new() });
    }

    ErrorAnalysisData {
        sentence: strip_tags(&marked_html),
        candidates,
        instruction_text: extract_instruction_text(html),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dom(html: &str) -> Dom {
        Dom { outer_html: html.to_string(), inside_iframe: true, has_visible_slider: None }
    }

    fn letters(data: &ErrorAnalysisData) -> Vec<Option<char>> {
        data.candidates.iter().map(|c| c.letter).collect()
    }

    const UNDERLINE_SAMPLE: &str = r#"
<div>
  <b><u>had went</u></b> to the store, and <b><u>bought</u></b> some milk,
  then <b><u>drove</u></b> home before it <b><u>rained</u></b>.
</div>
<div class="quiz-input-radio"></div>
<div class="quiz-input-radio"></div>
<div class="quiz-input-radio"></div>
<div class="quiz-input-radio"></div>
"#;

    const OVERLAY_SAMPLE: &str = r#"
<div>By the time the guests arrived, the organizers have already set up all the chairs.</div>
<div id="container-f" data-options="label: A"></div>
<div id="container-g" data-options="label: B"></div>
<div id="container-h" data-options="label: C"></div>
<div id="container-i" data-options="label: D"></div>
<div id="container-j" data-options="label: E"></div>
<input class="quiz-input-radio">
<input class="quiz-input-radio">
<input class="quiz-input-radio">
<input class="quiz-input-radio">
<input class="quiz-input-radio">
"#;

    const PARTIAL_UNDERLINE_SAMPLE: &str = r#"
<div>
  She left <b><u>her umbrella at</u></b> the cafe, so she <b><u>should have gotten</u></b> wet
  <b><u>on</u></b> <b><u>her way</u></b> home.
</div>
<input class="quiz-input-radio">
<input class="quiz-input-radio">
<input class="quiz-input-radio">
<input class="quiz-input-radio">
<input class="quiz-input-radio">
"#;

    #[test]
    fn detect_matches_an_error_analysis_quiz_input_radio_page() {
        assert!(detect(&dom(UNDERLINE_SAMPLE)));
    }

    #[test]
    fn parse_extracts_one_candidate_per_underlined_word() {
        let parsed = parse(&dom(UNDERLINE_SAMPLE));
        assert_eq!(letters(&parsed), vec![Some('A'), Some('B'), Some('C'), Some('D')]);
        assert_eq!(parsed.candidates[0].word, "had went");
    }

    #[test]
    fn parse_falls_back_to_one_candidate_per_radio_when_no_u_tags_are_present() {
        let parsed = parse(&dom(OVERLAY_SAMPLE));
        assert_eq!(letters(&parsed), vec![Some('A'), Some('B'), Some('C'), Some('D'), Some('E')]);
    }

    #[test]
    fn parse_pads_candidates_up_to_the_real_radio_count_when_fewer_u_tags_are_found_than_options() {
        let parsed = parse(&dom(PARTIAL_UNDERLINE_SAMPLE));
        assert_eq!(letters(&parsed), vec![Some('A'), Some('B'), Some('C'), Some('D'), Some('E')]);
        assert_eq!(parsed.candidates[0].word, "her umbrella at");
        assert_eq!(parsed.candidates[3].word, "her way");
        assert_eq!(parsed.candidates[4].word, "");
    }
}
