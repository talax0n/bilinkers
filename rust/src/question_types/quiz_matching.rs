//! Detect and parse from `src/questionTypes/quizMatching.js`.

use serde::Serialize;

use super::lti_quiz::extract_instruction_text;
use super::Dom;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuizMatchingData {
    pub instruction_text: String,
}

/// Also used by vocabularyIntro, whose intro slides embed a quiz manifest naming quiz-matching ids.
pub fn has_matching_markup(html: &str) -> bool {
    html.contains("quiz-matching") && html.contains("dropzone") && html.contains("draggable")
}

pub fn detect(dom: &Dom) -> bool {
    dom.inside_iframe && has_matching_markup(&dom.outer_html)
}

pub fn parse(dom: &Dom) -> QuizMatchingData {
    QuizMatchingData { instruction_text: extract_instruction_text(&dom.outer_html) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dom(html: &str, inside_iframe: bool) -> Dom {
        Dom { outer_html: html.to_string(), inside_iframe, has_visible_slider: None }
    }

    const SAMPLE: &str = r#"
<div id="quiz-matching-16" class="quiz quiz-matching">
  <div id="quiz-matching-16-target" class="dropzone quiz-matching-16" data-drag-target="1" style="left: 5%; top: 32%; width: 22%; height: 8%;"></div>
  <div id="quiz-matching-16-drag" class="draggable quiz-matching-16" data-drag="1" style="left: 26%; top: 74%; width: 20%; height: 5%;">leaky</div>
  <div id="quiz-matching-n-drag-2" class="draggable quiz-matching-n" data-drag="2" style="left: 36%; top: 60%; width: 20%; height: 5%;">unclog</div>
</div>
"#;

    #[test]
    fn detect_matches_a_quiz_matching_drag_drop_page() {
        assert!(detect(&dom(SAMPLE, true)));
    }

    #[test]
    fn detect_ignores_content_outside_the_iframe() {
        assert!(!detect(&dom(SAMPLE, false)));
    }

    #[test]
    fn detect_ignores_pages_with_no_drag_drop_markup() {
        assert!(!detect(&dom(r##"<a href="#/n">Next</a>"##, true)));
    }
}
