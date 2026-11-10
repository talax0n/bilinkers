//! The desktop window: a welcome screen that signs in to ChatGPT and picks the browser, Home
//! with one Start button, a wait for a course page, then the run's dashboard. Logger events
//! reach it through a channel sink, so the bot never waits on it, and Codex, the browser and the
//! page are checked on workers, so the window never waits on them.

pub mod dashboard;
pub mod flow;
mod home;
mod style;
mod welcome;

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use rune::{App, AppHandle, AppSpec, Chrome, Cx, El, FileDialogOptions, Latest, Tx, Waker};

use crate::logger::{self, Event, Level, Sink};
use crate::session::{BrowserSetup, Connection, RunMode, Sighting};
use crate::setup::{browser_executable, BrowserKind, BrowserPick, CodexStatus, FoundBrowser};
pub use dashboard::{all_courses_events, demo_events, Dashboard, DemoEvent, Phase, RunStatus, MAX_LOG_LINES};
pub use flow::BrowserStep;
use flow::{step, Effect, Flow, Input, Screen};
use style::{button, palette, Kind};

/// What the window asks of Codex, the browser and the bot. The worker calls may block; the
/// UI-thread calls must return at once.
pub trait Host: Send + Sync {
    /// Worker: `codex login status`.
    fn codex_status(&self) -> CodexStatus;
    /// Worker: `codex login`, until the sign-in in the system browser ends.
    fn sign_in(&self) -> Result<(), String>;
    /// UI thread: the installed browsers and what the settings resolve to.
    fn browsers(&self) -> BrowserStep;
    /// UI thread: saves the pick to the settings file and rereads the settings.
    fn save_browser(&self, pick: &BrowserPick) -> Result<BrowserStep, String>;
    /// Worker: a read-only look at the browser's tab list.
    fn probe(&self) -> Connection;
    /// Worker: opens the browser at the LMS unless it is already open.
    fn open_browser(&self) -> Result<(), String>;
    /// Worker: the tab list and, when `classify`, what the LMS tab shows. Read-only.
    fn watch(&self, classify: bool) -> Sighting;
    /// UI thread: starts a run that reports only through the logger.
    fn start(&self, mode: RunMode);
    fn stop(&self);
}

/// What the window opens with.
pub struct StartInfo {
    pub codex: CodexStatus,
    pub browser: BrowserStep,
    /// The CLI's mode, used for the first run instead of detecting the page.
    pub preset: Option<RunMode>,
}

/// The logger sink: queues a copy of each event and wakes the window, without blocking.
struct ChannelSink {
    events: Sender<Event>,
    window: AppHandle<Ui>,
}

impl Sink for ChannelSink {
    fn emit(&self, event: &Event) {
        if self.events.send(event.clone()).is_ok() {
            self.window.wake();
        }
    }
}

pub struct Ui {
    flow: Flow,
    host: Arc<dyn Host>,
    events: Arc<Mutex<Receiver<Event>>>,
    codex: Latest<CodexStatus>,
    signed_in: Latest<Result<(), String>>,
    probe: Latest<Connection>,
    opened: Latest<Result<(), String>>,
    watch: Latest<Sighting>,
    /// When Home probes or Waiting looks next, on the window clock.
    next_look: f64,
    /// Whether the run's raw log is unfolded.
    details: bool,
}

/// Steps the flow and carries out what it asks for.
fn send(s: &mut Ui, input: Input, tx: &mut Tx) {
    for effect in step(&mut s.flow, input) {
        let host = s.host.clone();
        match effect {
            Effect::CheckCodex => s.codex.request((), move |()| host.codex_status()),
            Effect::SignIn => s.signed_in.request((), move |()| host.sign_in()),
            Effect::CheckBrowsers => {
                let browsers = host.browsers();
                send(s, Input::BrowserSaved(Ok(browsers)), tx);
            }
            Effect::SaveBrowser(pick) => {
                let saved = host.save_browser(&pick);
                send(s, Input::BrowserSaved(saved), tx);
            }
            Effect::ChooseFile => {
                let directory = cfg!(target_os = "macos").then(|| "/Applications".into());
                let options =
                    FileDialogOptions { title: "Choose your browser".into(), directory, ..Default::default() };
                tx.open_file(options, |s: &mut Ui, result, tx| {
                    let chosen = result.map(|path| path.map(|p| browser_executable(&p))).map_err(|e| e.to_string());
                    send(s, Input::FileChosen(chosen), tx);
                });
            }
            Effect::Probe | Effect::Watch => {
                s.next_look = tx.clock();
                tx.wake_at(s.next_look);
            }
            Effect::OpenBrowser => s.opened.request((), move |()| host.open_browser()),
            Effect::StartRun(mode) => {
                s.watch.cancel();
                host.start(mode);
            }
            Effect::StopRun => host.stop(),
        }
    }
}

