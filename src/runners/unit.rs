//! Port of `src/runners/unit.js`.
//! Site behavior this relies on: docs/platform-notes.md#unit-activity-list

use std::collections::HashSet;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};

use super::{ActivityOutcome, ActivityRunners, Report};
use crate::browser::{Driver, FramedPage, Lms, PageError, PageResult, IFRAME_COUNT};
use crate::grouped_reading::{is_grouped_reading_layout, GroupedReadingLayout};
use crate::logger;

/// Completed rows show this checkmark SVG path.
pub const CHECKMARK_PATH: &str = "M14.2021 1.52344L5.45215 10.273L1.07715 5.89844";

/// The lettered option buttons of native and reading exercises.
const OPTION_MARKER: &str = "bl-w-full justify-content-start";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Row {
    pub title: String,
    pub disabled: bool,
    pub completed: bool,
}

/// What to do with the rows as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextRow<'a> {
    Open(&'a Row),
    AllDone,
    /// Open rows that were already tried this run and are still not complete.
    Uncompletable(Vec<String>),
}

/// Rows opened once this run are skipped, so an uncompletable row can't loop forever.
pub fn next_row<'a>(rows: &'a [Row], attempted: &HashSet<String>) -> NextRow<'a> {
    let open = |r: &&Row| !r.disabled && !r.completed;
    if let Some(row) = rows.iter().filter(open).find(|r| !attempted.contains(&r.title)) {
        return NextRow::Open(row);
    }
    let stuck: Vec<String> = rows.iter().filter(open).map(|r| r.title.clone()).collect();
    if stuck.is_empty() {
        NextRow::AllDone
    } else {
        NextRow::Uncompletable(stuck)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityKind {
    Exercise,
    Iframe,
    Reading,
}

impl ActivityKind {
    pub fn name(self) -> &'static str {
        match self {
            ActivityKind::Exercise => "exercise",
            ActivityKind::Iframe => "iframe",
            ActivityKind::Reading => "reading",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Video,
    Static,
}

impl MediaKind {
    pub fn name(self) -> &'static str {
        match self {
            MediaKind::Video => "video",
            MediaKind::Static => "static",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnitStuck {
    NoRows,
    Uncompletable { stuck: Vec<String> },
    RowNotClicked { title: String },
    RowDidNotOpen { title: String },
    Activity { title: String, outcome: ActivityOutcome },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnitOutcome {
    Complete { total: usize },
    Stuck(UnitStuck),
}

impl UnitOutcome {
    pub fn is_stuck(&self) -> bool {
        matches!(self, UnitOutcome::Stuck(_))
    }
}

impl Report for UnitOutcome {
    fn status(&self) -> &'static str {
        match self {
            UnitOutcome::Complete { .. } => "complete",
            UnitOutcome::Stuck(_) => "stuck",
        }
    }

    fn fields(&self) -> Vec<(&'static str, Value)> {
        let mut fields = vec![("status", json!(self.status()))];
        match self {
            UnitOutcome::Complete { total } => fields.push(("total", json!(total))),
            UnitOutcome::Stuck(UnitStuck::NoRows) => fields.push(("reason", json!("no-rows"))),
            UnitOutcome::Stuck(UnitStuck::Uncompletable { stuck }) => fields.push(("stuck", json!(stuck))),
            UnitOutcome::Stuck(UnitStuck::RowNotClicked { title } | UnitStuck::RowDidNotOpen { title }) => {
                fields.push(("title", json!(title)));
            }
            UnitOutcome::Stuck(UnitStuck::Activity { title, outcome }) => {
                fields.extend([("title", json!(title)), ("activityStatus", json!(outcome.status()))]);
            }
        }
        fields
    }
}

const READ_ROWS: &str = r#"
  const checkmarkPath = arguments[0];
  return Array.from(document.querySelectorAll('button.bl-w-full')).map((btn) => {
    const label = btn.querySelector('.bl-text-ellipsis');
    return {
      title: label ? label.textContent.trim() : '',
      disabled: btn.disabled || btn.classList.contains('Mui-disabled'),
      completed: btn.innerHTML.includes(checkmarkPath),
    };
  }).filter((r) => r.title);
"#;

const CLICK_START_GATE: &str = r#"
  const buttons = Array.from(document.querySelectorAll('button'));
  const btn = buttons.find((b) => ['Start', 'Continue'].includes(b.textContent.trim()));
  if (btn && !btn.disabled) { btn.click(); return true; }
  return false;
"#;

const DOCUMENT_HTML: &str = "return document.documentElement ? document.documentElement.outerHTML : null;";

/// src/groupedReading.js groupedReadingLayoutFromDocument.
pub(crate) const GROUPED_READING_LAYOUT: &str = r#"
  const pills = [...document.querySelectorAll('.bl-button__container')].map((pill) => pill.textContent.trim());
  const optionLetters = [...document.querySelectorAll('button.bl-w-full.justify-content-start')]
    .filter((button) => button.offsetParent !== null)
    .map((button) => button.textContent.trim()[0]);
  return { pills, optionLetters: optionLetters.map((l) => l === undefined ? '' : l) };
"#;

const PLAY_VIDEOS: &str = r#"
  const vids = [...document.querySelectorAll('video')];
  vids.forEach((v) => {
    try {
      v.muted = true;
      const p = v.play();
      if (p && p.catch) p.catch(() => {});
    } catch (e) {}
  });
  return vids.length;
"#;

// Seeking exactly to duration never sets ended. Seek to duration - 1 and let it play out.
const VIDEOS_ENDED: &str = r#"
  const vids = [...document.querySelectorAll('video')];
  vids.forEach((v) => {
    if (!v.ended && v.duration && isFinite(v.duration) && v.currentTime < v.duration - 1) {
      v.currentTime = Math.max(0, v.duration - 1);
    }
    if (!v.ended && v.paused) {
      const p = v.play();
      if (p && p.catch) p.catch(() => {});
    }
  });
  return vids.length > 0 && vids.every((v) => v.ended);
"#;

/// 15 minutes covers real-time playback when duration loads too late to seek.
const VIDEO_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// The unit activity list and the activity pages it opens.
#[allow(async_fn_in_trait)]
pub trait UnitPage: FramedPage {
    async fn read_rows(&self) -> PageResult<Vec<Row>> {
        let rows = self.execute_script(READ_ROWS, &[json!(CHECKMARK_PATH)]).await?;
        serde_json::from_value(rows).map_err(|e| PageError(format!("unit rows: {e}")))
    }

    /// A synthetic click() sometimes doesn't navigate on these rows, so this must be a real pointer click.
    async fn click_row(&self, title: &str) -> PageResult<bool>;

    /// Activities open behind a Start/Continue gate that mounts after the URL changes.
    /// Not every activity has one.
    async fn click_start_gate(&self, timeout: Duration) -> PageResult<bool> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.execute_script(CLICK_START_GATE, &[]).await? == json!(true) {
                return Ok(true);
            }
            self.sleep(300).await;
        }
        Ok(false)
    }

    /// Checks iframe and top-level markers each tick. Native exercises never get an iframe.
    /// `None` by the deadline means a video or notes row.
    async fn detect_mode(&self) -> PageResult<Option<ActivityKind>> {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if self.execute_script(IFRAME_COUNT, &[]).await?.as_u64().unwrap_or(0) > 0 {
                // A frame between loads cannot evaluate yet; it is still an iframe activity by the deadline.
                match self.execute_in_frame(DOCUMENT_HTML, &[]).await {
                    // Reading BlExercises also live in an iframe, with MUI option buttons instead of quiz-input-*.
                    Ok(Value::String(html)) if html.contains(OPTION_MARKER) => return Ok(Some(ActivityKind::Reading)),
                    Ok(_) => return Ok(Some(ActivityKind::Iframe)),
                    Err(_) if Instant::now() >= deadline => return Ok(Some(ActivityKind::Iframe)),
                    Err(_) => {
                        self.sleep(200).await;
                        continue;
                    }
                }
            }

            let layout = self.execute_script(GROUPED_READING_LAYOUT, &[]).await?;
            let layout: GroupedReadingLayout = serde_json::from_value(layout).unwrap_or_default();
            if is_grouped_reading_layout(&layout) {
                return Ok(Some(ActivityKind::Reading));
            }

            let top_html = self.execute_script("return document.documentElement.outerHTML;", &[]).await?;
            if top_html.as_str().is_some_and(|html| html.contains(OPTION_MARKER)) {
                return Ok(Some(ActivityKind::Exercise));
            }

            if Instant::now() >= deadline {
                return Ok(None);
            }
            self.sleep(200).await;
        }
    }

    /// Video and notes rows complete by being viewed. Videos must reach their end.
    async fn complete_media_activity(&self) -> PageResult<MediaKind> {
        // Top-level videos first, then each iframe; the ended poll stays in the document that played.
        let mut frame = None;
        let mut playing = self.execute_script(PLAY_VIDEOS, &[]).await?.as_u64().unwrap_or(0);
        if playing == 0 {
            let iframes = self.execute_script(IFRAME_COUNT, &[]).await?.as_u64().unwrap_or(0) as usize;
            for index in 0..iframes {
                // A frame that cannot evaluate has no video we could play.
                playing =
                    self.execute_in_frame_at(index, PLAY_VIDEOS, &[]).await.ok().and_then(|v| v.as_u64()).unwrap_or(0);
                if playing > 0 {
                    frame = Some(index);
                    break;
                }
            }
        }
        if playing > 0 {
            let deadline = Instant::now() + VIDEO_TIMEOUT;
            while Instant::now() < deadline {
                let ended = match frame {
                    None => self.execute_script(VIDEOS_ENDED, &[]).await?,
                    Some(index) => self.execute_in_frame_at(index, VIDEOS_ENDED, &[]).await?,
                };
                if ended == json!(true) {
                    return Ok(MediaKind::Video);
                }
                self.sleep(300).await;
            }
            return Err(PageError("Video did not finish within 15 minutes.".to_string()));
        }
        self.sleep(300).await;
        Ok(MediaKind::Static)
    }
}

const ROW_TITLE: &str =
    "function() { const label = this.querySelector('.bl-text-ellipsis'); return label ? label.textContent.trim() : ''; }";
const ROW_DISABLED: &str = "function() { return this.disabled || this.classList.contains('Mui-disabled'); }";
const SCROLL_TO_CENTER: &str = "function() { this.scrollIntoView({block: 'center'}); }";

impl UnitPage for Lms {
    async fn click_row(&self, title: &str) -> PageResult<bool> {
        for button in self.page.find_elements("button.bl-w-full").await? {
            // textContent, not innerText, to match read_rows (innerText collapses non-breaking spaces).
            let text = button.call_js_fn(ROW_TITLE, false).await?.result.value;
            if text.as_ref().and_then(Value::as_str).is_none_or(|t| t.is_empty() || t != title) {
                continue;
            }
            if button.call_js_fn(ROW_DISABLED, false).await?.result.value == Some(json!(true)) {
                continue;
            }
            button.call_js_fn(SCROLL_TO_CENTER, false).await?;
            self.sleep(200).await;
            // CDP mouse press and release at the element's center, like a Selenium Actions click.
            button.click().await?;
            return Ok(true);
        }
        Ok(false)
    }
}

/// Opens each unfinished row in turn on the current unit page until all are done or one fails.
pub async fn run<P: UnitPage, R: ActivityRunners<P>>(page: &P, runners: &mut R) -> PageResult<UnitOutcome> {
    let unit_url = page.current_url().await?;
    let mut attempted = HashSet::new();

    loop {
        let rows = page.read_rows().await?;
        if rows.is_empty() {
            logger::warn("No unit activity rows found — stopping", []);
            return Ok(UnitOutcome::Stuck(UnitStuck::NoRows));
        }
        let title = match next_row(&rows, &attempted) {
            NextRow::Open(row) => row.title.clone(),
            NextRow::AllDone => {
                logger::info("No unfinished activities left in this unit — done", [("total", json!(rows.len()))]);
                return Ok(UnitOutcome::Complete { total: rows.len() });
            }
            NextRow::Uncompletable(stuck) => {
                logger::warn("Remaining activities could not be auto-completed — done", [("stuck", json!(stuck))]);
                return Ok(UnitOutcome::Stuck(UnitStuck::Uncompletable { stuck }));
            }
        };
        attempted.insert(title.clone());

        logger::info("Opening activity", [("title", json!(title))]);
        let before_url = page.current_url().await?;
        if !page.click_row(&title).await? {
            logger::warn("Could not click activity row — stopping", [("title", json!(title))]);
            return Ok(UnitOutcome::Stuck(UnitStuck::RowNotClicked { title }));
        }

        if page.wait_for_url_change(&before_url, Duration::from_secs(10)).await.is_none() {
            logger::warn("Activity did not open (URL never changed) — stopping", [("title", json!(title))]);
            return Ok(UnitOutcome::Stuck(UnitStuck::RowDidNotOpen { title }));
        }

        if page.click_start_gate(Duration::from_secs(8)).await? {
            logger::info("Clicked past the Start gate", [("title", json!(title))]);
            page.sleep(1500).await;
        }

        // Reading rows can show several option groups at once. Only the reading runner scopes clicks per group.
        let mode = if title == "Reading" { Some(ActivityKind::Reading) } else { page.detect_mode().await? };
        let Some(mode) = mode else {
            let kind = page.complete_media_activity().await?;
            logger::info("Completed media activity", [("title", json!(title)), ("kind", json!(kind.name()))]);
            // Give the SPA time to POST the video's ended event before reloading.
            if kind == MediaKind::Video {
                page.sleep(3000).await;
            }
            page.get(&unit_url).await?;
            page.sleep(1500).await;
            continue;
        };

        logger::info("Running activity", [("title", json!(title)), ("mode", json!(mode.name()))]);
        let report = runners.run(mode, page).await?;
        let mut finished = vec![("title", json!(title)), ("mode", json!(mode.name()))];
        finished.extend(report.fields());
        logger::info("Activity finished", finished);

        let outcome = report.outcome;
        if outcome.stops_unit() {
            logger::warn(
                "Activity did not finish cleanly — stopping the unit run so it can be inspected",
                [("title", json!(title)), ("status", json!(outcome.status()))],
            );
            return Ok(UnitOutcome::Stuck(UnitStuck::Activity { title, outcome }));
        }

        // Same backend-sync grace before leaving a finished activity.
        page.sleep(3000).await;
        page.get(&unit_url).await?;
        page.sleep(1500).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runners::ActivityReport;
    use std::cell::{Cell, RefCell};

    /// Answers each script by what it contains, like the JS tests' executeScript fakes.
    struct FakeUnit<F: Fn(&str) -> Value> {
        script: F,
        clicked: RefCell<Vec<String>>,
        url: RefCell<String>,
    }

    impl<F: Fn(&str) -> Value> FakeUnit<F> {
        fn new(script: F) -> Self {
            FakeUnit {
                script,
                clicked: RefCell::default(),
                url: RefCell::new("https://lms.binus.ac.id/unit".to_string()),
            }
        }
    }

    impl<F: Fn(&str) -> Value> Driver for FakeUnit<F> {
        async fn current_url(&self) -> PageResult<String> {
            Ok(self.url.borrow().clone())
        }
        async fn execute_script(&self, body: &str, _args: &[Value]) -> PageResult<Value> {
            Ok((self.script)(body))
        }
        async fn get(&self, url: &str) -> PageResult<()> {
            *self.url.borrow_mut() = url.to_string();
            Ok(())
        }
        async fn sleep(&self, _ms: u64) {}
    }

    /// Frame scripts reach the fake with a marker, so one closure can answer both documents.
    impl<F: Fn(&str) -> Value> FramedPage for FakeUnit<F> {
        async fn execute_in_frame_at(&self, index: usize, body: &str, _args: &[Value]) -> PageResult<Value> {
            Ok((self.script)(&format!("/*frame {index}*/{body}")))
        }
        async fn reload(&self) -> PageResult<()> {
            Ok(())
        }
    }

    /// Runners that return a fixed report.
    struct FixedRunners(ActivityReport);

    impl<P> ActivityRunners<P> for FixedRunners {
        async fn run(&mut self, _kind: ActivityKind, _page: &P) -> PageResult<ActivityReport> {
            Ok(self.0)
        }
    }

    fn unhandled() -> FixedRunners {
        FixedRunners(ActivityReport::new(ActivityOutcome::Unhandled))
    }

    impl<F: Fn(&str) -> Value> UnitPage for FakeUnit<F> {
        async fn click_row(&self, title: &str) -> PageResult<bool> {
            self.clicked.borrow_mut().push(title.to_string());
            *self.url.borrow_mut() = format!("https://lms.binus.ac.id/unit/content/{title}");
            Ok(true)
        }
        async fn click_start_gate(&self, _timeout: Duration) -> PageResult<bool> {
            Ok(false)
        }
    }

    fn row(title: &str, disabled: bool, completed: bool) -> Row {
        Row { title: title.to_string(), disabled, completed }
    }

    #[tokio::test]
    async fn waits_until_video_playback_actually_ends() {
        let status_checks = Cell::new(0);
        let page = FakeUnit::new(|code: &str| {
            if code.contains("const vids") && code.contains("v.play()") && !code.contains("v.ended") {
                return json!(1);
            }
            if code.contains("v.ended") {
                status_checks.set(status_checks.get() + 1);
                return json!(status_checks.get() >= 3);
            }
            json!(0)
        });
        assert_eq!(page.complete_media_activity().await.unwrap(), MediaKind::Video);
        assert_eq!(status_checks.get(), 3);
    }

    #[tokio::test]
    async fn run_unit_does_not_complete_when_zero_rows_are_found() {
        let page = FakeUnit::new(|_: &str| json!([]));
        assert_eq!(run(&page, &mut unhandled()).await.unwrap(), UnitOutcome::Stuck(UnitStuck::NoRows));
    }

    #[tokio::test]
    async fn run_unit_reports_complete_without_opening_anything_when_every_row_is_checked() {
        let page = FakeUnit::new(|code: &str| {
            assert!(code.contains("button.bl-w-full"), "only the row read runs: {code}");
            json!([
                { "title": "Video", "disabled": false, "completed": true },
                { "title": "Reading", "disabled": false, "completed": true },
            ])
        });
        assert_eq!(run(&page, &mut unhandled()).await.unwrap(), UnitOutcome::Complete { total: 2 });
        assert!(page.clicked.borrow().is_empty());
    }

    #[tokio::test]
    async fn run_unit_stops_on_an_activity_runner_that_did_not_finish_cleanly() {
        let page = FakeUnit::new(|code: &str| {
            if code.contains("checkmarkPath") {
                return json!([
                    { "title": "Video", "disabled": false, "completed": true },
                    { "title": "Reading", "disabled": false, "completed": false },
                    { "title": "Listening", "disabled": true, "completed": false },
                ]);
            }
            json!(false)
        });
        let result = run(&page, &mut unhandled()).await.unwrap();
        assert_eq!(*page.clicked.borrow(), vec!["Reading"]);
        assert_eq!(
            result,
            UnitOutcome::Stuck(UnitStuck::Activity {
                title: "Reading".to_string(),
                outcome: ActivityOutcome::Unhandled
            })
        );
    }

    #[test]
    fn next_row_skips_locked_completed_and_tried_rows() {
        let rows = vec![
            row("Video", false, true),
            row("Bee Notes", false, false),
            row("Interactive Grammar Activity", false, false),
            row("Reading", true, false),
        ];
        let mut tried = HashSet::new();
        assert_eq!(next_row(&rows, &tried), NextRow::Open(&rows[1]));
        tried.insert("Bee Notes".to_string());
        assert_eq!(next_row(&rows, &tried), NextRow::Open(&rows[2]));
        tried.insert("Interactive Grammar Activity".to_string());
        assert_eq!(
            next_row(&rows, &tried),
            NextRow::Uncompletable(vec!["Bee Notes".to_string(), "Interactive Grammar Activity".to_string()])
        );
    }

    #[test]
    fn next_row_is_all_done_when_only_completed_or_locked_rows_remain() {
        let rows = vec![row("Video", false, true), row("Reading", true, false)];
        assert_eq!(next_row(&rows, &HashSet::new()), NextRow::AllDone);
    }

    #[tokio::test]
    async fn detect_mode_classifies_iframe_and_top_level_markup() {
        let iframe_reading = FakeUnit::new(|code: &str| {
            if code == IFRAME_COUNT {
                return json!(1);
            }
            if code.starts_with("/*frame 0*/") && code.contains("outerHTML") {
                return json!(
                    "<html><button class=\"MuiButtonBase-root bl-w-full justify-content-start\"></button></html>"
                );
            }
            json!(null)
        });
        assert_eq!(iframe_reading.detect_mode().await.unwrap(), Some(ActivityKind::Reading));
        let lti = FakeUnit::new(|code: &str| {
            if code == IFRAME_COUNT {
                return json!(1);
            }
            if code.starts_with("/*frame 0*/") {
                return json!("<html><input class=\"quiz-input-sa\"></html>");
            }
            json!(null)
        });
        assert_eq!(lti.detect_mode().await.unwrap(), Some(ActivityKind::Iframe));
        let native = FakeUnit::new(|code: &str| {
            if code == IFRAME_COUNT {
                return json!(0);
            }
            if code.contains("outerHTML") {
                return json!("<button class=\"bl-w-full justify-content-start\">A</button>");
            }
            json!({ "pills": ["1"], "optionLetters": ["A"] })
        });
        assert_eq!(native.detect_mode().await.unwrap(), Some(ActivityKind::Exercise));
    }

    #[tokio::test]
    async fn complete_media_activity_plays_a_video_inside_an_iframe_and_polls_that_frame_until_it_ends() {
        let status_checks = Cell::new(0);
        let page = FakeUnit::new(|code: &str| {
            if code == IFRAME_COUNT {
                return json!(2);
            }
            let in_second_frame = code.starts_with("/*frame 1*/");
            if code.contains("v.ended") {
                assert!(in_second_frame, "the ended poll stays in the frame that played: {code}");
                status_checks.set(status_checks.get() + 1);
                return json!(status_checks.get() >= 2);
            }
            if code.contains("v.play()") {
                return json!(if in_second_frame { 1 } else { 0 });
            }
            json!(0)
        });
        assert_eq!(page.complete_media_activity().await.unwrap(), MediaKind::Video);
        assert_eq!(status_checks.get(), 2);
    }
}
