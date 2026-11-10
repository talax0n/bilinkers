//! Port of `src/runners/course.js`.
//! Site behavior this relies on: docs/platform-notes.md#course-roadmap

use std::collections::HashSet;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::unit::UnitOutcome;
use super::{CheckpointOutcome, CourseNodeRunners, Report};
use crate::browser::{Driver, PageError, PageResult};
use crate::js;
use crate::logger;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Unit,
    Checkpoint,
}

impl NodeKind {
    pub fn name(self) -> &'static str {
        match self {
            NodeKind::Unit => "unit",
            NodeKind::Checkpoint => "checkpoint",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CourseNode {
    pub kind: NodeKind,
    pub number: u64,
    pub top: Option<f64>,
    pub label: String,
}

/// A `.hoverable-pointer` as read off the page: whitespace-free label, top offset if parseable.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RawNode {
    pub label: String,
    pub top: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Count {
    pub done: u64,
    pub total: u64,
}

/// The header readout, e.g. "Unit 4/8  CP 1/3  FT 0/1".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Progress {
    pub unit: Option<Count>,
    pub checkpoint: Option<Count>,
}

static UNIT_LABEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^Unit([0-9]+)$").unwrap());
static CHECKPOINT_LABEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^Checkpoint([0-9]+)$").unwrap());
static UNIT_PROGRESS: LazyLock<Regex> = LazyLock::new(|| {
    let ws = js::WS_CLASS;
    Regex::new(&format!(r"Unit{ws}*([0-9]+){ws}*/{ws}*([0-9]+)")).unwrap()
});
static CHECKPOINT_PROGRESS: LazyLock<Regex> = LazyLock::new(|| {
    let ws = js::WS_CLASS;
    Regex::new(&format!(r"CP{ws}*([0-9]+){ws}*/{ws}*([0-9]+)")).unwrap()
});

/// Whitelists `Unit N` and `Checkpoint N`, so the Final Test node is never opened,
/// and sorts by descending top, which is course order.
pub fn parse_course_nodes(raw: Vec<RawNode>) -> Vec<CourseNode> {
    let mut nodes: Vec<CourseNode> = raw
        .into_iter()
        .filter_map(|n| {
            let (kind, caps) = if let Some(caps) = UNIT_LABEL.captures(&n.label) {
                (NodeKind::Unit, caps)
            } else {
                (NodeKind::Checkpoint, CHECKPOINT_LABEL.captures(&n.label)?)
            };
            let number = caps[1].parse().ok()?;
            Some(CourseNode { kind, number, top: n.top, label: n.label.clone() })
        })
        .collect();
    // A node without a top sorts last; JS leaves NaN order to the engine.
    nodes.sort_by(|a, b| b.top.unwrap_or(f64::NEG_INFINITY).total_cmp(&a.top.unwrap_or(f64::NEG_INFINITY)));
    nodes
}

pub fn parse_progress(body_text: &str) -> Progress {
    let parse = |re: &Regex| {
        let caps = re.captures(body_text)?;
        Some(Count { done: caps[1].parse().ok()?, total: caps[2].parse().ok()? })
    };
    Progress { unit: parse(&UNIT_PROGRESS), checkpoint: parse(&CHECKPOINT_PROGRESS) }
}

/// The first node in course order past its type's done count and not yet tried this run.
pub fn next_node<'a>(
    nodes: &'a [CourseNode],
    progress: &Progress,
    attempted: &HashSet<String>,
) -> Option<&'a CourseNode> {
    let units_done = progress.unit.map_or(0, |c| c.done);
    let checkpoints_done = progress.checkpoint.map_or(0, |c| c.done);
    nodes.iter().find(|n| {
        let done = match n.kind {
            NodeKind::Unit => units_done,
            NodeKind::Checkpoint => checkpoints_done,
        };
        !attempted.contains(&n.label) && n.number > done
    })
}

const READ_NODES: &str = r#"
  return [...document.querySelectorAll('.hoverable-pointer')].map((el) => {
    const label = el.textContent.replace(/\s+/g, '').trim();
    const positioned = el.querySelector('[style*="top"]');
    const top = positioned ? parseFloat(positioned.style.top) : NaN;
    return { label, top: Number.isNaN(top) ? null : top };
  });
"#;

const CLICK_NODE: &str = r#"
  const target = String(arguments[0]);
  const nodes = [...document.querySelectorAll('.hoverable-pointer')];
  const el = nodes.find((n) => n.textContent.replace(/\s+/g, '').trim() === target);
  if (el) { el.click(); return true; }
  return false;
"#;