/// Seconds between looks at the browser.
const LOOK_EVERY: f64 = 1.0;

fn poll(s: &mut Ui, clock: f64, tx: &mut Tx) -> bool {
    let events: Vec<Event> = s.events.lock().unwrap_or_else(|e| e.into_inner()).try_iter().collect();
    let mut changed = !events.is_empty();
    for event in events {
        send(s, Input::Log(event), tx);
    }
    let before = s.flow.clone();
    if let Some(status) = s.codex.take() {
        send(s, Input::CodexChecked(status), tx);
    }
    if let Some(result) = s.signed_in.take() {
        send(s, Input::SignInDone(result), tx);
    }
    if let Some(result) = s.opened.take() {
        send(s, Input::Opened(result), tx);
    }
    if let Some(connection) = s.probe.take() {
        send(s, Input::Probed(connection), tx);
    }
    if let Some(sighting) = s.watch.take() {
        send(s, Input::Watched(sighting), tx);
    }
    changed |= s.flow != before;
    let (probing, watching) = (s.flow.probing(), s.flow.watching());
    if probing || watching {
        let busy = if probing { s.probe.is_pending() } else { s.watch.is_pending() };
        if clock >= s.next_look && !busy {
            let host = s.host.clone();
            if probing {
                s.probe.request((), move |()| host.probe());
            } else {
                let classify = s.flow.preset.is_none();
                s.watch.request((), move |()| host.watch(classify));
            }
            s.next_look = clock + LOOK_EVERY;
        }
        tx.wake_at(s.next_look);
    }
    changed
}

fn setup(s: &mut Ui, waker: Waker) {
    s.codex.set_waker(waker.clone());
    s.signed_in.set_waker(waker.clone());
    s.probe.set_waker(waker.clone());
    s.opened.set_waker(waker.clone());
    s.watch.set_waker(waker);
}

fn view(s: &Ui, cx: &mut Cx<Ui>) -> El {
    let p = palette(cx.system_dark());
    match &s.flow.screen {
        Screen::Welcome => welcome::view(&s.flow, &p),
        Screen::Home => home::home(&s.flow, &p),
        Screen::Waiting(waiting) => home::waiting(&s.flow, waiting, &p),
        Screen::Running(board) => {
            let stop = button("stop", "Stop", Kind::Secondary, true, &p, |s: &mut Ui, tx| send(s, Input::Stop, tx));
            dashboard::view(board, Phase::Running, vec![stop], s.details, &p)
        }
        Screen::Stopping(board) => {
            let stop = button("stop", "Stopping", Kind::Secondary, false, &p, |_: &mut Ui, _| {});
            dashboard::view(board, Phase::Stopping, vec![stop], s.details, &p)
        }
        Screen::Finished(board) => {
            let again = button("run-again", "Run again", Kind::Primary, true, &p, |s: &mut Ui, tx| {
                send(s, Input::RunAgain, tx)
            })
            .default_button();
            dashboard::view(board, Phase::Finished, vec![again], s.details, &p)
        }
    }
}

fn new_ui(info: StartInfo, host: Arc<dyn Host>, events: Receiver<Event>) -> Ui {
    Ui {
        flow: Flow::new(info.codex, info.browser, info.preset),
        host,
        events: Arc::new(Mutex::new(events)),
        codex: Latest::default(),
        signed_in: Latest::default(),
        probe: Latest::default(),
        opened: Latest::default(),
        watch: Latest::default(),
        next_look: 0.0,
        details: false,
    }
}

