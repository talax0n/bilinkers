//! The window's screens as one state machine: `step` is the only place a screen changes, and it
//! returns what the host should do instead of doing it.
//!
//! Welcome (until Codex is signed in and a browser is picked) -> Home (one Start button) ->
//! Waiting (the browser opens, the user opens a course page) -> Running -> Stopping ->
//! Finished -> Home.

use std::path::PathBuf;

use rune::machine::Effects;

use crate::detect::PageKind;
use crate::logger::Event;
use crate::session::{BrowserSetup, Connection, RunMode, Sighting};
use crate::setup::{BrowserKind, BrowserPick, CodexStatus, Origin};
use crate::ui::dashboard::{Dashboard, RunStatus};

#[derive(Debug, Clone, PartialEq)]
pub enum Screen {
    Welcome,
    Home,
    Waiting(Waiting),
    Running(Dashboard),
    /// Stop was pressed; the run's last event has not arrived yet.
    Stopping(Dashboard),
    Finished(Dashboard),
}

/// After Start, until a page the bot can work on shows up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Waiting {
    /// The browser launch was asked for and has not answered.
    Opening,
    OpenFailed(String),
    /// Watching the browser; None before the first look lands.
    Looking(Option<Sighting>),
}

/// The Codex step: the last `codex login status`, or what is in flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Codex {
    Checking,
    /// `codex login` is waiting on the sign-in in the system browser.
    SigningIn,
    Known(CodexStatus),
}

/// The browser step: what is installed, what the settings resolve to, and the pick on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserStep {
    pub installed: Vec<BrowserKind>,
    /// What the settings resolve to right now.
    pub setup: BrowserSetup,
    /// Set when the environment or `.env` chooses the browser, which outranks the window.
    pub locked_by: Option<Origin>,
    pub pick: Option<BrowserPick>,
    /// Why the last save or file dialog failed.
    pub error: Option<String>,
}

impl BrowserStep {
    /// Preselects the current setting when it is installed, else the first installed browser.
    pub fn new(installed: Vec<BrowserKind>, setup: BrowserSetup, locked_by: Option<Origin>) -> BrowserStep {
        let current = BrowserKind::parse(&setup.name).filter(|kind| installed.contains(kind));
        let pick = match (&setup.binary, current) {
            (Ok(path), None) if setup.name == "custom" => Some(BrowserPick::File(PathBuf::from(path))),
            _ => current.or(installed.first().copied()).map(BrowserPick::Installed),
        };
        BrowserStep { installed, setup, locked_by, pick, error: None }
    }