/// The roadmap page. Node state is baked into PNGs, so only labels, offsets and the header are read.
#[allow(async_fn_in_trait)]
pub trait CoursePage: Driver {
    async fn read_course_nodes(&self) -> PageResult<Vec<CourseNode>> {
        let raw = self.execute_script(READ_NODES, &[]).await?;
        let raw: Vec<RawNode> = serde_json::from_value(raw).map_err(|e| PageError(format!("roadmap nodes: {e}")))?;
        Ok(parse_course_nodes(raw))
    }

    async fn read_progress(&self) -> PageResult<Progress> {
        let text = self.execute_script("return document.body.innerText;", &[]).await?;
        Ok(parse_progress(text.as_str().unwrap_or_default()))
    }

    /// A JS click() on the node is enough here, unlike unit rows.
    async fn click_node(&self, label: &str) -> PageResult<bool> {
        Ok(self.execute_script(CLICK_NODE, &[json!(label)]).await? == json!(true))
    }
}

impl CoursePage for crate::browser::Lms {}

#[derive(Debug, Clone, PartialEq)]
pub enum NodeOutcome {
    Unit(UnitOutcome),
    Checkpoint(CheckpointOutcome),
}

impl NodeOutcome {
    fn stops_course(&self) -> bool {
        match self {
            NodeOutcome::Unit(outcome) => outcome.is_stuck(),
            NodeOutcome::Checkpoint(outcome) => outcome.stops_course(),
        }
    }
}

