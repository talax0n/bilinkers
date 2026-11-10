//! Port of `detectMode` from `src/cli.js`: which runner fits the page the user left open.
//! Site behavior this relies on: docs/platform-notes.md#page-detection

use serde_json::{json, Value};

use crate::browser::{FramedPage, PageResult, IFRAME_COUNT};
use crate::grouped_reading::{is_grouped_reading_layout, GroupedReadingLayout};
use crate::runners::unit::GROUPED_READING_LAYOUT;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageMode {
    Iframe,
    Reading,
    Checkpoint,
    Course,
    Exercise,
    Unit,
}

impl PageMode {
    pub fn name(self) -> &'static str {
        match self {
            PageMode::Iframe => "iframe",
            PageMode::Reading => "reading",
            PageMode::Checkpoint => "checkpoint",
            PageMode::Course => "course",
            PageMode::Exercise => "exercise",
            PageMode::Unit => "unit",
        }
    }
}

/// What the window sees on the LMS tab: a page a run can start on, or why it keeps waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageKind {
    Mode(PageMode),
    /// Final tests run only from the terminal (`beelingua final-test`).
    FinalTest,
    /// The Beelingua home with the course islands, before a course is opened.
    IslandMap,
    Other,
}

const DOCUMENT_HTML: &str = "return document.documentElement ? document.documentElement.outerHTML : null;";

// A checkpoint gate looks like an exercise gate. Only the word "Checkpoint" differs.
const CHECKPOINT_GATE: &str = r#"
    const hasCheckpointHeading = /Checkpoint/i.test(document.body.textContent);
    const buttons = Array.from(document.querySelectorAll('button'));
    const hasGateButton = buttons.some((b) => {
      const text = b.textContent.trim();
      return text.startsWith('Start Attempt') || text === 'Continue';
    });
    return hasCheckpointHeading && hasGateButton;
  "#;

const COURSE_MAP: &str = r#"
    return [...document.querySelectorAll('.hoverable-pointer')].some((el) =>
      /^(Unit|Checkpoint)\d+$/i.test(el.textContent.replace(/\s+/g, '').trim())
    );
  "#;

const UNIT_LIST: &str = r#"
    const rows = document.querySelectorAll('button.bl-w-full');
    let titled = 0;
    rows.forEach((b) => { if (b.querySelector('.bl-text-ellipsis')) titled += 1; });
    return titled >= 2;
  "#;

// The roadmap's own "FT 0/1" counter and node labels never spell out "Final Test", but its
// labels are checked anyway so a roadmap is never mistaken for a test.
const FINAL_TEST: &str = r#"
    const roadmap = [...document.querySelectorAll('.hoverable-pointer')].some((el) =>
      /^(Unit|Checkpoint)\d+$/i.test(el.textContent.replace(/\s+/g, '').trim())
    );
    if (roadmap) return false;
    return /final[\s_-]*test/i.test(location.href) || /Final\s*Test/i.test(document.body.innerText);
  "#;

const ISLAND_MAP: &str = r#"
    return [...document.querySelectorAll('.hoverable-pointer')].some((el) =>
      /^[BC]\d\.\d$/.test(el.textContent.replace(/\s+/g, '').trim())
    );
  "#;

const OPTION_MARKER: &str = "bl-w-full justify-content-start";

