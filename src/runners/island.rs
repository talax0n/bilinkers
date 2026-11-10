//! One level up from the course runner: surveys the island map (Beelingua home) and runs each
//! unfinished course in order, the Rust take on the deleted `scripts/run-level.js`.
//! Site behavior this relies on: docs/platform-notes.md#island-map-beelingua-home

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};

use super::course::{self, CourseOptions, CourseOutcome, CoursePage};
use super::{Live, Report};
use crate::browser::{Driver, Lms, PageError, PageResult};
use crate::logger;

/// Where a course stands, as the island map shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CourseState {
    Complete,
    Unfinished,
    /// Not open yet, or no panel opened for it. Skipped.
    Locked,
}

impl CourseState {
    pub fn name(self) -> &'static str {
        match self {
            CourseState::Complete => "complete",
            CourseState::Unfinished => "unfinished",
            CourseState::Locked => "locked",
        }
    }

    /// The node image's `alt`, which the site sets to the course's status. None for a value
    /// it was never seen to use, so the panel decides.
    pub fn from_marker(alt: &str) -> Option<CourseState> {
        match alt {
            "done" => Some(CourseState::Complete),
            "locked" => Some(CourseState::Locked),
            "progress" | "notStarted" => Some(CourseState::Unfinished),
            _ => None,
        }
    }
}

/// How a course's state was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Marker,
    Panel,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::Marker => "marker",
            Source::Panel => "panel",
        }
    }
}

/// A `.hoverable-pointer` on the island map: whitespace-free label and its image's `alt`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct IslandNode {
    pub label: String,
    pub marker: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CourseSurvey {
    pub label: String,
    pub state: CourseState,
}

/// The side panel a pointer click on a course opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    /// "View Unit List": unfinished.
    UnitList,
    /// Only "View Course Detail": complete.
    CourseDetail,
}

static COURSE_LABEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([BC])([0-9])\.([0-9])$").unwrap());

/// Course labels only, in course order (B1.1, B1.2, B2.1 ... C2.2), whatever the DOM order.
pub fn island_order(nodes: Vec<IslandNode>) -> Vec<IslandNode> {
    let mut courses: Vec<(String, IslandNode)> = nodes
        .into_iter()
        .filter_map(|n| {
            let caps = COURSE_LABEL.captures(&n.label)?;
            Some((format!("{}{}{}", &caps[1], &caps[2], &caps[3]), n))
        })
        .collect();
    courses.sort_by(|a, b| a.0.cmp(&b.0));
    courses.into_iter().map(|(_, n)| n).collect()
}

/// The first unfinished course in order that this run has not opened yet.
pub fn next_course<'a>(survey: &'a [CourseSurvey], attempted: &[String]) -> Option<&'a CourseSurvey> {
    survey.iter().find(|c| c.state == CourseState::Unfinished && !attempted.contains(&c.label))
}

const READ_ISLAND: &str = r#"
  return [...document.querySelectorAll('.hoverable-pointer')].map((el) => {
    const img = el.querySelector('img');
    return { label: el.textContent.replace(/\s+/g, '').trim(), marker: img ? img.getAttribute('alt') : null };
  });
"#;

const READ_PANEL: &str = r#"
  if (!document.body.innerText.includes('ENG-' + arguments[0])) return null;
  const texts = [...document.querySelectorAll('button')].map((b) => b.textContent.trim());
  if (texts.includes('View Unit List')) return 'unit-list';
  if (texts.includes('View Course Detail')) return 'course-detail';
  return null;
"#;

const NODE_LABEL: &str = "function() { return this.textContent.replace(/\\s+/g, '').trim(); }";
const BUTTON_TEXT: &str = "function() { return this.textContent.trim(); }";
const SCROLL_TO_CENTER: &str = "function() { this.scrollIntoView({block: 'center'}); }";

/// The island map. A JS click() on a node does nothing, so nodes and panel buttons get real pointer clicks.
#[allow(async_fn_in_trait)]
pub trait IslandPage: CoursePage {
    async fn read_island(&self) -> PageResult<Vec<IslandNode>> {
        let raw = self.execute_script(READ_ISLAND, &[]).await?;
        let nodes: Vec<IslandNode> =
            serde_json::from_value(raw).map_err(|e| PageError(format!("island nodes: {e}")))?;
        Ok(island_order(nodes))
    }

