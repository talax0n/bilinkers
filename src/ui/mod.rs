//! The desktop window: a start screen to pick the mode, open the browser and start, then the
//! run's dashboard. Logger events reach it through a channel sink, so the bot never waits on it,
//! and the browser is probed and launched on workers, so the window never waits on them.

pub mod dashboard;
pub mod flow;
mod start;
mod style;

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use rune::tok;
use rune::{col, App, AppHandle, AppSpec, Chrome, Cx, El, Latest, Tx, Waker};
use serde_json::json;

use crate::logger::{self, Event, Level, Sink};
use crate::session::{BrowserSetup, Connection, RunMode};
pub use dashboard::{demo_events, Dashboard, DemoEvent, RunStatus, MAX_LOG_LINES};
use flow::{step, Effect, Flow, Input, Screen};
use style::{button, tones, Kind};

/// What the window asks of the browser and the bot. Probe and open run on a worker; start and
/// stop are called on the UI thread and must return at once.
pub trait Host: Send + Sync {
    /// A read-only look at the debug port.
    fn probe(&self) -> Connection;
    fn open_browser(&self) -> Result<(), String>;
    /// Starts a run off the UI thread; it reports only through the logger.
    fn start(&self, mode: Option<RunMode>);
    fn stop(&self);
}

/// What the start screen shows before the first probe.
pub struct StartInfo {
    /// The CLI's mode, None for Auto-detect.
    pub preselect: Option<RunMode>,
    pub browser: BrowserSetup,
    pub auto_answer: bool,
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
    probe: Latest<Connection>,
    opened: Latest<Result<(), String>>,
    /// When the start screen probes next, on the window clock.
    next_probe: f64,
}

/// Steps the flow and carries out what it asks for.
fn send(s: &mut Ui, input: Input, tx: &mut Tx) {
    for effect in step(&mut s.flow, input) {
        match effect {
            Effect::Probe => {
                s.next_probe = tx.clock();
                tx.wake_at(s.next_probe);
            }
            Effect::OpenBrowser => {
                let host = s.host.clone();
                s.opened.request((), move |()| host.open_browser());
            }
            Effect::StartRun(mode) => s.host.start(mode),
            Effect::StopRun => s.host.stop(),
        }
    }
}

/// Seconds between probes of the debug port.
const PROBE_EVERY: f64 = 1.0;

fn poll(s: &mut Ui, clock: f64, tx: &mut Tx) -> bool {
    let events: Vec<Event> = s.events.lock().unwrap_or_else(|e| e.into_inner()).try_iter().collect();
    let mut changed = !events.is_empty();
    for event in events {
        send(s, Input::Log(event), tx);
    }
    if let Some(result) = s.opened.take() {
        send(s, Input::Opened(result), tx);
        changed = true;
    }
    if let Some(connection) = s.probe.take() {
        changed |= connection != s.flow.connection;
        send(s, Input::Probed(connection), tx);
    }
    if s.flow.probing() {
        if clock >= s.next_probe && !s.probe.is_pending() {
            let host = s.host.clone();
            s.probe.request((), move |()| host.probe());
            s.next_probe = clock + PROBE_EVERY;
        }
        tx.wake_at(s.next_probe);
    }
    changed
}

fn setup(s: &mut Ui, waker: Waker) {
    s.probe.set_waker(waker.clone());
    s.opened.set_waker(waker);
}

fn view(s: &Ui, cx: &mut Cx<Ui>) -> El {
    let dark = cx.system_dark();
    let t = tok::theme(dark);
    let tones = tones(dark);
    let screen = match &s.flow.screen {
        Screen::Start => start::view(&s.flow, &t, &tones),
        Screen::Running(board) => {
            let stop =
                button("stop", "Stop", Kind::Secondary, true, &t, &tones, |s: &mut Ui, tx| send(s, Input::Stop, tx));
            dashboard::view(board, stop, &t, &tones)
        }
        Screen::Stopping(board) => dashboard::view(
            board,
            button("stop", "Stopping", Kind::Secondary, false, &t, &tones, |_: &mut Ui, _| {}),
            &t,
            &tones,
        ),
        Screen::Finished(board) => {
            let again = button("new-run", "New run", Kind::Primary, true, &t, &tones, |s: &mut Ui, tx| {
                send(s, Input::NewRun, tx)
            })
            .default_button();
            dashboard::view(board, again, &t, &tones)
        }
    };
    col().w_full().h_full().padding(16.0).fill(t.bg).child(screen)
}