fn app(spec: AppSpec<Ui>, init: impl FnOnce() -> Ui + 'static) -> App<Ui> {
    let spec = AppSpec { poll: Some(poll), setup: Some(setup), min_size: Some((640.0, 480.0)), ..spec };
    // Rune reads its own flags from argv by default; the bot's arguments are not for it.
    App::new(spec)
        // A standard title bar, so the header never sits under the traffic lights.
        .chrome(Chrome { full_size_content: false, ..Chrome::DEFAULT })
        // No text fields here, so Tab has to reach the buttons whatever the system setting.
        .tab_visits_controls(true)
        .init(init)
        .args(Vec::<String>::new())
}

const SIZE: (f32, f32) = (900.0, 720.0);

/// Routes logger events to a new window and runs it on this thread until it closes.
/// Without a display it puts the stdout sink back, prints what it queued, and returns the error.
pub fn run(info: StartInfo, host: Arc<dyn Host>) -> Result<(), String> {
    let (sender, receiver) = mpsc::channel();
    let shared = Arc::new(Mutex::new(Some(receiver)));
    let take = shared.clone();
    let app = app(AppSpec::new("Bilinkers", SIZE, demo::placeholder, view), move || {
        let events = take.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_else(|| mpsc::channel().1);
        new_ui(info, host, events)
    });
    logger::set_sink(Some(Box::new(ChannelSink { events: sender, window: app.handle() })));
    let result = app.try_run();
    logger::set_sink(None);
    if result.is_err() {
        if let Some(receiver) = shared.lock().unwrap_or_else(|e| e.into_inner()).take() {
            for event in receiver.try_iter() {
                println!("{}", event.to_json_line());
            }
        }
    }
    result
}

/// Whether a window can open here: always on macOS and Windows, else when X11 or Wayland is set.
pub fn display_available() -> bool {
    if cfg!(any(target_os = "macos", target_os = "windows")) {
        return true;
    }
    ["DISPLAY", "WAYLAND_DISPLAY"].iter().any(|name| std::env::var_os(name).is_some_and(|v| !v.is_empty()))
}

/// Dev: renders the window headless with a fake Codex, browser and bot, and Rune's tooling
/// flags (`--scene home`, `--snap out.png`, `--script s.json`, `--tree tree.json`, `--theme light`).
pub fn render_headless(rune_args: Vec<String>) {
    let (sender, receiver) = mpsc::channel();
    let spec = AppSpec { scenes: demo::SCENES, ..AppSpec::new("Bilinkers", SIZE, demo::placeholder, view) };
    let app = app(spec, move || new_ui(demo::info(), Arc::new(demo::FakeHost::default()), receiver)).args(rune_args);
    logger::set_sink(Some(Box::new(ChannelSink { events: sender, window: app.handle() })));
    app.run();
    logger::set_sink(None);
}

/// A Codex, browser and bot that don't exist, for headless renders and tests.
mod demo {
    use super::*;
    use crate::detect::{PageKind, PageMode};
    use crate::setup::Origin;
    use crate::ui::flow::{Codex, Waiting};
    use serde_json::json;

    pub const BRAVE: &str = "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser";

    pub fn signed_in() -> CodexStatus {
        CodexStatus::SignedIn("Logged in using ChatGPT".into())
    }

