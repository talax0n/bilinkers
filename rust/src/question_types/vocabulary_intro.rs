//! Port of `src/questionTypes/vocabularyIntro.js`.

use serde::Serialize;

use serde_json::json;

use super::quiz_matching::has_matching_markup;
use super::Dom;
use crate::answer_loop::{Check, Outcome};
use crate::browser::{jittered, Driver};

/// Intro slides carry no question data; serializes as `{}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct VocabularyIntroData {}

/// Quiz pages also carry a hidden `#/n` Continue, so they are excluded first.
pub fn detect(dom: &Dom) -> bool {
    if !dom.inside_iframe {
        return false;
    }
    let html = &dom.outer_html;
    if html.contains("quiz-input-radio") || html.contains("quiz-input-sa") {
        return false;
    }
    if has_matching_markup(html) {
        return false;
    }
    html.contains(r##"href="#/n""##)
}

pub fn parse(_dom: &Dom) -> VocabularyIntroData {
    VocabularyIntroData {}
}

/// The visible Continue sometimes has a stale href, so the `#/n` arrow is clicked instead.
pub async fn answer<D: Driver>(driver: &D) -> Result<(), String> {
    let clicked = driver
        .execute_script(
            r##"
    const link = document.querySelector('a[href="#/n"]');
    if (link) { link.click(); return true; }
    return false;
  "##,
            &[],
        )
        .await
        .map_err(|e| e.0)?;
    if clicked != json!(true) {
        return Err("vocabularyIntro.answer: no \"#/n\" next link found".to_string());
    }
    driver.sleep(jittered(100, 100)).await;
    Ok(())
}

/// Nothing is graded on these slides.
pub fn check_result() -> Check {
    Check::new(Outcome::Correct)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dom(html: &str, inside_iframe: bool) -> Dom {
        Dom { outer_html: html.to_string(), inside_iframe, has_visible_slider: None }
    }

    #[test]
    fn detect_matches_an_iframe_slide_with_a_n_next_link() {
        assert!(detect(&dom(r##"<a href="#/n">Next</a>"##, true)));
    }

    #[test]
    fn detect_ignores_content_outside_the_iframe() {
        assert!(!detect(&dom(r##"<a href="#/n">Next</a>"##, false)));
    }

    #[test]
    fn detect_defers_to_fill_in_blank_error_analysis_on_real_quiz_pages() {
        assert!(!detect(&dom(r##"<a href="#/n"></a><input class="quiz-input-sa">"##, true)));
        assert!(!detect(&dom(r##"<a href="#/n"></a><input class="quiz-input-radio">"##, true)));
    }

    #[test]
    fn detect_matches_an_intro_slide_whose_embedded_quiz_manifest_json_happens_to_contain_quiz_matching() {
        let html = r##"<a href="#/n"></a><script>{"quizes":[{"quizID":"quiz-matching-14","page":70731}]}</script>"##;
        assert!(detect(&dom(html, true)));
    }

    #[test]
    fn check_result_always_reports_correct_nothing_is_graded_on_these_pages() {
        assert_eq!(check_result(), Check::new(Outcome::Correct));
    }
}