    /// The panel open for `label` right now, if any.
    async fn read_panel(&self, label: &str) -> PageResult<Option<Panel>> {
        Ok(match self.execute_script(READ_PANEL, &[json!(label)]).await?.as_str() {
            Some("unit-list") => Some(Panel::UnitList),
            Some("course-detail") => Some(Panel::CourseDetail),
            _ => None,
        })
    }

    /// Pointer-clicks the course node. False when no node has that label.
    async fn press_course(&self, label: &str) -> PageResult<bool>;

    /// Pointer-clicks the open panel's "View Unit List". False when there is none.
    async fn press_view_unit_list(&self) -> PageResult<bool>;
}

impl Lms {
    async fn press_first(&self, selector: &str, read_text: &str, wanted: &str) -> PageResult<bool> {
        for element in self.page.find_elements(selector).await? {
            let text = element.call_js_fn(read_text, false).await?.result.value;
            if text.as_ref().and_then(Value::as_str) != Some(wanted) {
                continue;
            }
            element.call_js_fn(SCROLL_TO_CENTER, false).await?;
            self.sleep(200).await;
            element.click().await?;
            return Ok(true);
        }
        Ok(false)
    }
}

impl IslandPage for Lms {
    async fn press_course(&self, label: &str) -> PageResult<bool> {
        self.press_first(".hoverable-pointer", NODE_LABEL, label).await
    }

    async fn press_view_unit_list(&self) -> PageResult<bool> {
        self.press_first("button", BUTTON_TEXT, "View Unit List").await
    }
}

/// What the island runner hands an opened course to.
#[allow(async_fn_in_trait)]
pub trait CourseRunner<P> {
    async fn course(&mut self, page: &P) -> PageResult<CourseOutcome>;
}