    /// What the search finds on a typical Mac: Chrome dragged into ~/Applications, Brave and
    /// Edge in /Applications.
    pub fn found() -> Vec<FoundBrowser> {
        let at = |kind, path: &str| FoundBrowser { kind, path: path.into() };
        vec![
            at(BrowserKind::Chrome, "/Users/student/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
            at(BrowserKind::Brave, BRAVE),
            at(BrowserKind::Edge, "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
        ]
    }

    pub fn brave() -> BrowserStep {
        BrowserStep::new(found(), BrowserSetup { name: "brave".into(), binary: Ok(BRAVE.into()) }, None)
    }

    pub fn info() -> StartInfo {
        StartInfo { codex: signed_in(), browser: brave(), preset: None }
    }

    pub fn placeholder() -> Ui {
        new_ui(info(), Arc::new(FakeHost::default()), mpsc::channel().1)
    }

    pub fn page(title: &str, url: &str, page: Option<PageKind>) -> Sighting {
        Sighting::Lms { title: title.into(), url: url.into(), page }
    }

    pub fn unit_page() -> Sighting {
        page(
            "Unit 5 | Beelingua",
            "https://lms.binus.ac.id/beelingua/student/class/5",
            Some(PageKind::Mode(PageMode::Unit)),
        )
    }

    /// Signed in, Brave installed, the browser showing `sighting`; a run logs the demo events
    /// up to the last one.
    pub struct FakeHost {
        pub sighting: Sighting,
    }

    impl Default for FakeHost {
        fn default() -> FakeHost {
            FakeHost { sighting: unit_page() }
        }
    }

    impl Host for FakeHost {
        fn codex_status(&self) -> CodexStatus {
            signed_in()
        }

        fn sign_in(&self) -> Result<(), String> {
            Ok(())
        }

        fn browsers(&self) -> BrowserStep {
            brave()
        }

        fn save_browser(&self, _: &BrowserPick) -> Result<BrowserStep, String> {
            Ok(brave())
        }

        fn probe(&self) -> Connection {
            match &self.sighting {
                Sighting::NotRunning => Connection::NotRunning,
                Sighting::NoLmsTab => Connection::NoLmsTab,
                Sighting::Lms { title, url, .. } => Connection::Lms { title: title.clone(), url: url.clone() },
            }
        }

        fn open_browser(&self) -> Result<(), String> {
            Ok(())
        }

        fn watch(&self, _: bool) -> Sighting {
            self.sighting.clone()
        }

        fn start(&self, _: RunMode) {
            let events = demo_events();
            for (level, message, fields) in &events[..events.len() - 2] {
                logger::log(*level, message, fields.iter().cloned());
            }
        }

        fn stop(&self) {
            logger::warn("Run stopped", []);
        }
    }

    fn board(events: &[DemoEvent]) -> Dashboard {
        board_for("Unit", events)
    }

    pub fn board_for(mode: &str, events: &[DemoEvent]) -> Dashboard {
        let mut board = Dashboard::new(mode);
        for (level, message, fields) in events {
            board.apply(&Event {
                time: std::time::SystemTime::now(),
                level: *level,
                message: message.to_string(),
                fields: fields.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
            });
        }
        board
    }

    fn welcome(s: &mut Ui, codex: Codex, browser: BrowserStep) {
        s.flow.screen = Screen::Welcome;
        s.flow.codex = codex;
        s.flow.browser = browser;
    }

    fn nothing_installed() -> BrowserStep {
        let missing = "BROWSER=chrome: searched /Applications, ~/Applications and Spotlight for Google Chrome.app and did not find Google Chrome.";
        BrowserStep::new(vec![], BrowserSetup { name: "chrome".into(), binary: Err(missing.into()) }, None)
    }

    fn waiting(s: &mut Ui, sighting: Sighting) {
        s.host = Arc::new(FakeHost { sighting: sighting.clone() });
        s.flow.screen = Screen::Waiting(Waiting::Looking(Some(sighting)));
    }

    fn home(s: &mut Ui, sighting: Sighting) {
        s.host = Arc::new(FakeHost { sighting: sighting.clone() });
        s.flow.connection = FakeHost { sighting }.probe();
    }

    type Scene = (&'static str, fn(&mut Ui));

    pub const SCENES: &[Scene] = &[
        ("welcome-empty", |s| welcome(s, Codex::Known(CodexStatus::SignedOut), nothing_installed())),
        ("welcome-signing-in", |s| welcome(s, Codex::SigningIn, brave())),
        ("welcome-missing-codex", |s| welcome(s, Codex::Known(CodexStatus::Missing), brave())),
        ("welcome-done", |s| welcome(s, Codex::Known(signed_in()), brave())),
        ("welcome-locked", |s| {
            let locked = BrowserStep::new(
                found(),
                BrowserSetup { name: "brave".into(), binary: Ok(BRAVE.into()) },
                Some(Origin::DotEnv),
            );
            welcome(s, Codex::Known(signed_in()), locked)
        }),
        ("home", |s| home(s, Sighting::NotRunning)),
        ("home-lms", |s| home(s, unit_page())),
        ("waiting-no-browser", |s| waiting(s, Sighting::NotRunning)),
        ("waiting-final-test", |s| {
            waiting(s, page("Final Test | Beelingua", "https://lms.binus.ac.id/beelingua/x", Some(PageKind::FinalTest)))
        }),
        ("waiting-open-failed", |s| {
            s.flow.screen = Screen::Waiting(Waiting::OpenFailed("Failed to launch browser at \"/Applications/Brave Browser.app/Contents/MacOS/Brave Browser\": permission denied".into()));
        }),
        ("waiting-opening", |s| s.flow.screen = Screen::Waiting(Waiting::Opening)),
        ("running", |s| s.flow.screen = Screen::Running(board(&demo_events()[..14]))),
        ("running-details", |s| {
            s.flow.screen = Screen::Running(board(&demo_events()[..14]));
            s.details = true;
        }),
        ("finished", |s| s.flow.screen = Screen::Finished(board(&demo_events()))),
        ("finished-all-courses", |s| {
            s.flow.screen = Screen::Finished(board_for(RunMode::Island.label(), &all_courses_events(false)));
        }),
        ("finished-all-courses-locked", |s| {
            s.flow.screen = Screen::Finished(board_for(RunMode::Island.label(), &all_courses_events(true)));
        }),
        ("finished-unit-done", |s| {
            let done = vec![
                (Level::Info, "No unfinished activities left in this unit — done", vec![("total", json!(6))]),
                (
                    Level::Info,
                    "Run finished",
                    vec![("status", json!("complete")), ("total", json!(6)), ("mode", json!("unit"))],
                ),
            ];
            s.flow.screen = Screen::Finished(board(&done));
        }),
        ("stuck", |s| {
            let mut events = demo_events()[..14].to_vec();
            events.push((Level::Warn, "Unhandled question type: stopping", vec![("questionNum", json!(2))]));
            events.push((Level::Info, "Run finished", vec![("status", json!("unhandled"))]));
            s.flow.screen = Screen::Finished(board(&events));
        }),
        ("stopped", |s| {
            let mut events = demo_events();
            events.truncate(5);
            events.push((Level::Warn, "Run stopped", vec![]));
            s.flow.screen = Screen::Finished(board(&events));
        }),
    ];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::PageKind;
    use serde_json::json;
    use std::time::UNIX_EPOCH;

    fn event(level: Level, message: &str, fields: Vec<(&str, serde_json::Value)>) -> Event {
        Event {
            time: UNIX_EPOCH,
            level,
            message: message.to_string(),
            fields: fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        }
    }

    fn runtime(ui: Ui, dark: bool) -> rune::Runtime<Ui> {
        runtime_sized(ui, dark, SIZE)
    }

    fn runtime_sized(ui: Ui, dark: bool, size: (f32, f32)) -> rune::Runtime<Ui> {
        let mut rt = rune::Runtime::new(ui, view, size, rune::Env { system_dark: dark, scale: 2.0 });
        rt.poll = Some(poll);
        rt
    }

    fn shown(rt: &rune::Runtime<Ui>, key: &str) -> Option<String> {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).and_then(|e| e.text)
    }

    fn click(rt: &mut rune::Runtime<Ui>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} is on screen"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    /// Frames until `done` holds, letting the workers answer.
    fn frames_until(rt: &mut rune::Runtime<Ui>, done: impl Fn(&Ui) -> bool) {
        for _ in 0..300 {
            rt.frame(1.0 / 60.0);
            if done(&rt.state) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("timed out on {:?}", rt.state.flow.screen);
    }

    fn ui(info: StartInfo, host: demo::FakeHost, events: Receiver<Event>) -> Ui {
        new_ui(info, Arc::new(host), events)
    }

    #[test]
    fn the_run_screen_drains_the_channel_and_shows_the_steps_and_header() {
        let (sender, receiver) = mpsc::channel();
        for (level, message, fields) in demo_events() {
            sender.send(event(level, message, fields)).unwrap();
        }
        let mut ui = ui(demo::info(), demo::FakeHost::default(), receiver);
        ui.flow.screen = Screen::Running(Dashboard::new("Course"));
        // Tall enough that Details is in view without scrolling.
        let mut rt = runtime_sized(ui, true, (945.0, 1025.0));
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(shown(&rt, "answered-value").as_deref(), Some("6 answered"));
        assert_eq!(shown(&rt, "pill-detail").as_deref(), Some("5 of 6 correct"));
        assert_eq!(shown(&rt, "step-2-title").as_deref(), Some("Vocabulary: Daily routines"));
        assert_eq!(shown(&rt, "step-5-state").as_deref(), Some("Correct after 2 tries"));
        assert_eq!(shown(&rt, "mode").as_deref(), Some("Unit"), "Detected names the mode");
        assert_eq!(shown(&rt, "status-label").as_deref(), Some("Complete"));
        assert_eq!(shown(&rt, "run-title").as_deref(), Some("All done"));
        assert!(rt.find("line-16").is_none(), "the log starts folded");
        click(&mut rt, "details");
        assert!(rt.state.details, "Details toggles");
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("line-16")), "Details unfolds the log");
        assert!(rt.find("run-again").is_some() && rt.find("stop").is_none(), "a finished run offers Run again");
    }

    #[test]
    fn a_full_log_drops_old_rows_and_keeps_the_newest_in_view() {
        let (sender, receiver) = mpsc::channel();
        for i in 0..MAX_LOG_LINES + 50 {
            sender.send(event(Level::Info, &format!("line {i}"), vec![("questionNum", json!(i))])).unwrap();
        }
        let mut ui = ui(demo::info(), demo::FakeHost::default(), receiver);
        ui.flow.screen = Screen::Running(Dashboard::new("Unit"));
        ui.details = true;
        let mut rt = runtime(ui, false);
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        let elements = rt.elements();
        let rect = |key: &str| elements.iter().find(|e| e.key.as_deref() == Some(key)).map(|e| e.rect);
        assert!(rect("line-49").is_none(), "the oldest rows are gone");
        let (log, newest) = (rect("log").unwrap(), rect(&format!("line-{}", MAX_LOG_LINES + 49)).unwrap());
        assert!(
            newest[1] >= log[1] && newest[1] + newest[3] <= log[1] + log[3] + 0.5,
            "{newest:?} sits inside {log:?}"
        );
    }

    #[test]
    fn continue_then_start_on_a_unit_page_reaches_the_dashboard() {
        let info = StartInfo { codex: CodexStatus::SignedOut, ..demo::info() };
        let mut rt = runtime(ui(info, demo::FakeHost::default(), mpsc::channel().1), true);
        rt.frame(0.0);
        assert_eq!(rt.state.flow.screen, Screen::Welcome);
        assert_eq!(shown(&rt, "codex-status-text").as_deref(), Some("Not signed in yet."));
        click(&mut rt, "continue");
        assert_eq!(rt.state.flow.screen, Screen::Welcome, "Continue is off until the sign-in");
        click(&mut rt, "sign-in");
        assert_eq!(shown(&rt, "codex-status-text").as_deref(), Some(welcome::SIGNING_IN));
        frames_until(&mut rt, |s| s.flow.signed_in());
        assert_eq!(shown(&rt, "codex-status-text").as_deref(), Some("Signed in with ChatGPT"));
        click(&mut rt, "continue");
        assert_eq!(rt.state.flow.screen, Screen::Home);
        frames_until(&mut rt, |s| s.flow.connection.has_lms_tab());
        click(&mut rt, "start");
        frames_until(&mut rt, |s| matches!(s.flow.screen, Screen::Running(_)));
        assert!(rt.find("stop").is_some());
        assert_eq!(shown(&rt, "mode").as_deref(), Some("Unit"));
    }

    #[test]
    fn the_welcome_screen_lists_each_found_browser_with_its_path() {
        let info = StartInfo { codex: CodexStatus::SignedOut, ..demo::info() };
        let mut rt = runtime(ui(info, demo::FakeHost::default(), mpsc::channel().1), false);
        rt.frame(0.0);
        for found in demo::found() {
            let path = shown(&rt, &format!("browser-{}-path", found.kind.key()));
            assert_eq!(path, Some(found.path.display().to_string()));
        }
        assert!(rt.find("browser-chromium").is_none(), "only what the search found");
        assert_eq!(shown(&rt, "credit-text").as_deref(), Some(style::CREDIT));
    }

    #[test]
    fn the_island_map_starts_the_all_courses_run() {
        let island = Some(PageKind::Mode(crate::detect::PageMode::Island));
        let sighting = demo::page("Beelingua", "https://lms.binus.ac.id/beelingua/", island);
        let mut rt = runtime(ui(demo::info(), demo::FakeHost { sighting }, mpsc::channel().1), false);
        rt.frame(0.0);
        click(&mut rt, "start");
        frames_until(&mut rt, |s| matches!(s.flow.screen, Screen::Running(_)));
        assert_eq!(shown(&rt, "mode").as_deref(), Some("All courses"));
    }

    #[test]
    fn the_all_courses_finished_screen_lists_each_course() {
        let mut ui = ui(demo::info(), demo::FakeHost::default(), mpsc::channel().1);
        ui.flow.screen = Screen::Finished(demo::board_for("All courses", &all_courses_events(true)));
        let mut rt = runtime_sized(ui, false, (945.0, 1025.0));
        rt.frame(0.0);
        assert_eq!(shown(&rt, "run-title").as_deref(), Some("All courses finished"));
        assert_eq!(
            shown(&rt, "run-text").as_deref(),
            Some("Nothing left to do on Beelingua. The bot checked B1.1 to C2.2.")
        );
        assert_eq!(shown(&rt, "step-1-title").as_deref(), Some("B1.1"));
        assert_eq!(shown(&rt, "step-1-state").as_deref(), Some("Finished"));
        assert_eq!(shown(&rt, "step-8-title").as_deref(), Some("C2.2"));
        assert_eq!(shown(&rt, "step-8-state").as_deref(), Some("Locked"));
        assert_eq!(shown(&rt, "status-label").as_deref(), Some("All courses complete"));
        assert!(rt.find("run-again").is_some());
    }

    #[test]
    fn a_unit_with_nothing_left_says_so() {
        let mut ui = ui(demo::info(), demo::FakeHost::default(), mpsc::channel().1);
        let done = [(Level::Info, "Run finished", vec![("status", json!("complete")), ("mode", json!("unit"))])];
        ui.flow.screen = Screen::Finished(demo::board_for("Unit", &done));
        let mut rt = runtime(ui, true);
        rt.frame(0.0);
        assert_eq!(shown(&rt, "run-title").as_deref(), Some("This unit is finished"));
        assert_eq!(
            shown(&rt, "step-empty-text").as_deref(),
            Some("Nothing left to do. Everything here was already done.")
        );
    }

    #[test]
    fn a_final_test_page_is_refused_in_the_window() {
        let test = demo::page("Final Test", "https://lms.binus.ac.id/x", Some(PageKind::FinalTest));
        let mut rt = runtime(ui(demo::info(), demo::FakeHost { sighting: test }, mpsc::channel().1), true);
        rt.frame(0.0);
        click(&mut rt, "start");
        frames_until(&mut rt, |s| matches!(&s.flow.screen, Screen::Waiting(flow::Waiting::Looking(Some(_)))));
        for _ in 0..90 {
            rt.frame(1.0 / 60.0);
        }
        assert!(matches!(rt.state.flow.screen, Screen::Waiting(_)), "never starts");
        assert_eq!(shown(&rt, "wait-status-text").as_deref(), Some(home::FINAL_TEST_REFUSED));
    }

    #[test]
    fn home_shows_what_start_will_do() {
        let mut rt =
            runtime(ui(demo::info(), demo::FakeHost { sighting: Sighting::NotRunning }, mpsc::channel().1), false);
        rt.frame(0.0);
        frames_until(&mut rt, |s| s.flow.connection == Connection::NotRunning);
        assert_eq!(shown(&rt, "codex-line-text").as_deref(), Some("Signed in"));
        assert_eq!(shown(&rt, "browser-line-text").as_deref(), Some("Brave"));
        assert_eq!(shown(&rt, "connection-line-text").as_deref(), Some("Not open yet. Start opens it."));
        assert_eq!(shown(&rt, "how-1-about").as_deref(), Some("Opens Brave at the LMS."));
        assert!(rt.find("mode-course").is_none(), "no mode picker");
        assert_eq!(shown(&rt, "credit-text").as_deref(), Some(style::CREDIT));
        click(&mut rt, "setup");
        assert_eq!(rt.state.flow.screen, Screen::Welcome);
    }
}
