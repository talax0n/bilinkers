//! Desktop dashboard for a run, the Rune port of `src/dashboard.js`: the mode and run status,
//! the correct and incorrect tallies, the current question and a scrolling log.
//! Logger events reach the window through a channel sink, so the bot never waits on it.

use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use rune::tok::{self, type_, Family, TextStyle};
use rune::{col, el, oklch, row, text, App, AppHandle, AppSpec, Color, Cx, El, ScrollSpec, Tx};
use serde_json::{json, Value};

use crate::logger::{self, Event, Level, Sink};

/// A logger call to replay: level, message and fields.
pub type DemoEvent = (Level, &'static str, Vec<(&'static str, Value)>);

/// Matches the JS dashboard's MAX_LOG_LINES.
pub const MAX_LOG_LINES: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    /// The `status` field of "Run finished": complete, stuck, unhandled, passed and so on.
    Finished(String),
    Failed,
}

impl RunStatus {
    pub fn label(&self) -> &str {
        match self {
            RunStatus::Running => "running",
            RunStatus::Finished(status) => status,
            RunStatus::Failed => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LogLine {
    /// Unique for the run, so rows keep their keys as old lines drop off the top.
    pub id: u64,
    pub level: Level,
    pub message: String,
    /// The fields as compact JSON, empty when the event has none.
    pub meta: String,
}

/// What the window shows, built only from logger events.
#[derive(Debug, Clone, PartialEq)]
pub struct Dashboard {
    pub mode: String,
    pub status: RunStatus,
    pub correct: u32,
    pub incorrect: u32,
    pub question: Option<u64>,
    pub question_type: Option<String>,
    pub lines: VecDeque<LogLine>,
    next_id: u64,
}

impl Dashboard {
    pub fn new(mode: &str) -> Dashboard {
        Dashboard {
            mode: mode.to_string(),
            status: RunStatus::Running,
            correct: 0,
            incorrect: 0,
            question: None,
            question_type: None,
            lines: VecDeque::new(),
            next_id: 0,
        }
    }

    pub fn apply(&mut self, event: &Event) {
        let field = |name: &str| event.fields.iter().find(|(k, _)| k == name).map(|(_, v)| v);
        let status = field("status").and_then(Value::as_str);
        // Activity reports carry statuses too, so only per-question events count toward the tally.
        if event.message == "Question processed" {
            match status {
                Some("correct") => self.correct += 1,
                Some("incorrect") => self.incorrect += 1,
                _ => {}
            }
        }
        if let Some(num) = field("questionNum").and_then(Value::as_u64) {
            let kind = field("type").and_then(Value::as_str).map(str::to_string);
            if kind.is_some() || self.question != Some(num) {
                self.question_type = kind;
            }
            self.question = Some(num);
        }
        match event.message.as_str() {
            "Run finished" => self.status = RunStatus::Finished(status.unwrap_or("complete").to_string()),
            "Run failed" => self.status = RunStatus::Failed,
            _ => {}
        }

        let meta = if event.fields.is_empty() {
            String::new()
        } else {
            let pairs: Vec<String> = event.fields.iter().map(|(k, v)| format!("{}:{v}", json!(k))).collect();
            format!("{{{}}}", pairs.join(","))
        };
        self.lines.push_back(LogLine { id: self.next_id, level: event.level, message: event.message.clone(), meta });
        self.next_id += 1;
        while self.lines.len() > MAX_LOG_LINES {
            self.lines.pop_front();
        }
    }
}

/// The logger sink: queues a copy of each event and wakes the window, without blocking.
struct ChannelSink {
    events: Sender<Event>,
    window: AppHandle<State>,
}

impl Sink for ChannelSink {
    fn emit(&self, event: &Event) {
        if self.events.send(event.clone()).is_ok() {
            self.window.wake();
        }
    }
}

struct State {
    board: Dashboard,
    events: Arc<Mutex<Receiver<Event>>>,
}

fn poll(s: &mut State, _: f64, _: &mut Tx) -> bool {
    let events = s.events.lock().unwrap_or_else(|e| e.into_inner());
    let mut changed = false;
    while let Ok(event) = events.try_recv() {
        s.board.apply(&event);
        changed = true;
    }
    changed
}

/// The colours the theme tokens don't have, tuned per appearance for contrast on `bg` and `card`.
struct Tones {
    good: Color,
    warn: Color,
    bad: Color,
    accent: Color,
}

fn tones(dark: bool) -> Tones {
    if dark {
        Tones {
            good: oklch(0.78, 0.13, 150.0),
            warn: oklch(0.82, 0.12, 80.0),
            bad: oklch(0.72, 0.16, 25.0),
            accent: tok::accent(0, true),
        }
    } else {
        Tones {
            good: oklch(0.5, 0.13, 150.0),
            warn: oklch(0.55, 0.12, 65.0),
            bad: oklch(0.52, 0.18, 27.0),
            accent: oklch(0.5, 0.15, 256.0),
        }
    }
}

const MONO: TextStyle = TextStyle::new(12.0, 400.0).family(Family::SystemMono).lh(17.0);
const MONO_LEVEL: TextStyle = TextStyle::new(12.0, 600.0).family(Family::SystemMono).lh(17.0);
const FIGURE: TextStyle = TextStyle::new(22.0, 600.0).features("'tnum' 1");

fn status_color(status: &RunStatus, tones: &Tones) -> Color {
    match status {
        RunStatus::Running => tones.accent,
        RunStatus::Finished(s) if s == "complete" || s == "passed" => tones.good,
        RunStatus::Finished(_) => tones.warn,
        RunStatus::Failed => tones.bad,
    }
}

fn stat(key: &str, label: &str, value: El, t: &tok::Theme) -> El {
    col()
        .key(key)
        .grow(1.0)
        .gap(2.0)
        .padding(12.0)
        .radius(8.0)
        .fill(t.card)
        .border(1.0, t.card_border)
        .child(text(label.to_string()).font(type_::LABEL).color(t.text_secondary))
        .child(value)
}

fn figure(key: &str, value: String, color: Color) -> El {
    text(value).key(format!("{key}-value")).font(FIGURE).color(color)
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let dark = cx.system_dark();
    let t = tok::theme(dark);
    let tones = tones(dark);
    let board = &s.board;

    let status_color = status_color(&board.status, &tones);
    let status = row()
        .key("status")
        .gap(6.0)
        .center_y()
        .child(el().size(8.0, 8.0).radius(4.0).fill(status_color))
        .child(text(board.status.label().to_string()).key("status-label").font(type_::LABEL).color(status_color));
    let header = row()
        .key("header")
        .w_full()
        .center_y()
        .gap(8.0)
        .child(text("Beelingua Bot").font(TextStyle::new(15.0, 600.0)).color(t.text))
        .child(text(board.mode.clone()).key("mode").font(TextStyle::new(15.0, 400.0)).color(t.text_secondary))
        .child(el().grow(1.0))
        .child(status);

    let number = board.question.map_or("none yet".to_string(), |n| format!("#{n}"));
    let mut question = row().gap(10.0).center_y().child(figure("question", number, t.text));
    if let Some(kind) = &board.question_type {
        question = question.child(text(kind.clone()).key("question-type").font(type_::BODY).color(t.text_secondary));
    }
    let stats = row()
        .w_full()
        .gap(10.0)
        .child(stat("correct", "Correct", figure("correct", board.correct.to_string(), tones.good), &t))
        .child(stat("incorrect", "Incorrect", figure("incorrect", board.incorrect.to_string(), tones.bad), &t))
        .child(stat("question", "Question", question, &t).grow(2.0));

    let mut lines = col().w_full().py(6.0);
    for line in &board.lines {
        let (level_color, message_color) = match line.level {
            Level::Info => (t.text_secondary, t.text),
            Level::Warn => (tones.warn, t.text),
            Level::Error => (tones.bad, tones.bad),
        };
        let mut body =
            row().grow(1.0).min_w(0.0).gap(8.0).child(text(line.message.clone()).font(MONO).color(message_color));
        if !line.meta.is_empty() {
            body = body.child(text(line.meta.clone()).font(MONO).color(t.text_secondary).grow(1.0).min_w(0.0).wrap());
        }
        lines = lines.child(
            row()
                .key(format!("line-{}", line.id))
                .w_full()
                .px(12.0)
                .py(2.0)
                .gap(10.0)
                .child(text(line.level.name().to_string()).font(MONO_LEVEL).color(level_color).w(40.0))
                .child(body),
        );
    }
    let log = col()
        .key("log")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .radius(8.0)
        .fill(t.card)
        .border(1.0, t.card_border)
        .scroll(ScrollSpec { stick_bottom: true })
        .child(lines);

    col().w_full().h_full().padding(16.0).gap(12.0).fill(t.bg).child(header).child(stats).child(log)
}

fn app(mode: &str, events: Arc<Mutex<Receiver<Event>>>) -> App<State> {
    let spec = AppSpec {
        poll: Some(poll),
        min_size: Some((520.0, 360.0)),
        ..AppSpec::new("Beelingua Bot", (820.0, 560.0), placeholder, view)
    };
    let board = Dashboard::new(mode);
    // Rune reads its own flags from argv by default; the bot's arguments are not for it.
    App::new(spec).init(move || State { board, events }).args(Vec::<String>::new())
}

fn placeholder() -> State {
    State { board: Dashboard::new(""), events: Arc::new(Mutex::new(mpsc::channel().1)) }
}

/// Routes logger events to a new window and runs it on this thread until it closes.
/// Without a display it puts the stdout sink back, prints what it queued, and returns the error.
pub fn run(mode: &str) -> Result<(), String> {
    let (sender, receiver) = mpsc::channel();
    let receiver = Arc::new(Mutex::new(receiver));
    let app = app(mode, receiver.clone());
    logger::set_sink(Some(Box::new(ChannelSink { events: sender, window: app.handle() })));
    let result = app.try_run();
    logger::set_sink(None);
    if result.is_err() {
        for event in receiver.lock().unwrap_or_else(|e| e.into_inner()).try_iter() {
            println!("{}", event.to_json_line());
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

/// Dev: logs `events` through the real sink, then renders the window headless with Rune's
/// tooling flags (`--snap out.png`, `--tree tree.json`, `--theme light`, ...).
pub fn render_headless(mode: &str, events: &[DemoEvent], rune_args: Vec<String>) {
    let (sender, receiver) = mpsc::channel();
    let app = app(mode, Arc::new(Mutex::new(receiver))).args(rune_args);
    logger::set_sink(Some(Box::new(ChannelSink { events: sender, window: app.handle() })));
    for (level, message, fields) in events {
        logger::log(*level, message, fields.iter().cloned());
    }
    logger::set_sink(None);
    app.run();
}

/// A fixed run for `render_headless`: three correct answers, one incorrect, a few info and
/// warn lines, then a finished course.
pub fn demo_events() -> Vec<DemoEvent> {
    let question = |n: u32, kind: &str, status: &str, attempts: u32| {
        vec![("questionNum", json!(n)), ("type", json!(kind)), ("status", json!(status)), ("attempts", json!(attempts))]
    };
    vec![
        (Level::Info, "Attached to LMS", vec![("url", json!("https://lms.binus.ac.id/course/unit-5"))]),
        (Level::Info, "Opening course node", vec![("label", json!("Unit 5 Exercise")), ("type", json!("exercise"))]),
        (Level::Info, "Question processed", question(1, "multiple-choice", "correct", 1)),
        (Level::Info, "Question processed", question(2, "fill-in-blank", "correct", 2)),
        (Level::Warn, "Check button not found, retrying", vec![("questionNum", json!(3))]),
        (Level::Info, "Question processed", question(3, "quiz-matching", "incorrect", 3)),
        (Level::Info, "Question processed", question(4, "multiple-choice", "correct", 1)),
        (Level::Warn, "Listening question skipped: audio cannot be heard", vec![]),
        (Level::Info, "Exercise finished", vec![("status", json!("complete"))]),
        (Level::Info, "Run finished", vec![("status", json!("complete"))]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    fn event(level: Level, message: &str, fields: Vec<(&str, Value)>) -> Event {
        Event {
            time: UNIX_EPOCH,
            level,
            message: message.to_string(),
            fields: fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        }
    }

    fn board_after(events: &[DemoEvent]) -> Dashboard {
        let mut board = Dashboard::new("course");
        for (level, message, fields) in events {
            board.apply(&event(*level, message, fields.clone()));
        }
        board
    }

    #[test]
    fn counts_only_question_processed_statuses_and_tracks_the_current_question() {
        let board = board_after(&demo_events());
        assert_eq!((board.correct, board.incorrect), (3, 1), "the exercise's own complete status is not a tally");
        assert_eq!((board.question, board.question_type.as_deref()), (Some(4), Some("multiple-choice")));
        assert_eq!(board.status, RunStatus::Finished("complete".to_string()));
        assert_eq!(board.lines.len(), 10);
        assert_eq!(board.lines[1].meta, r#"{"label":"Unit 5 Exercise","type":"exercise"}"#);
        assert_eq!(board.lines[7].meta, "");
    }

    #[test]
    fn a_new_question_without_a_type_clears_the_old_type() {
        let board = board_after(&[
            (Level::Info, "Question processed", vec![("questionNum", json!(2)), ("type", json!("cloze"))]),
            (Level::Info, "Retrying", vec![("questionNum", json!(2))]),
        ]);
        assert_eq!(board.question_type.as_deref(), Some("cloze"));
        let board = board_after(&[
            (Level::Info, "Question processed", vec![("questionNum", json!(2)), ("type", json!("cloze"))]),
            (Level::Info, "Answering", vec![("questionNum", json!(3))]),
        ]);
        assert_eq!((board.question, board.question_type), (Some(3), None));
    }

    #[test]
    fn run_finished_and_run_failed_set_the_status() {
        let stuck = board_after(&[(Level::Info, "Run finished", vec![("status", json!("stuck"))])]);
        assert_eq!(stuck.status.label(), "stuck");
        let failed = board_after(&[(Level::Error, "Run failed", vec![("error", json!("boom"))])]);
        assert_eq!(failed.status.label(), "error");
        assert_eq!(Dashboard::new("unit").status.label(), "running");
    }

    #[test]
    fn the_log_keeps_the_newest_max_log_lines() {
        let mut board = Dashboard::new("exercise");
        for i in 0..MAX_LOG_LINES + 5 {
            board.apply(&event(Level::Info, &format!("line {i}"), vec![]));
        }
        assert_eq!(board.lines.len(), MAX_LOG_LINES);
        assert_eq!(board.lines.front().map(|l| l.message.as_str()), Some("line 5"));
        assert_eq!(board.lines.back().map(|l| l.id), Some(MAX_LOG_LINES as u64 + 4));
    }

    #[test]
    fn the_view_drains_the_channel_and_shows_the_tally_and_header() {
        let (sender, receiver) = mpsc::channel();
        for (level, message, fields) in demo_events() {
            sender.send(event(level, message, fields)).unwrap();
        }
        let state = State { board: Dashboard::new("course"), events: Arc::new(Mutex::new(receiver)) };
        let env = rune::Env { system_dark: true, scale: 2.0 };
        let mut rt = rune::Runtime::new(state, view, (820.0, 560.0), env);
        rt.poll = Some(poll);
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        let shown = |key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).and_then(|e| e.text);
        assert_eq!(shown("correct-value").as_deref(), Some("3"));
        assert_eq!(shown("incorrect-value").as_deref(), Some("1"));
        assert_eq!(shown("question-value").as_deref(), Some("#4"));
        assert_eq!(shown("question-type").as_deref(), Some("multiple-choice"));
        assert_eq!(shown("mode").as_deref(), Some("course"));
        assert_eq!(shown("status-label").as_deref(), Some("complete"));
        assert!(rt.find("line-9").is_some(), "the newest log line is laid out");
    }

    #[test]
    fn a_full_log_drops_old_rows_and_keeps_the_newest_in_view() {
        let (sender, receiver) = mpsc::channel();
        for i in 0..MAX_LOG_LINES + 50 {
            sender.send(event(Level::Info, &format!("line {i}"), vec![("questionNum", json!(i))])).unwrap();
        }
        let state = State { board: Dashboard::new("unit"), events: Arc::new(Mutex::new(receiver)) };
        let mut rt = rune::Runtime::new(state, view, (820.0, 560.0), rune::Env { system_dark: false, scale: 2.0 });
        rt.poll = Some(poll);
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
}
