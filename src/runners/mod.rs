//! Ports of `src/runners/`.

pub mod checkpoint;
pub mod course;
pub mod exercise;
pub mod final_test_runner;
pub mod iframe;
pub mod island;
pub mod native;
pub mod reading;
pub mod unit;

use serde_json::{json, Value};

use crate::browser::{FramedPage, PageResult};
use crate::config::Config;
use unit::{ActivityKind, UnitOutcome, UnitPage};

/// Log fields for an outcome, the JS result object spread into a log line.
pub trait Report {
    fn status(&self) -> &'static str;

    fn fields(&self) -> Vec<(&'static str, Value)> {
        vec![("status", json!(self.status()))]
    }
}

/// What an exercise, iframe or reading runner returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityOutcome {
    Complete,
    Unhandled,
    Incorrect,
    /// The reading runner's pass cap or a locked wrong item.
    Stuck,
    /// The reading runner left Submit refused.
    Incomplete,
}

impl ActivityOutcome {
    /// unit.js stops only on these; a stuck or incomplete reading row moves on to the next row.
    pub fn stops_unit(self) -> bool {
        matches!(self, ActivityOutcome::Unhandled | ActivityOutcome::Incorrect)
    }
}

impl Report for ActivityOutcome {
    fn status(&self) -> &'static str {
        match self {
            ActivityOutcome::Complete => "complete",
            ActivityOutcome::Unhandled => "unhandled",
            ActivityOutcome::Incorrect => "incorrect",
            ActivityOutcome::Stuck => "stuck",
            ActivityOutcome::Incomplete => "incomplete",
        }
    }
}

/// An activity runner's result object: the outcome plus the counters the JS spreads into its log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivityReport {
    pub outcome: ActivityOutcome,
    pub question_num: Option<u32>,
    pub slide_num: Option<u32>,
    pub questions: Option<usize>,
    pub score: Option<u64>,
}

impl ActivityReport {
    pub fn new(outcome: ActivityOutcome) -> Self {
        ActivityReport { outcome, question_num: None, slide_num: None, questions: None, score: None }
    }

    pub fn question_num(self, n: u32) -> Self {
        ActivityReport { question_num: Some(n), ..self }
    }

    pub fn slide_num(self, n: u32) -> Self {
        ActivityReport { slide_num: Some(n), ..self }
    }

    pub fn questions(self, n: usize) -> Self {
        ActivityReport { questions: Some(n), ..self }
    }

    pub fn score(self, score: Option<u64>) -> Self {
        ActivityReport { score, ..self }
    }
}

impl Report for ActivityReport {
    fn status(&self) -> &'static str {
        self.outcome.status()
    }

    fn fields(&self) -> Vec<(&'static str, Value)> {
        let mut fields = vec![("status", json!(self.status()))];
        if let Some(n) = self.question_num {
            fields.push(("questionNum", json!(n)));
        }
        if let Some(n) = self.slide_num {
            fields.push(("slideNum", json!(n)));
        }
        if let Some(n) = self.questions {
            fields.push(("questions", json!(n)));
        }
        if let Some(score) = self.score {
            fields.push(("score", json!(score)));
        }
        fields
    }
}

/// What the checkpoint runner returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointOutcome {
    Passed,
    Unhandled,
    NoGate,
    SubmitFailed,
}

impl CheckpointOutcome {
    /// course.js stops on these; a submit failure moves on to the next node.
    pub fn stops_course(self) -> bool {
        matches!(self, CheckpointOutcome::Unhandled | CheckpointOutcome::NoGate)
    }
}

impl Report for CheckpointOutcome {
    fn status(&self) -> &'static str {
        match self {
            CheckpointOutcome::Passed => "passed",
            CheckpointOutcome::Unhandled => "unhandled",
            CheckpointOutcome::NoGate => "no-gate",
            CheckpointOutcome::SubmitFailed => "submit-failed",
        }
    }
}

/// The checkpoint runner's result object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointReport {
    pub outcome: CheckpointOutcome,
    pub attempt: u32,
    pub score: Option<u64>,
    pub question_num: Option<u32>,
}

impl CheckpointReport {
    pub fn new(outcome: CheckpointOutcome, attempt: u32) -> Self {
        CheckpointReport { outcome, attempt, score: None, question_num: None }
    }

    pub fn score(self, score: Option<u64>) -> Self {
        CheckpointReport { score, ..self }
    }

    pub fn question_num(self, n: u32) -> Self {
        CheckpointReport { question_num: Some(n), ..self }
    }

    pub fn stops_course(&self) -> bool {
        self.outcome.stops_course()
    }
}

impl Report for CheckpointReport {
    fn status(&self) -> &'static str {
        self.outcome.status()
    }

    fn fields(&self) -> Vec<(&'static str, Value)> {
        let mut fields = vec![("status", json!(self.status())), ("attempt", json!(self.attempt))];
        if let Some(n) = self.question_num {
            fields.push(("questionNum", json!(n)));
        }
        if let Some(score) = self.score {
            fields.push(("score", json!(score)));
        }
        fields
    }
}

/// The runners a unit hands an opened activity to.
#[allow(async_fn_in_trait)]
pub trait ActivityRunners<P> {
    async fn run(&mut self, kind: ActivityKind, page: &P) -> PageResult<ActivityReport>;
}

/// The runners a course hands an opened node to.
#[allow(async_fn_in_trait)]
pub trait CourseNodeRunners<P> {
    async fn unit(&mut self, page: &P) -> PageResult<UnitOutcome>;
    async fn checkpoint(&mut self, page: &P) -> PageResult<CheckpointReport>;
}

/// The live runners.
#[derive(Debug, Clone)]
pub struct Live {
    pub config: Config,
}

impl<P: FramedPage> ActivityRunners<P> for Live {
    async fn run(&mut self, kind: ActivityKind, page: &P) -> PageResult<ActivityReport> {
        match kind {
            ActivityKind::Exercise => exercise::run(page, &self.config).await,
            ActivityKind::Iframe => iframe::run(page, &self.config).await,
            ActivityKind::Reading => reading::run(page).await,
        }
    }
}

impl<P: UnitPage + checkpoint::CheckpointPage> CourseNodeRunners<P> for Live {
    async fn unit(&mut self, page: &P) -> PageResult<UnitOutcome> {
        unit::run(page, self).await
    }

    async fn checkpoint(&mut self, page: &P) -> PageResult<CheckpointReport> {
        checkpoint::run(page, &mut checkpoint::LiveAttempt).await
    }
}