/// The LTI (Bits) player's markers, which only ever appear inside an iframe.
fn is_lti_player(html: &str) -> bool {
    ["quiz-input-sa", "quiz-input-radio", "quiz-matching", r##"href="#/n""##].iter().any(|m| html.contains(m))
}

/// Checked in order. App chrome (MuiButtonBase, .bl-w-full) is on every page, so only specific markers count.
pub async fn detect_mode<P: FramedPage>(page: &P) -> PageResult<Option<PageMode>> {
    if page.execute_script(IFRAME_COUNT, &[]).await?.as_u64().unwrap_or(0) > 0 {
        // A frame between loads cannot be read; the top-level checks still apply.
        if let Ok(Value::String(html)) = page.execute_in_frame(DOCUMENT_HTML, &[]).await {
            if is_lti_player(&html) {
                return Ok(Some(PageMode::Iframe));
            }
            // A reading BlExercise is also iframe-embedded but uses MUI option buttons, not quiz-input-*.
            if html.contains(OPTION_MARKER) {
                return Ok(Some(PageMode::Reading));
            }
        }
    }

    let top_html = page.execute_script(DOCUMENT_HTML, &[]).await?;
    let top_html = top_html.as_str().unwrap_or_default();

    if page.execute_script(CHECKPOINT_GATE, &[]).await? == json!(true) {
        return Ok(Some(PageMode::Checkpoint));
    }
    if page.execute_script(COURSE_MAP, &[]).await? == json!(true) {
        return Ok(Some(PageMode::Course));
    }
    let layout = page.execute_script(GROUPED_READING_LAYOUT, &[]).await?;
    let layout: GroupedReadingLayout = serde_json::from_value(layout).unwrap_or_default();
    if is_grouped_reading_layout(&layout) {
        return Ok(Some(PageMode::Reading));
    }
    if top_html.contains(OPTION_MARKER) {
        return Ok(Some(PageMode::Exercise));
    }
    if page.execute_script(UNIT_LIST, &[]).await? == json!(true) {
        return Ok(Some(PageMode::Unit));
    }
    Ok(None)
}

/// detect_mode for the window: a Final Test is refused before anything else, since its pages
/// look like a checkpoint or an exercise, and the island map gets its own hint. Read-only.
pub async fn classify_page<P: FramedPage>(page: &P) -> PageResult<PageKind> {
    if page.execute_script(FINAL_TEST, &[]).await? == json!(true) {
        return Ok(PageKind::FinalTest);
    }
    if let Some(mode) = detect_mode(page).await? {
        return Ok(PageKind::Mode(mode));
    }
    if page.execute_script(ISLAND_MAP, &[]).await? == json!(true) {
        return Ok(PageKind::IslandMap);
    }
    Ok(PageKind::Other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::fake::FakePage;
    use crate::browser::Scope;

    /// A page described by its frame HTML (None without an iframe) and which top-level probes match.
    async fn detect(frame: Option<&str>, top: &str, matches: &[&str]) -> Option<PageMode> {
        detect_mode(&page(frame, top, matches)).await.unwrap()
    }

    fn page<'a>(
        frame: Option<&'a str>,
        top: &'a str,
        matches: &'a [&'a str],
    ) -> FakePage<impl FnMut(Scope, &str, &[Value]) -> PageResult<Value> + 'a> {
        FakePage::new(move |scope: Scope, code: &str, _: &[Value]| {
            Ok(match (scope, code) {
                (Scope::Top, IFRAME_COUNT) => json!(u8::from(frame.is_some())),
                (Scope::Frame, DOCUMENT_HTML) => json!(frame),
                (Scope::Top, DOCUMENT_HTML) => json!(top),
                (Scope::Top, GROUPED_READING_LAYOUT) if matches.contains(&"grouped") => {
                    json!({ "pills": ["1", "2"], "optionLetters": ["A", "B", "A", "B"] })
                }
                (Scope::Top, GROUPED_READING_LAYOUT) => json!({ "pills": [], "optionLetters": [] }),
                (Scope::Top, code) => json!(matches.iter().any(|m| code.contains(m))),
                other => panic!("unexpected script {other:?}"),
            })
        })
    }

    async fn classify(top: &str, matches: &[&str]) -> PageKind {
        classify_page(&page(None, top, matches)).await.unwrap()
    }

    #[tokio::test]
    async fn the_window_refuses_a_final_test_and_names_the_island_map() {
        assert_eq!(
            classify("", &["Final\\s*Test", "Start Attempt"]).await,
            PageKind::FinalTest,
            "before the checkpoint gate"
        );
        assert_eq!(classify("", &["Start Attempt"]).await, PageKind::Mode(PageMode::Checkpoint));
        assert_eq!(classify("", &["[BC]"]).await, PageKind::IslandMap);
        assert_eq!(classify("<html></html>", &["bl-text-ellipsis"]).await, PageKind::Mode(PageMode::Unit));
        assert_eq!(classify("<html></html>", &[]).await, PageKind::Other);
    }

    #[tokio::test]
    async fn detect_mode_prefers_the_iframe_markers_then_the_top_level_ones_in_cli_order() {
        assert_eq!(detect(Some(r#"<input class="quiz-input-sa">"#), "", &[]).await, Some(PageMode::Iframe));
        assert_eq!(detect(Some(r##"<a href="#/n">"##), "", &[]).await, Some(PageMode::Iframe));
        assert_eq!(
            detect(Some(r#"<button class="bl-w-full justify-content-start">"#), "", &[]).await,
            Some(PageMode::Reading)
        );
        assert_eq!(detect(None, "", &["Start Attempt"]).await, Some(PageMode::Checkpoint));
        assert_eq!(detect(None, "", &["hoverable-pointer"]).await, Some(PageMode::Course));
        assert_eq!(
            detect(None, r#"<button class="bl-w-full justify-content-start">"#, &["grouped"]).await,
            Some(PageMode::Reading)
        );
        assert_eq!(
            detect(None, r#"<button class="bl-w-full justify-content-start">"#, &["bl-text-ellipsis"]).await,
            Some(PageMode::Exercise)
        );
        assert_eq!(detect(None, "<html></html>", &["bl-text-ellipsis"]).await, Some(PageMode::Unit));
        assert_eq!(detect(Some("<html>launcher</html>"), "<html></html>", &[]).await, None);
    }
}