fn new_ui(info: StartInfo, host: Arc<dyn Host>, events: Receiver<Event>) -> Ui {
    Ui {
        flow: Flow::new(info.preselect, info.browser, info.auto_answer),
        host,
        events: Arc::new(Mutex::new(events)),
        probe: Latest::default(),
        opened: Latest::default(),
        next_probe: 0.0,
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

const SIZE: (f32, f32) = (820.0, 560.0);

/// Routes logger events to a new window and runs it on this thread until it closes.
/// Without a display it puts the stdout sink back, prints what it queued, and returns the error.
pub fn run(info: StartInfo, host: Arc<dyn Host>) -> Result<(), String> {
    let (sender, receiver) = mpsc::channel();
    let shared = Arc::new(Mutex::new(Some(receiver)));
    let take = shared.clone();
    let app = app(AppSpec::new("Beelingua Bot", SIZE, demo::placeholder, view), move || {
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

/// Dev: renders the window headless with a fake browser and Rune's tooling flags
/// (`--scene lms-tab`, `--snap out.png`, `--script s.json`, `--tree tree.json`, `--theme light`).
pub fn render_headless(rune_args: Vec<String>) {
    let (sender, receiver) = mpsc::channel();
    let spec = AppSpec { scenes: demo::SCENES, ..AppSpec::new("Beelingua Bot", SIZE, demo::placeholder, view) };
    let app = app(spec, move || new_ui(demo::info(), Arc::new(demo::FakeHost::new(Connection::NotRunning)), receiver))
        .args(rune_args);
    logger::set_sink(Some(Box::new(ChannelSink { events: sender, window: app.handle() })));
    app.run();
    logger::set_sink(None);
}

/// A browser and bot that don't exist, for headless renders and tests.
mod demo {
    use super::*;

    pub const CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

    pub fn info() -> StartInfo {
        StartInfo {
            preselect: None,
            browser: BrowserSetup { name: "chrome".into(), binary: Ok(CHROME.into()) },
            auto_answer: false,
        }
    }

    pub fn placeholder() -> Ui {
        new_ui(info(), Arc::new(FakeHost::new(Connection::NotRunning)), mpsc::channel().1)
    }

    /// Answers every probe with one connection; a run logs the demo events up to the last one.
    pub struct FakeHost {
        connection: Connection,
    }

    impl FakeHost {
        pub fn new(connection: Connection) -> FakeHost {
            FakeHost { connection }
        }
    }

    impl Host for FakeHost {
        fn probe(&self) -> Connection {
            self.connection.clone()
        }

        fn open_browser(&self) -> Result<(), String> {
            Ok(())
        }

        fn start(&self, mode: Option<RunMode>) {
            if mode.is_none() {
                logger::info("Detected", [("mode", json!("course"))]);
            }
            let events = demo_events();
            for (level, message, fields) in &events[..events.len() - 2] {
                logger::log(*level, message, fields.iter().cloned());
            }
        }

        fn stop(&self) {
            logger::warn("Run stopped", []);
        }
    }

    pub fn lms_tab() -> Connection {
        Connection::Lms {
            title: "Unit 5 Exercise | Beelingua".into(),
            url: "https://lms.binus.ac.id/course/unit-5".into(),
        }
    }

    fn with_connection(s: &mut Ui, connection: Connection) {
        s.host = Arc::new(FakeHost::new(connection.clone()));
        s.flow.connection = connection;
    }

    fn board(events: &[DemoEvent]) -> Dashboard {
        let mut board = Dashboard::new("course");
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

    type Scene = (&'static str, fn(&mut Ui));

    pub const SCENES: &[Scene] = &[
        ("no-browser", |s| with_connection(s, Connection::NotRunning)),
        ("no-lms-tab", |s| with_connection(s, Connection::NoLmsTab)),
        ("lms-tab", |s| with_connection(s, lms_tab())),
        ("missing-browser", |s| {
            s.flow.browser.binary = Err("BROWSER=chrome defaults to \"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome\", but it's not installed there. Set BROWSER to a browser you have (chrome/brave/edge) or BROWSER_BINARY_PATH to its location.".into());
            with_connection(s, Connection::NotRunning);
        }),
        ("final-test", |s| {
            with_connection(s, lms_tab());
            s.flow.choice = Some(RunMode::FinalTest);
        }),
        ("final-test-auto", |s| {
            with_connection(s, lms_tab());
            s.flow.choice = Some(RunMode::FinalTest);
            s.flow.auto_answer = true;
        }),
        ("running", |s| {
            let events = demo_events();
            s.flow.screen = Screen::Running(board(&events[..events.len() - 2]));
        }),
        ("finished", |s| s.flow.screen = Screen::Finished(board(&demo_events()))),
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
        let mut rt = rune::Runtime::new(ui, view, SIZE, rune::Env { system_dark: dark, scale: 2.0 });
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

    #[test]
    fn the_dashboard_drains_the_channel_and_shows_the_tally_and_header() {
        let (sender, receiver) = mpsc::channel();
        for (level, message, fields) in demo_events() {
            sender.send(event(level, message, fields)).unwrap();
        }
        let mut ui = new_ui(demo::info(), Arc::new(demo::FakeHost::new(demo::lms_tab())), receiver);
        ui.flow.screen = Screen::Running(Dashboard::new("course"));
        let mut rt = runtime(ui, true);
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(shown(&rt, "correct-value").as_deref(), Some("3"));
        assert_eq!(shown(&rt, "incorrect-value").as_deref(), Some("1"));
        assert_eq!(shown(&rt, "question-value").as_deref(), Some("#4"));
        assert_eq!(shown(&rt, "question-type").as_deref(), Some("multiple-choice"));
        assert_eq!(shown(&rt, "mode").as_deref(), Some("course"));
        assert_eq!(shown(&rt, "status-label").as_deref(), Some("complete"));
        assert!(rt.find("line-9").is_some(), "the newest log line is laid out");
        assert!(rt.find("new-run").is_some() && rt.find("stop").is_none(), "a finished run offers New run");
    }

    #[test]
    fn a_full_log_drops_old_rows_and_keeps_the_newest_in_view() {
        let (sender, receiver) = mpsc::channel();
        for i in 0..MAX_LOG_LINES + 50 {
            sender.send(event(Level::Info, &format!("line {i}"), vec![("questionNum", json!(i))])).unwrap();
        }
        let mut ui = new_ui(demo::info(), Arc::new(demo::FakeHost::new(demo::lms_tab())), receiver);
        ui.flow.screen = Screen::Running(Dashboard::new("unit"));
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
    fn picking_course_and_pressing_start_reaches_the_dashboard() {
        let ui = new_ui(demo::info(), Arc::new(demo::FakeHost::new(demo::lms_tab())), mpsc::channel().1);
        let mut rt = runtime(ui, true);
        rt.frame(0.0);
        assert_eq!(rt.state.flow.connection, Connection::Checking);
        for _ in 0..120 {
            rt.frame(1.0 / 60.0);
            if rt.state.flow.connection != Connection::Checking {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(rt.state.flow.connection, demo::lms_tab(), "the probe landed");
        click(&mut rt, "mode-course");
        assert_eq!(rt.state.flow.choice, Some(RunMode::Course));
        click(&mut rt, "start");
        assert!(matches!(rt.state.flow.screen, Screen::Running(_)));
        assert!(rt.find("stop").is_some());
        assert_eq!(shown(&rt, "mode").as_deref(), Some("course"));
    }

    #[test]
    fn start_does_nothing_without_an_lms_tab() {
        let ui = new_ui(demo::info(), Arc::new(demo::FakeHost::new(Connection::NoLmsTab)), mpsc::channel().1);
        let mut rt = runtime(ui, false);
        rt.frame(0.0);
        click(&mut rt, "start");
        assert_eq!(rt.state.flow.screen, Screen::Start);
    }
}
