//! Ports of `src/runners/`. Course and unit are ported; the runners they hand off to
//! (exercise, iframe, reading, checkpoint) answer `NotPorted` until a later round.

pub mod course;
pub mod unit;

use serde_json::{json, Value};

use crate::browser::{Driver, PageResult};
use crate::logger;
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
    NotPorted,
}

impl ActivityOutcome {
    /// unit.js stops only on these; a stuck or incomplete reading row moves on to the next row.
    pub fn stops_unit(self) -> bool {
        matches!(self, ActivityOutcome::Unhandled | ActivityOutcome::Incorrect | ActivityOutcome::NotPorted)
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
            ActivityOutcome::NotPorted => "not-ported",
        }
    }
}

/// What the checkpoint runner returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointOutcome {
    Passed,
    Unhandled,
    NoGate,
    SubmitFailed,
    NotPorted,
}

impl CheckpointOutcome {
    /// course.js stops on these; a submit failure moves on to the next node.
    pub fn stops_course(self) -> bool {
        matches!(self, CheckpointOutcome::Unhandled | CheckpointOutcome::NoGate | CheckpointOutcome::NotPorted)
    }
}

impl Report for CheckpointOutcome {
    fn status(&self) -> &'static str {
        match self {
            CheckpointOutcome::Passed => "passed",
            CheckpointOutcome::Unhandled => "unhandled",
            CheckpointOutcome::NoGate => "no-gate",
            CheckpointOutcome::SubmitFailed => "submit-failed",
            CheckpointOutcome::NotPorted => "not-ported",
        }
    }
}

/// The runners a unit hands an opened activity to.
#[allow(async_fn_in_trait)]
pub trait ActivityRunners<P> {
    async fn run(&mut self, kind: ActivityKind, page: &P) -> PageResult<ActivityOutcome>;
}

/// The runners a course hands an opened node to.
#[allow(async_fn_in_trait)]
pub trait CourseNodeRunners<P> {
    async fn unit(&mut self, page: &P) -> PageResult<UnitOutcome>;
    async fn checkpoint(&mut self, page: &P) -> PageResult<CheckpointOutcome>;
}

/// The runners as far as the Rust port goes today.
#[derive(Debug, Clone, Copy, Default)]
pub struct Ported;

impl<P: Driver> ActivityRunners<P> for Ported {
    async fn run(&mut self, kind: ActivityKind, _page: &P) -> PageResult<ActivityOutcome> {
        logger::warn("Activity runner is not ported to Rust yet", [("mode", json!(kind.name()))]);
        Ok(ActivityOutcome::NotPorted)
    }
}

impl<P: UnitPage> CourseNodeRunners<P> for Ported {
    async fn unit(&mut self, page: &P) -> PageResult<UnitOutcome> {
        unit::run(page, &mut Ported).await
    }

    async fn checkpoint(&mut self, _page: &P) -> PageResult<CheckpointOutcome> {
        logger::warn("Checkpoint runner is not ported to Rust yet", []);
        Ok(CheckpointOutcome::NotPorted)
    }
}
