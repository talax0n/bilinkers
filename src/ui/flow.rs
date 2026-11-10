//! The window's screens as one state machine: `step` is the only place a screen changes, and it
//! returns what the host should do instead of doing it.

use rune::machine::Effects;

use crate::logger::Event;
use crate::session::{BrowserSetup, Connection, RunMode};
use crate::ui::dashboard::{Dashboard, RunStatus};

#[derive(Debug, Clone, PartialEq)]
pub enum Screen {
    /// Picking a mode and waiting for an LMS tab.
    Start,
    Running(Dashboard),
    /// Stop was pressed; the run's last event has not arrived yet.
    Stopping(Dashboard),
    Finished(Dashboard),
}

/// The Open browser button's request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opening {
    Idle,
    Busy,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// A mode in the picker, None for Auto-detect.
    Pick(Option<RunMode>),
    Probed(Connection),
    OpenBrowser,
    Opened(Result<(), String>),
    Start,
    Log(Event),
    Stop,
    NewRun,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Probe,
    OpenBrowser,
    StartRun(Option<RunMode>),
    StopRun,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Flow {
    pub choice: Option<RunMode>,
    pub browser: BrowserSetup,
    /// FINAL_TEST_AUTO_ANSWER=1 is set.
    pub auto_answer: bool,
    pub connection: Connection,
    pub opening: Opening,
    pub screen: Screen,
}

impl Flow {
    pub fn new(choice: Option<RunMode>, browser: BrowserSetup, auto_answer: bool) -> Flow {
        Flow {
            choice,
            browser,
            auto_answer,
            connection: Connection::Checking,
            opening: Opening::Idle,
            screen: Screen::Start,
        }
    }

    /// Start needs the start screen and an LMS tab on the debug port.
    pub fn can_start(&self) -> bool {
        self.screen == Screen::Start && self.connection.has_lms_tab()
    }

    pub fn can_open_browser(&self) -> bool {
        self.screen == Screen::Start && self.browser.binary.is_ok() && self.opening != Opening::Busy
    }

    /// Only the start screen shows the connection, so only it probes.
    pub fn probing(&self) -> bool {
        self.screen == Screen::Start
    }

    pub fn board(&self) -> Option<&Dashboard> {
        match &self.screen {
            Screen::Start => None,
            Screen::Running(board) | Screen::Stopping(board) | Screen::Finished(board) => Some(board),
        }
    }
}

/// What the dashboard header names before a mode is known.
pub const AUTO_LABEL: &str = "auto-detect";