    /// Locked, the settings must already work; otherwise a pick is enough, Continue saves it.
    pub fn done(&self) -> bool {
        match self.locked_by {
            Some(_) => self.setup.binary.is_ok(),
            None => self.pick.is_some(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    CodexChecked(CodexStatus),
    /// Retry after Codex was missing or failed.
    CheckCodex,
    SignIn,
    SignInDone(Result<(), String>),
    PickBrowser(BrowserKind),
    ChooseFile,
    FileChosen(Result<Option<PathBuf>, String>),
    BrowserSaved(Result<BrowserStep, String>),
    Continue,
    OpenSetup,
    Probed(Connection),
    Start,
    Opened(Result<(), String>),
    Watched(Sighting),
    Cancel,
    Log(Event),
    Stop,
    RunAgain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    CheckCodex,
    SignIn,
    /// Rereads the installed browsers and the settings.
    CheckBrowsers,
    ChooseFile,
    SaveBrowser(BrowserPick),
    /// Looks at the tab list now (Home).
    Probe,
    OpenBrowser,
    /// Looks at the browser now (Waiting).
    Watch,
    StartRun(RunMode),
    StopRun,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Flow {
    pub codex: Codex,
    pub codex_error: Option<String>,
    pub browser: BrowserStep,
    pub connection: Connection,
    /// The CLI's mode, used once instead of detecting the page.
    pub preset: Option<RunMode>,
    pub screen: Screen,
}

impl Flow {
    /// Opens on Home when Codex is signed in and the browser settings already work.
    pub fn new(codex: CodexStatus, browser: BrowserStep, preset: Option<RunMode>) -> Flow {
        let ready = codex.signed_in() && browser.setup.binary.is_ok();
        Flow {
            codex: Codex::Known(codex),
            codex_error: None,
            browser,
            connection: Connection::Checking,
            preset: preset.filter(|mode| *mode != RunMode::FinalTest),
            screen: if ready { Screen::Home } else { Screen::Welcome },
        }
    }

    pub fn signed_in(&self) -> bool {
        matches!(&self.codex, Codex::Known(status) if status.signed_in())
    }

    pub fn welcome_done(&self) -> bool {
        self.signed_in() && self.browser.done()
    }

    pub fn can_sign_in(&self) -> bool {
        self.screen == Screen::Welcome
            && matches!(&self.codex, Codex::Known(CodexStatus::SignedOut | CodexStatus::Failed(_)))
    }

    /// Start needs a browser to open, or one already open.
    pub fn can_start(&self) -> bool {
        let open = !matches!(self.connection, Connection::NotRunning | Connection::Checking);
        let startable = matches!(self.screen, Screen::Home | Screen::Waiting(Waiting::OpenFailed(_)));
        startable && (self.browser.setup.binary.is_ok() || open)
    }

    /// Home shows the connection, so it probes.
    pub fn probing(&self) -> bool {
        self.screen == Screen::Home
    }

    /// Waiting looks at the browser once it has been opened.
    pub fn watching(&self) -> bool {
        matches!(self.screen, Screen::Waiting(Waiting::Looking(_)))
    }

    pub fn board(&self) -> Option<&Dashboard> {
        match &self.screen {
            Screen::Running(board) | Screen::Stopping(board) | Screen::Finished(board) => Some(board),
            _ => None,
        }
    }

    /// Leaves for Home and asks for a fresh look at the tabs.
    fn home(&mut self) -> Effects<Effect> {
        self.screen = Screen::Home;
        self.connection = Connection::Checking;
        Effects::one(Effect::Probe)
    }

    fn start_run(&mut self, mode: RunMode) -> Effects<Effect> {
        self.preset = None;
        self.screen = Screen::Running(Dashboard::new(mode.label()));
        Effects::one(Effect::StartRun(mode))
    }
}

/// The mode a sighting can start, if any. A preset mode only needs an LMS tab.
fn startable(sighting: &Sighting, preset: Option<RunMode>) -> Option<RunMode> {
    match sighting {
        Sighting::Lms { page, .. } => match (preset, page) {
            (Some(mode), _) => Some(mode),
            (None, Some(PageKind::Mode(mode))) => Some(RunMode::from_detected(*mode)),
            _ => None,
        },
        _ => None,
    }
}

pub fn step(flow: &mut Flow, input: Input) -> Effects<Effect> {
    let welcome = flow.screen == Screen::Welcome;
    match input {
        Input::CodexChecked(status) => flow.codex = Codex::Known(status),
        Input::CheckCodex if welcome && flow.codex != Codex::SigningIn => {
            flow.codex = Codex::Checking;
            return Effects::one(Effect::CheckCodex);
        }
        Input::SignIn if flow.can_sign_in() => {
            flow.codex = Codex::SigningIn;
            flow.codex_error = None;
            return Effects::one(Effect::SignIn);
        }
        Input::SignInDone(result) => {
            flow.codex = Codex::Checking;
            flow.codex_error = result.err();
            return Effects::one(Effect::CheckCodex);
        }
        Input::PickBrowser(kind) if welcome && flow.browser.locked_by.is_none() => {
            flow.browser.pick = Some(BrowserPick::Installed(kind));
            flow.browser.error = None;
        }
        Input::ChooseFile if welcome && flow.browser.locked_by.is_none() => return Effects::one(Effect::ChooseFile),
        Input::FileChosen(result) => match result {
            Ok(Some(path)) => {
                flow.browser.pick = Some(BrowserPick::File(path));
                flow.browser.error = None;
            }
            Ok(None) => {}
            Err(e) => flow.browser.error = Some(e),
        },
        Input::BrowserSaved(Ok(saved)) => {
            flow.browser = saved;
            if welcome && flow.welcome_done() {
                return flow.home();
            }
        }
        Input::BrowserSaved(Err(e)) => flow.browser.error = Some(e),
        Input::Continue if welcome && flow.welcome_done() => {
            return match (&flow.browser.locked_by, &flow.browser.pick) {
                (None, Some(pick)) => Effects::one(Effect::SaveBrowser(pick.clone())),
                _ => flow.home(),
            };
        }
        Input::OpenSetup if flow.screen == Screen::Home => {
            flow.screen = Screen::Welcome;
            flow.codex = Codex::Checking;
            return Effects::two(Effect::CheckCodex, Effect::CheckBrowsers);
        }
        Input::Probed(connection) => flow.connection = connection,
        Input::Start if flow.can_start() => {
            flow.screen = Screen::Waiting(Waiting::Opening);
            return Effects::one(Effect::OpenBrowser);
        }
        Input::Opened(result) => {
            if flow.screen == Screen::Waiting(Waiting::Opening) {
                return match result {
                    Ok(()) => {
                        flow.screen = Screen::Waiting(Waiting::Looking(None));
                        Effects::one(Effect::Watch)
                    }
                    Err(e) => {
                        flow.screen = Screen::Waiting(Waiting::OpenFailed(e));
                        Effects::none()
                    }
                };
            }
        }
        Input::Watched(sighting) => {
            if flow.watching() {
                if let Some(mode) = startable(&sighting, flow.preset) {
                    return flow.start_run(mode);
                }
                flow.screen = Screen::Waiting(Waiting::Looking(Some(sighting)));
            }
        }
        Input::Cancel if matches!(flow.screen, Screen::Waiting(_)) => return flow.home(),
        Input::Log(event) => match &mut flow.screen {
            Screen::Running(board) | Screen::Stopping(board) => {
                board.apply(&event);
                if board.status != RunStatus::Running {
                    flow.screen = Screen::Finished(board.clone());
                }
            }
            Screen::Finished(board) => board.apply(&event),
            _ => {}
        },
        Input::Stop => {
            if let Screen::Running(board) = &flow.screen {
                flow.screen = Screen::Stopping(board.clone());
                return Effects::one(Effect::StopRun);
            }
        }
        Input::RunAgain if matches!(flow.screen, Screen::Finished(_)) => return flow.home(),
        Input::CheckCodex
        | Input::SignIn
        | Input::PickBrowser(_)
        | Input::ChooseFile
        | Input::Continue
        | Input::OpenSetup
        | Input::Start
        | Input::Cancel
        | Input::RunAgain => {}
    }
    Effects::none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::PageMode;
    use crate::logger::Level;
    use serde_json::{json, Value};
    use std::time::UNIX_EPOCH;

    fn brave() -> BrowserSetup {
        BrowserSetup { name: "brave".into(), binary: Ok("/bin/brave".into()) }
    }

    fn signed_in() -> CodexStatus {
        CodexStatus::SignedIn("Logged in using ChatGPT".into())
    }

    fn home() -> Flow {
        let flow = Flow::new(signed_in(), BrowserStep::new(vec![BrowserKind::Brave], brave(), None), None);
        assert_eq!(flow.screen, Screen::Home);
        flow
    }

    fn lms(page: Option<PageKind>) -> Sighting {
        Sighting::Lms { title: "Unit 5".into(), url: "https://lms.binus.ac.id/course/5".into(), page }
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

    /// Home, Start, the browser opened: watching.
    fn waiting() -> Flow {
        let mut f = home();
        assert_eq!(effects(&mut f, Input::Start), vec![Effect::OpenBrowser]);
        assert_eq!(f.screen, Screen::Waiting(Waiting::Opening));
        assert!(!f.watching(), "nothing to watch until the launch answers");
        assert_eq!(effects(&mut f, Input::Opened(Ok(()))), vec![Effect::Watch]);
        assert!(f.watching());
        f
    }

    #[test]
    fn the_welcome_screen_shows_until_codex_is_signed_in_and_the_browser_works() {
        let missing_chrome = BrowserSetup { name: "chrome".into(), binary: Err("not installed".into()) };
        let brave_only = || BrowserStep::new(vec![BrowserKind::Brave], missing_chrome.clone(), None);
        assert_eq!(Flow::new(CodexStatus::SignedOut, brave_only(), None).screen, Screen::Welcome);
        assert_eq!(Flow::new(signed_in(), brave_only(), None).screen, Screen::Welcome, "chrome is not installed");
        let step = BrowserStep::new(vec![BrowserKind::Brave], brave(), None);
        assert_eq!(Flow::new(CodexStatus::Missing, step.clone(), None).screen, Screen::Welcome);
        assert_eq!(Flow::new(signed_in(), step, None).screen, Screen::Home);
    }

    #[test]
    fn the_browser_step_preselects_the_setting_else_the_first_installed() {
        let all = BrowserKind::ALL.to_vec();
        assert_eq!(BrowserStep::new(all.clone(), brave(), None).pick, Some(BrowserPick::Installed(BrowserKind::Brave)));
        let chrome = BrowserSetup { name: "chrome".into(), binary: Err("missing".into()) };
        let step = BrowserStep::new(vec![BrowserKind::Brave, BrowserKind::Edge], chrome.clone(), None);
        assert_eq!(step.pick, Some(BrowserPick::Installed(BrowserKind::Brave)));
        assert!(step.done(), "a pick is enough; Continue saves it");
        let none = BrowserStep::new(vec![], chrome.clone(), None);
        assert_eq!(none.pick, None);
        assert!(!none.done(), "nothing installed and nothing chosen");
        let locked = BrowserStep::new(all, chrome, Some(Origin::DotEnv));
        assert!(!locked.done(), "a .env setting that does not work can't be fixed here");
        let custom = BrowserSetup { name: "custom".into(), binary: Ok("/opt/chromium".into()) };
        assert_eq!(BrowserStep::new(vec![], custom, None).pick, Some(BrowserPick::File("/opt/chromium".into())));
    }

    #[test]
    fn signing_in_runs_codex_login_then_checks_again() {
        let step = BrowserStep::new(vec![BrowserKind::Brave], brave(), None);
        let mut f = Flow::new(CodexStatus::SignedOut, step, None);
        assert!(!f.welcome_done());
        assert_eq!(effects(&mut f, Input::Continue), vec![], "Continue waits for the sign-in");
        assert_eq!(effects(&mut f, Input::SignIn), vec![Effect::SignIn]);
        assert_eq!(f.codex, Codex::SigningIn);
        assert_eq!(effects(&mut f, Input::SignIn), vec![], "one sign-in at a time");
        assert_eq!(effects(&mut f, Input::CheckCodex), vec![], "no recheck while it waits");
        assert_eq!(effects(&mut f, Input::SignInDone(Err("closed".into()))), vec![Effect::CheckCodex]);
        assert_eq!((&f.codex, f.codex_error.as_deref()), (&Codex::Checking, Some("closed")));
        effects(&mut f, Input::CodexChecked(signed_in()));
        assert!(f.welcome_done());
        assert_eq!(
            effects(&mut f, Input::Continue),
            vec![Effect::SaveBrowser(BrowserPick::Installed(BrowserKind::Brave))]
        );
        assert_eq!(f.screen, Screen::Welcome, "Home waits for the save");
        let saved = BrowserStep::new(vec![BrowserKind::Brave], brave(), None);
        assert_eq!(effects(&mut f, Input::BrowserSaved(Ok(saved))), vec![Effect::Probe]);
        assert_eq!(f.screen, Screen::Home);
    }

    #[test]
    fn a_missing_codex_offers_retry_not_sign_in() {
        let step = BrowserStep::new(vec![BrowserKind::Brave], brave(), None);
        let mut f = Flow::new(CodexStatus::Missing, step, None);
        assert!(!f.can_sign_in());
        assert_eq!(effects(&mut f, Input::SignIn), vec![]);
        assert_eq!(effects(&mut f, Input::CheckCodex), vec![Effect::CheckCodex]);
        assert_eq!(f.codex, Codex::Checking);
    }

    #[test]
    fn a_failed_save_stays_on_welcome_and_a_locked_browser_is_not_saved() {
        let step = BrowserStep::new(BrowserKind::ALL.to_vec(), brave(), None);
        let mut f = Flow::new(CodexStatus::SignedOut, step, None);
        effects(&mut f, Input::CodexChecked(signed_in()));
        effects(&mut f, Input::PickBrowser(BrowserKind::Edge));
        assert_eq!(
            effects(&mut f, Input::Continue),
            vec![Effect::SaveBrowser(BrowserPick::Installed(BrowserKind::Edge))]
        );
        effects(&mut f, Input::BrowserSaved(Err("read-only disk".into())));
        assert_eq!((&f.screen, f.browser.error.as_deref()), (&Screen::Welcome, Some("read-only disk")));

        let locked = BrowserStep::new(BrowserKind::ALL.to_vec(), brave(), Some(Origin::DotEnv));
        let mut f = Flow::new(CodexStatus::SignedOut, locked, None);
        effects(&mut f, Input::PickBrowser(BrowserKind::Edge));
        assert_eq!(f.browser.pick, Some(BrowserPick::Installed(BrowserKind::Brave)), "the .env choice stands");
        assert_eq!(effects(&mut f, Input::ChooseFile), vec![]);
        effects(&mut f, Input::CodexChecked(signed_in()));
        assert_eq!(effects(&mut f, Input::Continue), vec![Effect::Probe]);
        assert_eq!(f.screen, Screen::Home);
    }

    #[test]
    fn a_chosen_file_becomes_the_pick() {
        let none = BrowserSetup { name: "chrome".into(), binary: Err("missing".into()) };
        let mut f = Flow::new(signed_in(), BrowserStep::new(vec![], none, None), None);
        assert_eq!(effects(&mut f, Input::ChooseFile), vec![Effect::ChooseFile]);
        effects(&mut f, Input::FileChosen(Ok(None)));
        assert!(!f.welcome_done(), "a cancelled dialog picks nothing");
        effects(&mut f, Input::FileChosen(Err("no dialog".into())));
        assert_eq!(f.browser.error.as_deref(), Some("no dialog"));
        effects(&mut f, Input::FileChosen(Ok(Some("/opt/chromium".into()))));
        assert!(f.welcome_done());
        assert_eq!(
            effects(&mut f, Input::Continue),
            vec![Effect::SaveBrowser(BrowserPick::File("/opt/chromium".into()))]
        );
    }

    #[test]
    fn setup_reopens_the_welcome_screen_from_home() {
        let mut f = home();
        assert_eq!(effects(&mut f, Input::OpenSetup), vec![Effect::CheckCodex, Effect::CheckBrowsers]);
        assert_eq!(f.screen, Screen::Welcome);
        effects(&mut f, Input::CodexChecked(signed_in()));
        assert_eq!(effects(&mut f, Input::Continue).len(), 1);
    }

    #[test]
    fn start_opens_the_browser_then_waits_for_a_page_and_starts_by_itself() {
        let mut f = waiting();
        for sighting in [Sighting::NotRunning, Sighting::NoLmsTab, lms(None), lms(Some(PageKind::Other))] {
            assert_eq!(effects(&mut f, Input::Watched(sighting.clone())), vec![]);
            assert_eq!(f.screen, Screen::Waiting(Waiting::Looking(Some(sighting))));
        }
        assert_eq!(
            effects(&mut f, Input::Watched(lms(Some(PageKind::Mode(PageMode::Unit))))),
            vec![Effect::StartRun(RunMode::Unit)]
        );
        assert!(matches!(&f.screen, Screen::Running(b) if b.mode == "Unit"));
        assert!(!f.watching() && !f.probing());
    }

    #[test]
    fn the_island_map_and_a_final_test_keep_waiting() {
        let mut f = waiting();
        assert_eq!(effects(&mut f, Input::Watched(lms(Some(PageKind::IslandMap)))), vec![]);
        assert_eq!(f.screen, Screen::Waiting(Waiting::Looking(Some(lms(Some(PageKind::IslandMap))))));
        assert_eq!(effects(&mut f, Input::Watched(lms(Some(PageKind::FinalTest)))), vec![], "never auto-run");
        assert!(f.watching());
        assert_eq!(
            effects(&mut f, Input::Watched(lms(Some(PageKind::Mode(PageMode::Course))))),
            vec![Effect::StartRun(RunMode::Course)]
        );
    }

    #[test]
    fn a_cli_mode_skips_detection_once_and_final_test_is_never_preset() {
        let step = || BrowserStep::new(vec![BrowserKind::Brave], brave(), None);
        assert_eq!(Flow::new(signed_in(), step(), Some(RunMode::FinalTest)).preset, None);
        let mut f = Flow::new(signed_in(), step(), Some(RunMode::Reading));
        effects(&mut f, Input::Start);
        effects(&mut f, Input::Opened(Ok(())));
        assert_eq!(effects(&mut f, Input::Watched(Sighting::NoLmsTab)), vec![], "it still needs an LMS tab");
        assert_eq!(effects(&mut f, Input::Watched(lms(None))), vec![Effect::StartRun(RunMode::Reading)]);
        assert_eq!(f.preset, None, "the next run detects");
    }

    #[test]
    fn a_failed_launch_can_be_retried_or_cancelled_and_late_looks_are_ignored() {
        let mut f = home();
        effects(&mut f, Input::Start);
        effects(&mut f, Input::Opened(Err("no browser".into())));
        assert_eq!(f.screen, Screen::Waiting(Waiting::OpenFailed("no browser".into())));
        assert!(!f.watching());
        assert_eq!(effects(&mut f, Input::Start), vec![Effect::OpenBrowser], "Try again");
        assert_eq!(effects(&mut f, Input::Cancel), vec![Effect::Probe]);
        assert_eq!((&f.screen, &f.connection), (&Screen::Home, &Connection::Checking));
        assert_eq!(
            effects(&mut f, Input::Watched(lms(Some(PageKind::Mode(PageMode::Unit))))),
            vec![],
            "a look after Cancel"
        );
        assert_eq!(effects(&mut f, Input::Opened(Ok(()))), vec![], "a launch after Cancel");
        assert_eq!(f.screen, Screen::Home);
    }

    #[test]
    fn start_needs_a_working_browser_or_an_open_one() {
        let broken = BrowserSetup { name: "chrome".into(), binary: Err("missing".into()) };
        let mut f = home();
        f.browser.setup = broken;
        assert!(!f.can_start());
        assert_eq!(effects(&mut f, Input::Start), vec![]);
        f.connection = Connection::NoLmsTab;
        assert!(f.can_start(), "a browser that is already open needs no launch");
    }

    #[test]
    fn a_run_finishes_and_run_again_goes_home() {
        let mut f = waiting();
        effects(&mut f, Input::Watched(lms(Some(PageKind::Mode(PageMode::Exercise)))));
        effects(&mut f, log("Question processed", vec![("questionNum", json!(1)), ("status", json!("correct"))]));
        assert!(matches!(&f.screen, Screen::Running(b) if b.correct == 1));
        assert_eq!(effects(&mut f, Input::RunAgain), vec![], "not while running");
        effects(&mut f, log("Run finished", vec![("status", json!("complete"))]));
        assert!(matches!(&f.screen, Screen::Finished(b) if b.status == RunStatus::Finished("complete".into())));
        assert_eq!(effects(&mut f, Input::RunAgain), vec![Effect::Probe]);
        assert_eq!(f.screen, Screen::Home);
    }

    #[test]
    fn stop_waits_for_the_runs_last_event() {
        let mut f = waiting();
        effects(&mut f, Input::Watched(lms(Some(PageKind::Mode(PageMode::Unit)))));
        assert_eq!(effects(&mut f, Input::Stop), vec![Effect::StopRun]);
        assert!(matches!(f.screen, Screen::Stopping(_)));
        assert_eq!(effects(&mut f, Input::Stop), vec![], "a second Stop asks nothing");
        assert_eq!(effects(&mut f, Input::RunAgain), vec![], "Run again waits for the run to end");
        effects(&mut f, log("Run stopped", vec![]));
        assert!(matches!(&f.screen, Screen::Finished(b) if b.status == RunStatus::Stopped));
    }

    #[test]
    fn a_failed_run_finishes_too() {
        let mut f = waiting();
        effects(&mut f, Input::Watched(lms(Some(PageKind::Mode(PageMode::Reading)))));
        effects(&mut f, log("Run failed", vec![("error", json!("No LMS tab"))]));
        assert!(matches!(&f.screen, Screen::Finished(b) if b.status == RunStatus::Failed));
    }
}