impl Report for NodeOutcome {
    fn status(&self) -> &'static str {
        match self {
            NodeOutcome::Unit(o) => o.status(),
            NodeOutcome::Checkpoint(o) => o.status(),
        }
    }

    fn fields(&self) -> Vec<(&'static str, Value)> {
        match self {
            NodeOutcome::Unit(o) => o.fields(),
            NodeOutcome::Checkpoint(o) => o.fields(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum CourseStuck {
    MissingRoadmap,
    NodeNotClicked {
        label: String,
    },
    /// Locked node, or the click hit decoration. Nothing more is reachable.
    NodeDidNotOpen {
        label: String,
    },
    NodeFailed {
        label: String,
        outcome: NodeOutcome,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum CourseOutcome {
    Complete { unit: Count, checkpoint: Count },
    Stuck(CourseStuck),
}

impl Report for CourseOutcome {
    fn status(&self) -> &'static str {
        match self {
            CourseOutcome::Complete { .. } => "complete",
            CourseOutcome::Stuck(_) => "stuck",
        }
    }

    fn fields(&self) -> Vec<(&'static str, Value)> {
        let mut fields = vec![("status", json!(self.status()))];
        match self {
            CourseOutcome::Complete { unit, checkpoint } => {
                fields.extend([("unit", json!(unit)), ("checkpoint", json!(checkpoint))]);
            }
            CourseOutcome::Stuck(CourseStuck::MissingRoadmap) => fields.push(("reason", json!("missing-roadmap"))),
            CourseOutcome::Stuck(CourseStuck::NodeNotClicked { label } | CourseStuck::NodeDidNotOpen { label }) => {
                fields.push(("label", json!(label)));
            }
            CourseOutcome::Stuck(CourseStuck::NodeFailed { label, outcome }) => {
                let node: serde_json::Map<String, Value> =
                    outcome.fields().into_iter().map(|(k, v)| (k.to_string(), v)).collect();
                fields.extend([("label", json!(label)), ("nodeResult", Value::Object(node))]);
            }
        }
        fields
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CourseOptions {
    /// How long to poll for the roadmap after navigating back to it.
    pub read_retry: Duration,
}

impl Default for CourseOptions {
    fn default() -> Self {
        CourseOptions { read_retry: Duration::from_secs(5) }
    }
}

fn with<'a>(mut head: Vec<(&'a str, Value)>, tail: Vec<(&'a str, Value)>) -> Vec<(&'a str, Value)> {
    head.extend(tail);
    head
}

/// Opens unfinished nodes in course order until none is left or one fails, on the current roadmap tab.
pub async fn run<P: CoursePage, R: CourseNodeRunners<P>>(
    page: &P,
    runners: &mut R,
    options: CourseOptions,
) -> PageResult<CourseOutcome> {
    let course_url = page.current_url().await?;
    let mut attempted = HashSet::new();

    loop {
        // The roadmap may still be loading after navigating back, so poll before giving up.
        let read_deadline = Instant::now() + options.read_retry;
        let (nodes, progress) = loop {
            let nodes = page.read_course_nodes().await?;
            let progress = page.read_progress().await?;
            let ok = !nodes.is_empty() && progress.unit.is_some() && progress.checkpoint.is_some();
            if ok || Instant::now() > read_deadline {
                break (nodes, progress);
            }
            page.sleep(300).await;
        };
        let (Some(unit), Some(checkpoint), false) = (progress.unit, progress.checkpoint, nodes.is_empty()) else {
            logger::warn(
                "Course roadmap or progress could not be read — stopping",
                [("nodes", json!(nodes.len())), ("progress", json!(progress))],
            );
            return Ok(CourseOutcome::Stuck(CourseStuck::MissingRoadmap));
        };

        let Some(next) = next_node(&nodes, &progress, &attempted) else {
            logger::info(
                "No unfinished units or checkpoints left in this course — done",
                [("unit", json!(unit)), ("checkpoint", json!(checkpoint))],
            );
            return Ok(CourseOutcome::Complete { unit, checkpoint });
        };
        let label = next.label.clone();
        attempted.insert(label.clone());
        let node_fields = || vec![("label", json!(label)), ("type", json!(next.kind.name()))];

        logger::info("Opening course node", node_fields());
        let before_url = page.current_url().await?;
        if !page.click_node(&label).await? {
            logger::warn("Could not click course node — stopping", [("label", json!(label))]);
            return Ok(CourseOutcome::Stuck(CourseStuck::NodeNotClicked { label }));
        }

        if page.wait_for_url_change(&before_url, Duration::from_secs(10)).await.is_none() {
            logger::warn("Course node did not open (URL never changed) — stopping", [("label", json!(label))]);
            return Ok(CourseOutcome::Stuck(CourseStuck::NodeDidNotOpen { label }));
        }
        page.sleep(1500).await;

        logger::info("Running course node", node_fields());
        let outcome = match next.kind {
            NodeKind::Unit => NodeOutcome::Unit(runners.unit(page).await?),
            NodeKind::Checkpoint => NodeOutcome::Checkpoint(runners.checkpoint(page).await?),
        };
        logger::info("Course node finished", with(node_fields(), outcome.fields()));

        if outcome.stops_course() {
            logger::warn(
                "Course node did not finish cleanly — stopping the course run so it can be inspected",
                with(node_fields(), vec![("status", json!(outcome.status()))]),
            );
            return Ok(CourseOutcome::Stuck(CourseStuck::NodeFailed { label, outcome }));
        }

        page.get(&course_url).await?;
        page.sleep(1500).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runners::unit::{UnitOutcome, UnitStuck};
    use std::cell::RefCell;

    fn node(kind: NodeKind, number: u64, top: f64, label: &str) -> CourseNode {
        CourseNode { kind, number, top: Some(top), label: label.to_string() }
    }

    fn count(done: u64, total: u64) -> Option<Count> {
        Some(Count { done, total })
    }

    /// The JS test's injected readCourseNodesFn/readProgressFn/clickNodeFn/waitForUrlChangeFn.
    struct FakeCourse {
        nodes: Vec<CourseNode>,
        progress: RefCell<Progress>,
        opened: RefCell<Vec<String>>,
        urls: RefCell<Vec<String>>,
        opens: bool,
    }

    impl FakeCourse {
        fn new(nodes: Vec<CourseNode>, progress: Progress) -> Self {
            FakeCourse {
                nodes,
                progress: RefCell::new(progress),
                opened: RefCell::default(),
                urls: RefCell::new(vec!["https://lms.binus.ac.id/course".to_string()]),
                opens: true,
            }
        }
    }

    impl Driver for FakeCourse {
        async fn current_url(&self) -> PageResult<String> {
            Ok(self.urls.borrow().last().cloned().unwrap_or_default())
        }
        async fn execute_script(&self, _body: &str, _args: &[Value]) -> PageResult<Value> {
            unreachable!("the course fake overrides every page read")
        }
        async fn get(&self, url: &str) -> PageResult<()> {
            self.urls.borrow_mut().push(url.to_string());
            Ok(())
        }
        async fn sleep(&self, _ms: u64) {}
        async fn wait_for_url_change(&self, _previous: &str, _timeout: Duration) -> Option<String> {
            self.opens.then(|| "https://lms.binus.ac.id/course/opened".to_string())
        }
    }

    impl CoursePage for FakeCourse {
        async fn read_course_nodes(&self) -> PageResult<Vec<CourseNode>> {
            Ok(self.nodes.clone())
        }
        async fn read_progress(&self) -> PageResult<Progress> {
            Ok(*self.progress.borrow())
        }
        async fn click_node(&self, label: &str) -> PageResult<bool> {
            self.opened.borrow_mut().push(label.to_string());
            Ok(true)
        }
    }

    impl crate::browser::FramedPage for FakeCourse {
        async fn execute_in_frame_at(&self, _index: usize, _body: &str, _args: &[Value]) -> PageResult<Value> {
            unreachable!("no activity is opened in these tests")
        }
        async fn reload(&self) -> PageResult<()> {
            Ok(())
        }
    }

    impl crate::runners::unit::UnitPage for FakeCourse {
        async fn click_row(&self, _title: &str) -> PageResult<bool> {
            unreachable!("no unit node is opened in these tests")
        }
    }

    /// Unit and checkpoint runners that bump the fake's progress, or return a fixed outcome.
    struct FakeRunners {
        unit: Option<UnitOutcome>,
    }

    impl CourseNodeRunners<FakeCourse> for FakeRunners {
        async fn unit(&mut self, page: &FakeCourse) -> PageResult<UnitOutcome> {
            if let Some(outcome) = &self.unit {
                return Ok(outcome.clone());
            }
            let mut progress = page.progress.borrow_mut();
            progress.unit = progress.unit.map(|c| Count { done: c.done + 1, ..c });
            Ok(UnitOutcome::Complete { total: 6 })
        }
        async fn checkpoint(&mut self, page: &FakeCourse) -> PageResult<CheckpointOutcome> {
            let mut progress = page.progress.borrow_mut();
            progress.checkpoint = progress.checkpoint.map(|c| Count { done: c.done + 1, ..c });
            Ok(CheckpointOutcome::Passed)
        }
    }

    fn quick() -> CourseOptions {
        CourseOptions { read_retry: Duration::from_millis(50) }
    }

    #[tokio::test]
    async fn run_course_opens_nodes_in_ascending_course_order_skipping_completed_ones() {
        // Matches the live roadmap: Unit1, Unit2, Checkpoint1, Unit3, with
        // Unit 1/3 and CP 0/1 already done, so Unit2 is the next node to open.
        let page = FakeCourse::new(
            vec![
                node(NodeKind::Unit, 1, 900.0, "Unit1"),
                node(NodeKind::Unit, 2, 700.0, "Unit2"),
                node(NodeKind::Checkpoint, 1, 500.0, "Checkpoint1"),
                node(NodeKind::Unit, 3, 300.0, "Unit3"),
            ],
            Progress { unit: count(1, 3), checkpoint: count(0, 1) },
        );
        let result = run(&page, &mut FakeRunners { unit: None }, quick()).await.unwrap();
        assert_eq!(*page.opened.borrow(), vec!["Unit2", "Checkpoint1", "Unit3"]);
        assert_eq!(
            result,
            CourseOutcome::Complete { unit: Count { done: 3, total: 3 }, checkpoint: Count { done: 1, total: 1 } }
        );
    }

    #[tokio::test]
    async fn run_course_never_opens_a_final_test_node_even_if_present_in_the_dom() {
        let page = FakeCourse::new(
            vec![node(NodeKind::Unit, 1, 900.0, "Unit1")],
            Progress { unit: count(0, 1), checkpoint: count(0, 0) },
        );
        // The unit run leaves progress frozen, so only the tried-node memory ends the loop.
        let result =
            run(&page, &mut FakeRunners { unit: Some(UnitOutcome::Complete { total: 6 }) }, quick()).await.unwrap();
        assert_eq!(*page.opened.borrow(), vec!["Unit1"]);
        assert_eq!(result.status(), "complete");
    }

    #[test]
    fn read_course_nodes_drops_the_final_test_and_anything_else_off_the_whitelist() {
        let raw = |label: &str, top: f64| RawNode { label: label.to_string(), top: Some(top) };
        let nodes = parse_course_nodes(vec![
            raw("FinalTest1", 50.0),
            raw("Unit2", 700.0),
            raw("Checkpoint1", 500.0),
            raw("FinalTest", 40.0),
            raw("Unit1", 900.0),
            raw("C2.2", 10.0),
        ]);
        let labels: Vec<&str> = nodes.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(labels, vec!["Unit1", "Unit2", "Checkpoint1"]);
        assert_eq!(nodes[2], node(NodeKind::Checkpoint, 1, 500.0, "Checkpoint1"));
    }

    #[test]
    fn read_course_nodes_sorts_by_descending_top_into_the_verified_live_order() {
        let live = [
            ("Unit8", 120.0),
            ("Unit1", 1500.0),
            ("Checkpoint3", 400.0),
            ("Unit3", 1100.0),
            ("Unit2", 1400.0),
            ("Checkpoint1", 1250.0),
            ("Unit5", 800.0),
            ("Unit4", 1000.0),
            ("Checkpoint2", 900.0),
            ("Unit7", 250.0),
            ("Unit6", 600.0),
        ];
        let nodes =
            parse_course_nodes(live.iter().map(|(l, t)| RawNode { label: l.to_string(), top: Some(*t) }).collect());
        let labels: Vec<&str> = nodes.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Unit1",
                "Unit2",
                "Checkpoint1",
                "Unit3",
                "Unit4",
                "Checkpoint2",
                "Unit5",
                "Unit6",
                "Checkpoint3",
                "Unit7",
                "Unit8"
            ]
        );
    }

    #[test]
    fn read_progress_parses_the_header_readout() {
        assert_eq!(
            parse_progress("ENG-C2.2\nUnit 8/8  CP 3/3  FT 1/1\nUnit1"),
            Progress { unit: count(8, 8), checkpoint: count(3, 3) }
        );
        assert_eq!(parse_progress("Unit\u{a0}4 / 8 CP1/3"), Progress { unit: count(4, 8), checkpoint: count(1, 3) });
        assert_eq!(parse_progress("Loading"), Progress::default());
    }

    #[test]
    fn next_node_skips_done_and_tried_nodes() {
        let nodes = vec![
            node(NodeKind::Unit, 1, 900.0, "Unit1"),
            node(NodeKind::Unit, 2, 700.0, "Unit2"),
            node(NodeKind::Checkpoint, 1, 500.0, "Checkpoint1"),
        ];
        let progress = Progress { unit: count(1, 2), checkpoint: count(0, 1) };
        let mut tried = HashSet::new();
        assert_eq!(next_node(&nodes, &progress, &tried).map(|n| n.label.as_str()), Some("Unit2"));
        tried.insert("Unit2".to_string());
        assert_eq!(next_node(&nodes, &progress, &tried).map(|n| n.label.as_str()), Some("Checkpoint1"));
        let all_done = Progress { unit: count(2, 2), checkpoint: count(1, 1) };
        assert_eq!(next_node(&nodes, &all_done, &HashSet::new()), None);
    }

    #[tokio::test]
    async fn run_course_stops_when_a_course_node_fails_to_open_locked() {
        let mut page = FakeCourse::new(
            vec![node(NodeKind::Unit, 5, 300.0, "Unit5")],
            Progress { unit: count(4, 8), checkpoint: count(1, 3) },
        );
        page.opens = false;
        let result = run(&page, &mut FakeRunners { unit: None }, quick()).await.unwrap();
        assert_eq!(result, CourseOutcome::Stuck(CourseStuck::NodeDidNotOpen { label: "Unit5".to_string() }));
    }

    #[tokio::test]
    async fn run_course_stops_when_a_unit_inside_the_course_gets_stuck() {
        let page = FakeCourse::new(
            vec![node(NodeKind::Unit, 1, 900.0, "Unit1")],
            Progress { unit: count(0, 1), checkpoint: count(0, 0) },
        );
        let stuck = UnitOutcome::Stuck(UnitStuck::RowNotClicked { title: "Interactive Grammar Activity".to_string() });
        let result = run(&page, &mut FakeRunners { unit: Some(stuck.clone()) }, quick()).await.unwrap();
        assert_eq!(
            result,
            CourseOutcome::Stuck(CourseStuck::NodeFailed {
                label: "Unit1".to_string(),
                outcome: NodeOutcome::Unit(stuck)
            })
        );
    }

    #[tokio::test]
    async fn run_course_stops_on_a_checkpoint_that_is_not_ported() {
        let page = FakeCourse::new(
            vec![node(NodeKind::Checkpoint, 1, 500.0, "Checkpoint1")],
            Progress { unit: count(2, 2), checkpoint: count(0, 1) },
        );
        let result =
            run(&page, &mut super::super::Live { config: crate::config::build_config(&Default::default()) }, quick())
                .await
                .unwrap();
        assert_eq!(
            result,
            CourseOutcome::Stuck(CourseStuck::NodeFailed {
                label: "Checkpoint1".to_string(),
                outcome: NodeOutcome::Checkpoint(CheckpointOutcome::NotPorted),
            })
        );
    }

    #[tokio::test]
    async fn run_course_does_not_complete_when_roadmap_nodes_or_progress_are_absent() {
        let page = FakeCourse::new(vec![], Progress::default());
        let result = run(&page, &mut FakeRunners { unit: None }, quick()).await.unwrap();
        assert_eq!(result, CourseOutcome::Stuck(CourseStuck::MissingRoadmap));
    }
}