impl<P: super::unit::UnitPage + super::checkpoint::CheckpointPage + CoursePage> CourseRunner<P> for Live {
    async fn course(&mut self, page: &P) -> PageResult<CourseOutcome> {
        course::run(page, self, CourseOptions::default()).await
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum IslandStuck {
    MissingMap,
    /// The panel never offered "View Unit List", or the click on it did not navigate.
    CourseDidNotOpen {
        label: String,
    },
    CourseFailed {
        label: String,
        outcome: CourseOutcome,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum IslandOutcome {
    /// Every course that is not locked was already complete. Nothing was opened.
    AllCoursesComplete {
        courses: Vec<CourseSurvey>,
    },
    /// Each unfinished course was run once. One can still show unfinished, for a Final Test the bot never opens.
    Complete {
        courses: Vec<CourseSurvey>,
        ran: Vec<String>,
    },
    Stuck(IslandStuck),
}

fn courses_json(courses: &[CourseSurvey]) -> Value {
    json!(courses.iter().map(|c| json!({ "label": c.label, "state": c.state.name() })).collect::<Vec<_>>())
}

impl Report for IslandOutcome {
    fn status(&self) -> &'static str {
        match self {
            IslandOutcome::AllCoursesComplete { .. } => "all-courses-complete",
            IslandOutcome::Complete { .. } => "complete",
            IslandOutcome::Stuck(_) => "stuck",
        }
    }

    fn fields(&self) -> Vec<(&'static str, Value)> {
        let mut fields = vec![("status", json!(self.status()))];
        match self {
            IslandOutcome::AllCoursesComplete { courses } => fields.push(("courses", courses_json(courses))),
            IslandOutcome::Complete { courses, ran } => {
                fields.extend([("courses", courses_json(courses)), ("ran", json!(ran))]);
            }
            IslandOutcome::Stuck(IslandStuck::MissingMap) => fields.push(("reason", json!("missing-map"))),
            IslandOutcome::Stuck(IslandStuck::CourseDidNotOpen { label }) => fields.push(("label", json!(label))),
            IslandOutcome::Stuck(IslandStuck::CourseFailed { label, outcome }) => {
                let result: serde_json::Map<String, Value> =
                    outcome.fields().into_iter().map(|(k, v)| (k.to_string(), v)).collect();
                fields.extend([("label", json!(label)), ("courseResult", Value::Object(result))]);
            }
        }
        fields
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IslandOptions {
    /// How long to poll for the island map after loading it.
    pub read_retry: Duration,
    /// How long a pointer click gets to open the course's panel.
    pub panel_wait: Duration,
}

impl Default for IslandOptions {
    fn default() -> Self {
        IslandOptions { read_retry: Duration::from_secs(8), panel_wait: Duration::from_secs(5) }
    }
}

/// The map may still be loading right after a navigation, so an empty read is retried.
async fn wait_for_island<P: IslandPage>(page: &P, options: IslandOptions) -> PageResult<Vec<IslandNode>> {
    let deadline = Instant::now() + options.read_retry;
    loop {
        let nodes = page.read_island().await?;
        if !nodes.is_empty() || Instant::now() > deadline {
            return Ok(nodes);
        }
        page.sleep(300).await;
    }
}

/// Pointer-clicks the node and waits for its panel. None means no panel came: locked.
async fn open_panel<P: IslandPage>(page: &P, label: &str, options: IslandOptions) -> PageResult<Option<Panel>> {
    if !page.press_course(label).await? {
        return Ok(None);
    }
    let deadline = Instant::now() + options.panel_wait;
    loop {
        if let Some(panel) = page.read_panel(label).await? {
            return Ok(Some(panel));
        }
        if Instant::now() > deadline {
            return Ok(None);
        }
        page.sleep(200).await;
    }
}

/// Every course in order with its state. The marker is read when it is one the site was seen
/// to use; otherwise the panel decides, and home is reloaded since an open panel covers nodes.
async fn survey<P: IslandPage>(
    page: &P,
    nodes: Vec<IslandNode>,
    home: &str,
    options: IslandOptions,
) -> PageResult<Vec<CourseSurvey>> {
    let mut courses = Vec::new();
    for node in nodes {
        let (state, source) = match node.marker.as_deref().and_then(CourseState::from_marker) {
            Some(state) => (state, Source::Marker),
            None => {
                let state = match open_panel(page, &node.label, options).await? {
                    Some(Panel::UnitList) => CourseState::Unfinished,
                    Some(Panel::CourseDetail) => CourseState::Complete,
                    None => CourseState::Locked,
                };
                page.get(home).await?;
                page.sleep(1500).await;
                wait_for_island(page, options).await?;
                (state, Source::Panel)
            }
        };
        logger::info(
            "Course checked",
            [("label", json!(node.label)), ("state", json!(state.name())), ("via", json!(source.name()))],
        );
        courses.push(CourseSurvey { label: node.label, state });
    }
    Ok(courses)
}

/// Surveys the island map on the current tab and runs each unfinished course in order until
/// none is left or one gets stuck. Locked courses are skipped; a Final Test is never opened.
pub async fn run<P: IslandPage, R: CourseRunner<P>>(
    page: &P,
    runners: &mut R,
    options: IslandOptions,
) -> PageResult<IslandOutcome> {
    let home = page.current_url().await?;
    let mut attempted: Vec<String> = Vec::new();

    loop {
        let nodes = wait_for_island(page, options).await?;
        if nodes.is_empty() {
            logger::warn("Island map could not be read — stopping", []);
            return Ok(IslandOutcome::Stuck(IslandStuck::MissingMap));
        }
        let courses = survey(page, nodes, &home, options).await?;

        let Some(next) = next_course(&courses, &attempted) else {
            if attempted.is_empty() {
                logger::info("Every course is already finished — nothing to do", [("courses", courses_json(&courses))]);
                return Ok(IslandOutcome::AllCoursesComplete { courses });
            }
            logger::info("No unfinished courses left to run — done", [("courses", courses_json(&courses))]);
            return Ok(IslandOutcome::Complete { courses, ran: attempted });
        };
        let label = next.label.clone();
        attempted.push(label.clone());

        logger::info("Opening course", [("label", json!(label))]);
        let before_url = page.current_url().await?;
        let opened = open_panel(page, &label, options).await? == Some(Panel::UnitList)
            && page.press_view_unit_list().await?
            && page.wait_for_url_change(&before_url, Duration::from_secs(10)).await.is_some();
        if !opened {
            logger::warn("Course did not open — stopping", [("label", json!(label))]);
            return Ok(IslandOutcome::Stuck(IslandStuck::CourseDidNotOpen { label }));
        }
        page.sleep(1500).await;

        let outcome = runners.course(page).await?;
        let mut finished = vec![("label", json!(label))];
        finished.extend(outcome.fields());
        logger::info("Course finished", finished);

        if let CourseOutcome::Stuck(_) = outcome {
            logger::warn(
                "Course did not finish cleanly — stopping the island run so it can be inspected",
                [("label", json!(label))],
            );
            return Ok(IslandOutcome::Stuck(IslandStuck::CourseFailed { label, outcome }));
        }

        page.get(&home).await?;
        page.sleep(1500).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runners::course::{Count, CourseNode, CourseStuck, Progress};
    use std::cell::RefCell;
    use std::collections::HashMap;

    const HOME: &str = "https://lms.binus.ac.id/beelingua/";
    const ORDER: [&str; 8] = ["B1.1", "B1.2", "B2.1", "B2.2", "C1.1", "C1.2", "C2.1", "C2.2"];

    fn node(label: &str, marker: Option<&str>) -> IslandNode {
        IslandNode { label: label.to_string(), marker: marker.map(str::to_string) }
    }

    /// An island map whose courses have a marker and, behind it, a panel (None: no panel opens).
    struct FakeIsland {
        nodes: RefCell<Vec<IslandNode>>,
        panels: HashMap<String, Option<Panel>>,
        /// The panel open right now.
        open: RefCell<Option<String>>,
        pressed: RefCell<Vec<String>>,
        urls: RefCell<Vec<String>>,
        /// Whether View Unit List navigates.
        navigates: bool,
    }

    impl FakeIsland {
        fn new(nodes: Vec<IslandNode>) -> Self {
            FakeIsland {
                nodes: RefCell::new(nodes),
                panels: HashMap::new(),
                open: RefCell::default(),
                pressed: RefCell::default(),
                urls: RefCell::new(vec![HOME.to_string()]),
                navigates: true,
            }
        }

        fn panel(mut self, label: &str, panel: Option<Panel>) -> Self {
            self.panels.insert(label.to_string(), panel);
            self
        }

        fn set_marker(&self, label: &str, marker: &str) {
            for n in self.nodes.borrow_mut().iter_mut().filter(|n| n.label == label) {
                n.marker = Some(marker.to_string());
            }
        }
    }

    impl Driver for FakeIsland {
        async fn current_url(&self) -> PageResult<String> {
            Ok(self.urls.borrow().last().cloned().unwrap_or_default())
        }
        async fn execute_script(&self, _body: &str, _args: &[Value]) -> PageResult<Value> {
            unreachable!("the island fake overrides every page read")
        }
        async fn get(&self, url: &str) -> PageResult<()> {
            *self.open.borrow_mut() = None;
            self.urls.borrow_mut().push(url.to_string());
            Ok(())
        }
        async fn sleep(&self, _ms: u64) {}
        async fn wait_for_url_change(&self, _previous: &str, _timeout: Duration) -> Option<String> {
            self.navigates.then(|| "https://lms.binus.ac.id/beelingua/student/class/1".to_string())
        }
    }

    impl CoursePage for FakeIsland {
        async fn read_course_nodes(&self) -> PageResult<Vec<CourseNode>> {
            unreachable!("the course runner is faked")
        }
        async fn read_progress(&self) -> PageResult<Progress> {
            unreachable!("the course runner is faked")
        }
    }

    impl IslandPage for FakeIsland {
        async fn read_island(&self) -> PageResult<Vec<IslandNode>> {
            Ok(island_order(self.nodes.borrow().clone()))
        }
        async fn read_panel(&self, label: &str) -> PageResult<Option<Panel>> {
            let open = self.open.borrow().clone();
            Ok(open.filter(|o| o == label).and_then(|o| self.panels.get(&o).copied().flatten()))
        }
        async fn press_course(&self, label: &str) -> PageResult<bool> {
            self.pressed.borrow_mut().push(label.to_string());
            *self.open.borrow_mut() = Some(label.to_string());
            Ok(self.nodes.borrow().iter().any(|n| n.label == label))
        }
        async fn press_view_unit_list(&self) -> PageResult<bool> {
            let open = self.open.borrow().clone();
            Ok(open.is_some_and(|o| self.panels.get(&o) == Some(&Some(Panel::UnitList))))
        }
    }

    /// Finishes each course it runs (its marker turns done), or returns a fixed outcome.
    struct FakeCourses {
        ran: Vec<String>,
        outcome: Option<CourseOutcome>,
    }

    impl FakeCourses {
        fn finishing() -> Self {
            FakeCourses { ran: Vec::new(), outcome: None }
        }
    }

    fn done_course() -> CourseOutcome {
        CourseOutcome::Complete { unit: Count { done: 8, total: 8 }, checkpoint: Count { done: 3, total: 3 } }
    }

    impl CourseRunner<FakeIsland> for FakeCourses {
        async fn course(&mut self, page: &FakeIsland) -> PageResult<CourseOutcome> {
            let label = page.open.borrow().clone().expect("a course panel was open");
            self.ran.push(label.clone());
            if let Some(outcome) = &self.outcome {
                return Ok(outcome.clone());
            }
            page.set_marker(&label, "done");
            Ok(done_course())
        }
    }

    fn quick() -> IslandOptions {
        IslandOptions { read_retry: Duration::from_millis(20), panel_wait: Duration::from_millis(20) }
    }

    fn all_done() -> Vec<IslandNode> {
        ORDER.iter().map(|l| node(l, Some("done"))).collect()
    }

    fn states(courses: &[CourseSurvey]) -> Vec<(&str, CourseState)> {
        courses.iter().map(|c| (c.label.as_str(), c.state)).collect()
    }

    #[test]
    fn the_island_is_read_in_course_order_whatever_the_dom_order() {
        let shuffled = ["C2.2", "B2.1", "Unit1", "C1.1", "B1.2", "FinalTest", "B1.1", "C2.1", "B2.2", "C1.2", ""];
        let ordered = island_order(shuffled.iter().map(|l| node(l, None)).collect());
        let labels: Vec<&str> = ordered.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(labels, ORDER, "only course labels, B1.1 first and C2.2 last");
    }

    #[test]
    fn the_marker_is_the_courses_status_and_unknown_values_fall_back_to_the_panel() {
        assert_eq!(CourseState::from_marker("done"), Some(CourseState::Complete));
        assert_eq!(CourseState::from_marker("locked"), Some(CourseState::Locked));
        assert_eq!(CourseState::from_marker("progress"), Some(CourseState::Unfinished));
        assert_eq!(CourseState::from_marker("notStarted"), Some(CourseState::Unfinished));
        assert_eq!(CourseState::from_marker("map"), None);
        assert_eq!(CourseState::from_marker(""), None);
    }

    #[test]
    fn next_course_is_the_first_unfinished_one_not_yet_run() {
        let survey = |label: &str, state| CourseSurvey { label: label.to_string(), state };
        let courses = vec![
            survey("B1.1", CourseState::Complete),
            survey("B1.2", CourseState::Locked),
            survey("B2.1", CourseState::Unfinished),
            survey("B2.2", CourseState::Unfinished),
        ];
        assert_eq!(next_course(&courses, &[]).map(|c| c.label.as_str()), Some("B2.1"));
        assert_eq!(next_course(&courses, &["B2.1".to_string()]).map(|c| c.label.as_str()), Some("B2.2"));
        assert_eq!(next_course(&courses, &["B2.1".to_string(), "B2.2".to_string()]), None);
    }

    #[tokio::test]
    async fn a_finished_island_is_all_courses_complete_without_a_single_click() {
        let page = FakeIsland::new(all_done());
        let mut courses = FakeCourses::finishing();
        let result = run(&page, &mut courses, quick()).await.unwrap();
        let IslandOutcome::AllCoursesComplete { courses: surveyed } = &result else { panic!("{result:?}") };
        assert_eq!(states(surveyed), ORDER.map(|l| (l, CourseState::Complete)));
        assert!(page.pressed.borrow().is_empty(), "the markers say it all");
        assert!(courses.ran.is_empty());
        assert_eq!(result.status(), "all-courses-complete");
    }

    #[tokio::test]
    async fn locked_courses_are_skipped_and_do_not_stop_all_courses_complete() {
        let mut nodes = all_done();
        nodes[7].marker = Some("locked".into());
        // An unknown marker falls back to the panel; no panel means locked.
        nodes[6].marker = None;
        let page = FakeIsland::new(nodes).panel("C2.1", None);
        let result = run(&page, &mut FakeCourses::finishing(), quick()).await.unwrap();
        let IslandOutcome::AllCoursesComplete { courses } = &result else { panic!("{result:?}") };
        assert_eq!(courses[6].state, CourseState::Locked);
        assert_eq!(courses[7].state, CourseState::Locked);
        assert_eq!(*page.pressed.borrow(), vec!["C2.1"], "a locked marker is never clicked");
        assert_eq!(page.urls.borrow().last().map(String::as_str), Some(HOME), "home is reloaded after a panel");
    }

    #[tokio::test]
    async fn the_panel_decides_when_the_marker_is_unknown() {
        let mut nodes = all_done();
        nodes[0].marker = None;
        nodes[1].marker = None;
        let page = FakeIsland::new(nodes).panel("B1.1", Some(Panel::CourseDetail)).panel("B1.2", Some(Panel::UnitList));
        let mut courses = FakeCourses::finishing();
        let result = run(&page, &mut courses, quick()).await.unwrap();
        assert_eq!(courses.ran, vec!["B1.2"]);
        let IslandOutcome::Complete { courses: surveyed, ran } = &result else { panic!("{result:?}") };
        assert_eq!(surveyed[0].state, CourseState::Complete);
        assert_eq!(surveyed[1].state, CourseState::Complete, "its marker turned done after the run");
        assert_eq!(ran, &vec!["B1.2".to_string()]);
    }

    #[tokio::test]
    async fn unfinished_courses_run_in_order_then_the_run_is_complete() {
        let mut nodes = all_done();
        nodes[4].marker = Some("notStarted".into());
        nodes[2].marker = Some("progress".into());
        let page = FakeIsland::new(nodes).panel("B2.1", Some(Panel::UnitList)).panel("C1.1", Some(Panel::UnitList));
        let mut courses = FakeCourses::finishing();
        let result = run(&page, &mut courses, quick()).await.unwrap();
        assert_eq!(courses.ran, vec!["B2.1", "C1.1"]);
        let IslandOutcome::Complete { courses: surveyed, ran } = &result else { panic!("{result:?}") };
        assert!(surveyed.iter().all(|c| c.state == CourseState::Complete));
        assert_eq!(ran, &vec!["B2.1".to_string(), "C1.1".to_string()]);
        assert_eq!(result.status(), "complete");
    }

    #[tokio::test]
    async fn a_course_left_unfinished_is_run_once() {
        let mut nodes = all_done();
        nodes[0].marker = Some("progress".into());
        let page = FakeIsland::new(nodes).panel("B1.1", Some(Panel::UnitList));
        // Its Final Test is never opened, so the course stays unfinished on the island.
        let mut courses = FakeCourses { outcome: Some(done_course()), ..FakeCourses::finishing() };
        let result = run(&page, &mut courses, quick()).await.unwrap();
        assert_eq!(courses.ran, vec!["B1.1"]);
        let IslandOutcome::Complete { courses: surveyed, .. } = &result else { panic!("{result:?}") };
        assert_eq!(surveyed[0].state, CourseState::Unfinished);
    }

    #[tokio::test]
    async fn a_stuck_course_stops_the_island_run() {
        let mut nodes = all_done();
        nodes[0].marker = Some("progress".into());
        nodes[1].marker = Some("progress".into());
        let page = FakeIsland::new(nodes).panel("B1.1", Some(Panel::UnitList)).panel("B1.2", Some(Panel::UnitList));
        let stuck = CourseOutcome::Stuck(CourseStuck::NodeDidNotOpen { label: "Unit5".into() });
        let mut courses = FakeCourses { outcome: Some(stuck.clone()), ..FakeCourses::finishing() };
        let result = run(&page, &mut courses, quick()).await.unwrap();
        assert_eq!(courses.ran, vec!["B1.1"], "B1.2 is never opened");
        assert_eq!(result, IslandOutcome::Stuck(IslandStuck::CourseFailed { label: "B1.1".into(), outcome: stuck }));
    }

    #[tokio::test]
    async fn an_unfinished_course_that_does_not_open_stops_the_run() {
        let mut nodes = all_done();
        nodes[3].marker = Some("progress".into());
        // No panel at all, then a panel whose button does not navigate.
        let page = FakeIsland::new(nodes.clone()).panel("B2.2", None);
        let result = run(&page, &mut FakeCourses::finishing(), quick()).await.unwrap();
        assert_eq!(result, IslandOutcome::Stuck(IslandStuck::CourseDidNotOpen { label: "B2.2".into() }));
        let mut page = FakeIsland::new(nodes).panel("B2.2", Some(Panel::UnitList));
        page.navigates = false;
        let mut courses = FakeCourses::finishing();
        let result = run(&page, &mut courses, quick()).await.unwrap();
        assert_eq!(result, IslandOutcome::Stuck(IslandStuck::CourseDidNotOpen { label: "B2.2".into() }));
        assert!(courses.ran.is_empty());
    }

    #[tokio::test]
    async fn an_empty_page_is_not_a_finished_island() {
        let page = FakeIsland::new(vec![node("Unit1", Some("done"))]);
        let result = run(&page, &mut FakeCourses::finishing(), quick()).await.unwrap();
        assert_eq!(result, IslandOutcome::Stuck(IslandStuck::MissingMap));
    }
}
