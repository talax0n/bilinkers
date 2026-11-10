//! Port of `src/questionTypes/quizMatching.js`.

use serde::Serialize;

use std::time::Duration;

use super::lti_quiz::{extract_instruction_text, poll_with_submit_retry, CLICK_SUBMIT};
use super::Dom;
use crate::answer_loop::{Check, Outcome};
use crate::browser::{jittered, Driver};
use crate::js;

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

const FILL_DROPZONES: &str = r#"
    let count = 0;
    document.querySelectorAll('.quiz-matching').forEach((quiz) => {
      quiz.querySelectorAll(':scope > .dropzone').forEach((dropzone) => {
        const target = dropzone.getAttribute('data-drag-target');
        const dragEl = quiz.querySelector('.draggable[data-drag="' + target + '"]');
        if (!dragEl) return;
        const dz = dropzone.style;
        const de = dragEl.style;
        de.left = (parseFloat(dz.left) + (parseFloat(dz.width) - parseFloat(de.width)) / 2) + '%';
        de.top = (parseFloat(dz.top) + (parseFloat(dz.height) - parseFloat(de.height)) / 2) + '%';
        dropzone.setAttribute('data-filled', target);
        count += 1;
      });
    });
    const btn = document.querySelector('#quiz-submit-btn');
    if (btn) btn.removeAttribute('disabled');
    return count;
  "#;

/// Synthetic drags don't register with interact.js. Grading only compares data-filled
/// to data-drag-target, so the end state is written directly.
pub async fn answer<D: Driver>(driver: &D) -> Result<(), String> {
    let filled = driver.execute_script(FILL_DROPZONES, &[]).await.map_err(|e| e.0)?;
    if !js::truthy(&filled) {
        return Err("quizMatching.answer: no dropzone/draggable pairs found to fill".to_string());
    }
    driver.sleep(jittered(100, 100)).await;
    driver.execute_script(CLICK_SUBMIT, &[]).await.map_err(|e| e.0)?;
    Ok(())
}

/// The banners often stay hidden, so a visible .quiz-next-btn is the signal.
pub async fn check_result<D: Driver>(driver: &D) -> Result<Check, String> {
    let poll = r#"
      const nextBtn = document.querySelector('.quiz-next-btn');
      if (nextBtn && getComputedStyle(nextBtn).display !== 'none') return 'correct';
      const incorrect = document.querySelector('.incorrect.timeout, .incorrect');
      if (incorrect && getComputedStyle(incorrect).display !== 'none') return 'incorrect';
      return null;
    "#;
    let outcome = poll_with_submit_retry(driver, Duration::from_secs(20), poll, |v| match v.as_str() {
        Some("correct") => Some(Outcome::Correct),
        Some("incorrect") => Some(Outcome::Incorrect),
        _ => None,
    })
    .await
    .map_err(|e| e.0)?;
    outcome.map(Check::new).ok_or_else(|| {
        "quizMatching checkResult: timed out waiting for Continue link or incorrect feedback".to_string()
    })
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

    use crate::browser::fake::FakePage;
    use crate::browser::{PageResult, Scope};
    use serde_json::{json, Value};

    #[tokio::test]
    async fn answer_fills_each_dropzone_with_the_draggable_whose_data_drag_matches_its_data_drag_target_then_submits() {
        let mut executed: Vec<String> = Vec::new();
        let page = FakePage::new(|_: Scope, script: &str, _: &[Value]| -> PageResult<Value> {
            executed.push(script.to_string());
            Ok(if executed.len() == 1 { json!(1) } else { Value::Null })
        });
        answer(&page).await.unwrap();
        drop(page);
        assert_eq!(executed.len(), 2);
        assert!(executed[0].contains("data-drag-target"));
        assert!(executed[1].contains("quiz-submit-btn"));
    }

    #[tokio::test]
    async fn answer_throws_when_no_dropzone_draggable_pairs_are_found_nothing_filled() {
        let page = FakePage::new(|_: Scope, _: &str, _: &[Value]| -> PageResult<Value> { Ok(json!(0)) });
        let err = answer(&page).await.unwrap_err();
        assert!(err.contains("no dropzone/draggable pairs"), "{err}");
    }
}