pub fn step(flow: &mut Flow, input: Input) -> Effects<Effect> {
    match input {
        Input::Pick(choice) if flow.screen == Screen::Start => flow.choice = choice,
        Input::Probed(connection) => flow.connection = connection,
        Input::OpenBrowser if flow.can_open_browser() => {
            flow.opening = Opening::Busy;
            return Effects::one(Effect::OpenBrowser);
        }
        Input::Opened(result) => {
            flow.opening = result.err().map_or(Opening::Idle, Opening::Failed);
            return Effects::one(Effect::Probe);
        }
        Input::Start if flow.can_start() => {
            flow.screen = Screen::Running(Dashboard::new(flow.choice.map_or(AUTO_LABEL, RunMode::name)));
            return Effects::one(Effect::StartRun(flow.choice));
        }
        Input::Log(event) => match &mut flow.screen {
            Screen::Running(board) | Screen::Stopping(board) => {
                board.apply(&event);
                if board.status != RunStatus::Running {
                    flow.screen = Screen::Finished(board.clone());
                }
            }
            Screen::Finished(board) => board.apply(&event),
            Screen::Start => {}
        },
        Input::Stop => {
            if let Screen::Running(board) = &flow.screen {
                flow.screen = Screen::Stopping(board.clone());
                return Effects::one(Effect::StopRun);
            }
        }
        Input::NewRun if matches!(flow.screen, Screen::Finished(_)) => {
            flow.screen = Screen::Start;
            flow.connection = Connection::Checking;
            return Effects::one(Effect::Probe);
        }
        Input::Pick(_) | Input::OpenBrowser | Input::Start | Input::NewRun => {}
    }
    Effects::none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logger::Level;
    use serde_json::{json, Value};
    use std::time::UNIX_EPOCH;

    fn flow() -> Flow {
        let browser = BrowserSetup { name: "chrome".into(), binary: Ok("/bin/chrome".into()) };
        Flow::new(None, browser, false)
    }

    fn lms() -> Connection {
        Connection::Lms { title: "Unit 5".into(), url: "https://lms.binus.ac.id/course/5".into() }
    }

    fn log(message: &str, fields: Vec<(&str, Value)>) -> Input {
        Input::Log(Event {
            time: UNIX_EPOCH,
            level: Level::Info,
            message: message.into(),
            fields: fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        })
    }

    fn effects(flow: &mut Flow, input: Input) -> Vec<Effect> {
        step(flow, input).into_iter().collect()
    }

    #[test]
    fn start_is_enabled_only_on_the_start_screen_with_an_lms_tab() {
        let mut f = flow();
        for connection in [Connection::Checking, Connection::NotRunning, Connection::NoLmsTab] {
            f.connection = connection;
            assert!(!f.can_start(), "{:?}", f.connection);
            assert_eq!(effects(&mut f, Input::Start), vec![], "a disabled Start does nothing");
            assert_eq!(f.screen, Screen::Start);
        }
        f.connection = lms();
        assert!(f.can_start());
        f.screen = Screen::Running(Dashboard::new("course"));
        assert!(!f.can_start(), "never while a run is on screen");
    }

    #[test]
    fn a_run_goes_from_start_to_running_to_finished_and_back() {
        let mut f = flow();
        assert_eq!(effects(&mut f, Input::Pick(Some(RunMode::Course))), vec![]);
        assert_eq!(effects(&mut f, Input::Probed(lms())), vec![]);
        assert_eq!(effects(&mut f, Input::Start), vec![Effect::StartRun(Some(RunMode::Course))]);
        assert!(matches!(&f.screen, Screen::Running(b) if b.mode == "course"));
        assert!(!f.probing());

        effects(&mut f, log("Question processed", vec![("questionNum", json!(1)), ("status", json!("correct"))]));
        assert!(matches!(&f.screen, Screen::Running(b) if b.correct == 1));
        effects(&mut f, log("Run finished", vec![("status", json!("complete"))]));
        assert!(matches!(&f.screen, Screen::Finished(b) if b.status == RunStatus::Finished("complete".into())));

        assert_eq!(effects(&mut f, Input::Pick(Some(RunMode::Unit))), vec![], "the picker is not on screen");
        assert_eq!(f.choice, Some(RunMode::Course));
        assert_eq!(effects(&mut f, Input::NewRun), vec![Effect::Probe]);
        assert_eq!((&f.screen, &f.connection), (&Screen::Start, &Connection::Checking));
        assert_eq!(f.choice, Some(RunMode::Course), "the last choice stays picked");
    }

    #[test]
    fn auto_detect_names_the_detected_mode_once_it_is_known() {
        let mut f = flow();
        f.connection = lms();
        assert_eq!(effects(&mut f, Input::Start), vec![Effect::StartRun(None)]);
        assert_eq!(f.board().map(|b| b.mode.as_str()), Some(AUTO_LABEL));
        effects(&mut f, log("Detected", vec![("mode", json!("unit"))]));
        assert_eq!(f.board().map(|b| b.mode.as_str()), Some("unit"));
    }

    #[test]
    fn stop_waits_for_the_runs_last_event() {
        let mut f = flow();
        f.connection = lms();
        effects(&mut f, Input::Start);
        assert_eq!(effects(&mut f, Input::Stop), vec![Effect::StopRun]);
        assert!(matches!(f.screen, Screen::Stopping(_)));
        assert_eq!(effects(&mut f, Input::Stop), vec![], "a second Stop asks nothing");
        assert_eq!(effects(&mut f, Input::NewRun), vec![], "New run waits for the run to end");
        effects(&mut f, log("Run stopped", vec![]));
        assert!(matches!(&f.screen, Screen::Finished(b) if b.status == RunStatus::Stopped));
    }

    #[test]
    fn a_failed_run_finishes_too() {
        let mut f = flow();
        f.connection = lms();
        effects(&mut f, Input::Start);
        effects(&mut f, log("Run failed", vec![("error", json!("No LMS tab"))]));
        assert!(matches!(&f.screen, Screen::Finished(b) if b.status == RunStatus::Failed));
    }

    #[test]
    fn open_browser_is_one_request_at_a_time_and_needs_the_binary() {
        let mut f = flow();
        assert_eq!(effects(&mut f, Input::OpenBrowser), vec![Effect::OpenBrowser]);
        assert_eq!(f.opening, Opening::Busy);
        assert_eq!(effects(&mut f, Input::OpenBrowser), vec![]);
        assert_eq!(effects(&mut f, Input::Opened(Err("no".into()))), vec![Effect::Probe]);
        assert_eq!(f.opening, Opening::Failed("no".into()));
        assert_eq!(effects(&mut f, Input::OpenBrowser), vec![Effect::OpenBrowser], "a failure can be retried");
        effects(&mut f, Input::Opened(Ok(())));
        assert_eq!(f.opening, Opening::Idle);

        f.browser.binary = Err("not installed".into());
        assert!(!f.can_open_browser());
        assert_eq!(effects(&mut f, Input::OpenBrowser), vec![]);
    }
}
