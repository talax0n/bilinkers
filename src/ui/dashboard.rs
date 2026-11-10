//! The run screen, built only from logger events: a header pill with the run's state, a big
//! "answered" figure, the activities and questions as a step list, and the raw log folded away
//! under Details.

use std::collections::VecDeque;

use rune::{col, el, row, text, Color, Cursor, El, ScrollSpec};
use serde_json::{json, Value};

use crate::logger::{Event, Level};
use crate::ui::style::{
    bar, chip, dot, font, glyph, list_row, live_dot, page, section, space, two_tone, Header, Palette, BANG, CHECK,
    CHEVRON_DOWN, CHEVRON_RIGHT, CROSS, RETRY,
};

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
    /// Stop was pressed, or the window closed, before the run ended.
    Stopped,
}

impl RunStatus {
    pub fn label(&self) -> &str {
        match self {
            RunStatus::Running => "running",
            RunStatus::Finished(status) => status,
            RunStatus::Failed => "error",
            RunStatus::Stopped => "stopped",
        }
    }

    fn succeeded(&self) -> bool {
        matches!(self, RunStatus::Finished(s) if s == "complete" || s == "passed")
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    /// An activity or a course node.
    Activity,
    Question(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Active,
    /// Correct on the first try, or an activity that finished.
    Done,
    /// Correct after this many attempts.
    Retried(u64),
    Incorrect(u64),
    /// A question type the bot can't answer.
    Unhandled,
    /// An activity that ended without finishing.
    Stuck,
    /// Still open when the run stopped or failed.
    Stopped,
}

/// One row of the step list.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub id: u64,
    pub kind: StepKind,
    /// An activity's title; a question's type.
    pub title: String,
    /// An activity's mode or node type.
    pub detail: Option<String>,
    pub state: StepState,
    /// Whether a question sits under an activity.
    pub nested: bool,
}

/// What the window shows, built only from logger events.
#[derive(Debug, Clone, PartialEq)]
pub struct Dashboard {
    pub mode: String,
    pub status: RunStatus,
    pub correct: u32,
    pub incorrect: u32,
    pub steps: Vec<Step>,
    /// The error of "Run failed".
    pub error: Option<String>,
    pub lines: VecDeque<LogLine>,
    next_id: u64,
}

fn field<'a>(event: &'a Event, name: &str) -> Option<&'a Value> {
    event.fields.iter().find(|(k, _)| k == name).map(|(_, v)| v)
}

fn field_str(event: &Event, name: &str) -> Option<String> {
    field(event, name).and_then(Value::as_str).map(str::to_string)
}

impl Dashboard {
    pub fn new(mode: &str) -> Dashboard {
        Dashboard {
            mode: mode.to_string(),
            status: RunStatus::Running,
            correct: 0,
            incorrect: 0,
            steps: Vec::new(),
            error: None,
            lines: VecDeque::new(),
            next_id: 0,
        }
    }

    pub fn answered(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| {
                matches!(s.kind, StepKind::Question(_))
                    && matches!(s.state, StepState::Done | StepState::Retried(_) | StepState::Incorrect(_))
            })
            .count()
    }

    pub fn retried(&self) -> usize {
        self.steps.iter().filter(|s| matches!(s.state, StepState::Retried(_))).count()
    }

    pub fn activities(&self) -> usize {
        self.steps.iter().filter(|s| s.kind == StepKind::Activity).count()
    }

    /// The row the run is on: the newest activity still open.
    pub fn current(&self) -> Option<u64> {
        if self.status != RunStatus::Running {
            return None;
        }
        self.steps.iter().rev().find(|s| s.state == StepState::Active).map(|s| s.id)
    }

    fn push(&mut self, kind: StepKind, title: String, detail: Option<String>, state: StepState) {
        let nested = matches!(kind, StepKind::Question(_)) && self.steps.iter().any(|s| s.kind == StepKind::Activity);
        self.steps.push(Step { id: self.next_id, kind, title, detail, state, nested });
    }

    /// An activity or course node opened: reuses its open row, else adds one.
    fn open(&mut self, title: String, detail: Option<String>) {
        let open = self.steps.iter_mut().rev().find(|s| s.kind == StepKind::Activity && s.title == title);
        match open {
            Some(step) if step.state == StepState::Active => {
                if detail.is_some() {
                    step.detail = detail;
                }
            }
            _ => self.push(StepKind::Activity, title, detail, StepState::Active),
        }
    }

    fn close(&mut self, title: String, detail: Option<String>, state: StepState) {
        let open = self
            .steps
            .iter_mut()
            .rev()
            .find(|s| s.kind == StepKind::Activity && s.title == title && s.state == StepState::Active);
        match open {
            Some(step) => step.state = state,
            None => self.push(StepKind::Activity, title, detail, state),
        }
    }

    /// The question's row in the current activity, added when it is new.
    fn question(&mut self, num: u64, kind: Option<String>, state: StepState) {
        let since = self.steps.iter().rposition(|s| s.kind == StepKind::Activity).map_or(0, |i| i + 1);
        match self.steps[since..].iter_mut().find(|s| s.kind == StepKind::Question(num)) {
            Some(step) => {
                step.state = state;
                if let Some(kind) = kind {
                    step.title = kind;
                }
            }
            None => self.push(StepKind::Question(num), kind.unwrap_or_default(), None, state),
        }
    }

    pub fn apply(&mut self, event: &Event) {
        let status = field_str(event, "status");
        let title = field_str(event, "title").or_else(|| field_str(event, "label"));
        let message = event.message.as_str();
        match message {
            "Question processed" => {
                let num = field(event, "questionNum").and_then(Value::as_u64).unwrap_or(0);
                let attempts = field(event, "attempts").and_then(Value::as_u64).unwrap_or(1);
                let state = match status.as_deref() {
                    Some("correct") if attempts > 1 => StepState::Retried(attempts),
                    Some("correct") => StepState::Done,
                    Some("incorrect") => StepState::Incorrect(attempts),
                    _ => StepState::Unhandled,
                };
                // Activity reports carry statuses too, so only per-question events count toward the tally.
                match status.as_deref() {
                    Some("correct") => self.correct += 1,
                    Some("incorrect") => self.incorrect += 1,
                    _ => {}
                }
                self.question(num, field_str(event, "type"), state);
            }
            "Opening activity" | "Running activity" | "Opening course node" | "Running course node" => {
                if let Some(title) = title {
                    self.open(title, field_str(event, "mode").or_else(|| field_str(event, "type")));
                }
            }
            "Activity finished" | "Course node finished" | "Completed media activity" => {
                if let Some(title) = title {
                    let state = match status.as_deref() {
                        None | Some("complete" | "passed") => StepState::Done,
                        Some(_) => StepState::Stuck,
                    };
                    let detail = field_str(event, "kind").or_else(|| field_str(event, "mode"));
                    self.close(title, detail, state);
                }
            }
            "Run finished" => self.end(RunStatus::Finished(status.unwrap_or_else(|| "complete".into()))),
            "Run failed" => {
                self.error = field_str(event, "error");
                self.end(RunStatus::Failed);
            }
            "Run stopped" => self.end(RunStatus::Stopped),
            "Detected" => {
                if let Some(mode) = field_str(event, "mode") {
                    self.mode = mode;
                }
            }
            _ if message.starts_with("Unhandled question type") => {
                let num = field(event, "questionNum").or_else(|| field(event, "slideNum")).and_then(Value::as_u64);
                if let Some(num) = num {
                    self.question(num, None, StepState::Unhandled);
                }
            }
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

    /// The run ended: what was still open finished with it, or stopped there.
    fn end(&mut self, status: RunStatus) {
        let left = match &status {
            s if s.succeeded() => StepState::Done,
            RunStatus::Finished(_) => StepState::Stuck,
            _ => StepState::Stopped,
        };
        for step in self.steps.iter_mut().filter(|s| s.state == StepState::Active) {
            step.state = left;
        }
        self.status = status;
    }
}

/// Which of the run's screens shows the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Running,
    Stopping,
    Finished,
}

/// "multiple-choice" as "Multiple choice".
fn humanize(kind: &str) -> String {
    let words = kind.replace(['-', '_'], " ");
    let mut chars = words.chars();
    chars.next().map_or_else(String::new, |c| c.to_uppercase().collect::<String>() + chars.as_str())
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn status_color(status: &RunStatus, p: &Palette) -> Color {
    match status {
        RunStatus::Running => p.accent,
        s if s.succeeded() => p.good,
        RunStatus::Finished(_) | RunStatus::Stopped => p.warn,
        RunStatus::Failed => p.bad,
    }
}

fn titles(board: &Dashboard, phase: Phase) -> (String, String) {
    let mode = board.mode.to_lowercase();
    let answered = plural(board.answered(), "question", "questions");
    let across = match board.activities() {
        0 => String::new(),
        n => format!(" across {}", plural(n, "activity", "activities")),
    };
    match (phase, &board.status) {
        (Phase::Running, _) => (
            format!("Working on your {mode}"),
            "Answering in your browser. Leave it open; each step shows here.".into(),
        ),
        (Phase::Stopping, _) => ("Stopping".into(), "Finishing the current step, then the bot stops.".into()),
        (_, s) if s.succeeded() => ("All done".into(), format!("Answered {answered}{across}.")),
        (_, RunStatus::Finished(s)) if s == "unhandled" => (
            "Stuck on a question".into(),
            "The bot can't answer that question type. Answer it in the browser, then run again.".into(),
        ),
        (_, RunStatus::Finished(_)) => (
            "Stopped partway".into(),
            "Some activities could not be finished. Check them in the browser, then run again.".into(),
        ),
        (_, RunStatus::Stopped) => ("Stopped".into(), format!("You stopped the run after {answered}.")),
        (_, _) => (
            "Something went wrong".into(),
            board
                .error
                .clone()
                .unwrap_or_else(|| "The run ended with an error. The log under Details says why.".into()),
        ),
    }
}

fn header_pill(board: &Dashboard, phase: Phase, p: &Palette) -> El {
    let color = status_color(&board.status, p);
    let (lead, label) = match phase {
        Phase::Running => (live_dot(p.accent, p.accent_soft), "Running".to_string()),
        Phase::Stopping => (live_dot(p.warn, p.warn_soft), "Stopping".to_string()),
        Phase::Finished => {
            let label = match &board.status {
                RunStatus::Finished(s) if s == "unhandled" => "Needs you".to_string(),
                RunStatus::Failed => "Error".to_string(),
                status => humanize(status.label()),
            };
            (dot(color), label)
        }
    };
    let mut pill = crate::ui::style::pill("status", p)
        .child(lead)
        .child(text(label).key("status-label").font(font::STRONG).color(p.ink))
        .child(text("\u{b7}").font(font::STRONG).color(p.ink_3))
        .child(text(board.mode.clone()).key("mode").font(font::BODY).color(p.ink_2));
    let tried = board.correct + board.incorrect;
    if tried > 0 {
        let share = board.correct as f32 / tried as f32;
        pill = pill.child(el().w(4.0)).child(bar(56.0, share, p.primary, p)).child(
            text(format!("{} of {tried} correct", board.correct)).key("pill-detail").font(font::SMALL).color(p.ink_2),
        );
    }
    pill
}

fn summary(board: &Dashboard, p: &Palette) -> El {
    let answered = board.answered();
    let note = |key: &str, glyph_icon, color: Color, words: String| {
        row()
            .key(key)
            .gap(3.0)
            .center_y()
            .child(glyph(glyph_icon, 13.0, color))
            .child(text(words).font(font::SMALL_STRONG).color(color))
    };
    let first_try =
        board.steps.iter().filter(|s| matches!(s.kind, StepKind::Question(_)) && s.state == StepState::Done).count();
    let mut notes = row().gap(space::MD).center_y().pb(7.0).child(note(
        "first-try",
        &CHECK,
        p.good,
        format!("{first_try} first try"),
    ));
    if board.retried() > 0 {
        notes = notes.child(note("retried", &RETRY, p.warn, format!("{} retried", board.retried())));
    }
    if board.incorrect > 0 {
        notes = notes.child(note("incorrect", &CROSS, p.bad, format!("{} incorrect", board.incorrect)));
    }
    row()
        .key("summary")
        .w_full()
        .gap(space::LG)
        .items_end()
        .child(two_tone(&answered.to_string(), " answered", font::FIGURE, p.ink, p.ink_3).key("answered-value"))
        .child(notes)
}

fn state_note(step: &Step, p: &Palette) -> El {
    let note = |glyph_icon, color: Color, words: String| {
        row()
            .gap(4.0)
            .center_y()
            .child(glyph(glyph_icon, 14.0, color))
            .child(text(words).key(format!("step-{}-state", step.id)).font(font::SMALL_STRONG).color(color))
    };
    let tries = |n: u64| plural(n as usize, "try", "tries");
    match (step.kind, step.state) {
        (StepKind::Activity, StepState::Active) => row()
            .gap(6.0)
            .center_y()
            .child(text("In progress").key(format!("step-{}-state", step.id)).font(font::SMALL_STRONG).color(p.accent)),
        (StepKind::Activity, StepState::Done) => note(&CHECK, p.good, "Done".into()),
        (_, StepState::Done) => note(&CHECK, p.good, "Correct".into()),
        (_, StepState::Retried(n)) => note(&RETRY, p.warn, format!("Correct after {}", tries(n))),
        (_, StepState::Incorrect(n)) => note(&CROSS, p.bad, format!("Incorrect after {}", tries(n))),
        (_, StepState::Unhandled) => note(&BANG, p.bad, "Can't answer this type".into()),
        (_, StepState::Stuck) => note(&BANG, p.warn, "Not finished".into()),
        (_, StepState::Stopped) | (StepKind::Question(_), StepState::Active) => {
            row().child(text("Stopped").key(format!("step-{}-state", step.id)).font(font::SMALL_STRONG).color(p.ink_3))
        }
    }
}

fn step_row(board: &Dashboard, step: &Step, current: bool, p: &Palette) -> El {
    let key = format!("step-{}", step.id);
    let r = list_row(&key, current, p);
    match step.kind {
        StepKind::Activity => {
            let lead = match step.state {
                StepState::Active if current => live_dot(p.accent, p.accent_soft),
                StepState::Done => el().size(16.0, 16.0).radius(8.0).fill(p.good).center().child(glyph(
                    &CHECK,
                    12.0,
                    p.on_primary_fixed(),
                )),
                StepState::Stuck | StepState::Unhandled => el()
                    .size(16.0, 16.0)
                    .radius(8.0)
                    .fill(p.warn)
                    .center()
                    .child(glyph(&BANG, 12.0, p.on_primary_fixed())),
                _ => el().size(16.0, 16.0).radius(8.0).border(1.5, p.row_border),
            };
            let questions = board
                .steps
                .iter()
                .skip_while(|s| s.id != step.id)
                .skip(1)
                .take_while(|s| s.kind != StepKind::Activity)
                .count();
            let mut title =
                row().grow(1.0).min_w(0.0).gap(space::SM).center_y().child(
                    text(step.title.clone()).key(format!("{key}-title")).font(font::STRONG).color(p.ink).truncate(),
                );
            if let Some(detail) = &step.detail {
                title = title.child(chip(&humanize(detail), p));
            }
            let mut r = r.child(lead).child(title);
            if questions > 0 {
                r = r.child(text(plural(questions, "question", "questions")).font(font::SMALL).color(p.ink_3));
            }
            r.child(el().w(space::SM)).child(state_note(step, p))
        }
        StepKind::Question(num) => {
            let indent = if step.nested { space::NEST - space::MD } else { 0.0 };
            let kind = if step.title.is_empty() { "Question".to_string() } else { humanize(&step.title) };
            let r = r
                .min_h(38.0)
                .child(text(format!("Q{num}")).font(font::NUMBER).color(p.ink_3).w(30.0))
                .child(text(kind).font(font::BODY).color(p.ink).grow(1.0).min_w(0.0).truncate())
                .child(state_note(step, p));
            row().w_full().pl(indent).child(r)
        }
    }
}

fn details(board: &Dashboard, open: bool, p: &Palette) -> El {
    let toggle = row()
        .key("details")
        .role("button")
        .label("Details")
        .expanded(open)
        .w_full()
        .h(40.0)
        .px(space::MD)
        .gap(space::SM)
        .center_y()
        .radius(8.0)
        .focusable(true)
        .focus_ring(2.0, 2.0, p.accent)
        .hover(p.hover)
        .press(p.press)
        .cursor(Cursor::Pointer)
        .on_press_tx(|s: &mut crate::ui::Ui, _| s.details = !s.details)
        .child(glyph(if open { &CHEVRON_DOWN } else { &CHEVRON_RIGHT }, 16.0, p.ink_2))
        .child(text("Details").font(font::STRONG).color(p.ink))
        .child(text("The raw log").font(font::BODY).color(p.ink_3))
        .child(el().grow(1.0))
        .child(text(plural(board.lines.len(), "line", "lines")).font(font::SMALL).color(p.ink_3));
    let mut block = col().w_full().gap(space::SM).child(toggle);
    if open {
        let mut lines = col().w_full().py(8.0);
        for line in &board.lines {
            let (level_color, message_color) = match line.level {
                Level::Info => (p.ink_3, p.ink),
                Level::Warn => (p.warn, p.ink),
                Level::Error => (p.bad, p.bad),
            };
            let mut body = row()
                .grow(1.0)
                .min_w(0.0)
                .gap(8.0)
                .child(text(line.message.clone()).font(font::MONO).color(message_color));
            if !line.meta.is_empty() {
                body = body.child(text(line.meta.clone()).font(font::MONO).color(p.ink_3).grow(1.0).min_w(0.0).wrap());
            }
            lines = lines.child(
                row()
                    .key(format!("line-{}", line.id))
                    .w_full()
                    .px(14.0)
                    .py(2.0)
                    .gap(10.0)
                    .child(text(line.level.name().to_string()).font(font::MONO_LEVEL).color(level_color).w(40.0))
                    .child(body),
            );
        }
        block = block.child(
            col()
                .key("log")
                .w_full()
                .h(260.0)
                .radius(10.0)
                .fill(p.sunken)
                .border(1.0, p.row_border)
                .scroll(ScrollSpec { stick_bottom: true })
                .child(lines),
        );
    }
    block
}

/// The run screen with `actions` (Stop, or Run again) at the header's end.
pub fn view(board: &Dashboard, phase: Phase, actions: Vec<El>, details_open: bool, p: &Palette) -> El {
    let (title, subtitle) = titles(board, phase);
    let current = board.current();
    let mut list = col().key("steps").w_full().gap(2.0);
    for step in &board.steps {
        list = list.child(step_row(board, step, current == Some(step.id), p));
    }
    if board.steps.is_empty() {
        let waiting = if phase == Phase::Finished { "Nothing was answered." } else { "Looking at the page..." };
        list = list.child(
            list_row("step-empty", phase != Phase::Finished, p)
                .child(live_dot(p.accent, p.accent_soft))
                .child(text(waiting).font(font::BODY).color(p.ink_2)),
        );
    }
    let count = match board.activities() {
        0 => plural(board.steps.len(), "question", "questions"),
        n => plural(n, "activity", "activities"),
    };
    let body = col()
        .key("dashboard")
        .gap(space::XL)
        .child(summary(board, p))
        .child(
            col()
                .w_full()
                .gap(space::SM)
                .child(section("steps-head", "Steps", Some(text(count).font(font::SMALL).color(p.ink_3)), p))
                .child(list),
        )
        .child(el().w_full().h(1.0).fill(p.hairline))
        .child(details(board, details_open, p));
    let header = Header { key: "run", title, subtitle, pill: Some(header_pill(board, phase, p)), actions };
    let screen = match phase {
        Phase::Running => "Running",
        Phase::Stopping => "Stopping",
        Phase::Finished => "Finished",
    };
    page(screen, header, body, phase != Phase::Finished, p)
}

/// A fixed unit run for headless renders: a vocabulary exercise with one retried and one
/// incorrect answer, a video, then a grammar exercise, then a finished unit.
pub fn demo_events() -> Vec<DemoEvent> {
    let question = |n: u32, kind: &str, status: &str, attempts: u32| {
        vec![("questionNum", json!(n)), ("type", json!(kind)), ("status", json!(status)), ("attempts", json!(attempts))]
    };
    let activity = |title: &str, mode: &str| vec![("title", json!(title)), ("mode", json!(mode))];
    let finished =
        |title: &str| vec![("title", json!(title)), ("mode", json!("exercise")), ("status", json!("complete"))];
    vec![
        (Level::Info, "Attached to LMS", vec![("url", json!("https://lms.binus.ac.id/beelingua/unit-5"))]),
        (Level::Info, "Detected", vec![("mode", json!("Unit"))]),
        (Level::Info, "Opening activity", vec![("title", json!("Vocabulary: Daily routines"))]),
        (Level::Info, "Running activity", activity("Vocabulary: Daily routines", "exercise")),
        (Level::Info, "Question processed", question(1, "multiple-choice", "correct", 1)),
        (Level::Info, "Question processed", question(2, "fill-in-blank", "correct", 2)),
        (Level::Warn, "Check button not found, retrying", vec![("questionNum", json!(3))]),
        (Level::Info, "Question processed", question(3, "quiz-matching", "incorrect", 3)),
        (Level::Info, "Question processed", question(4, "multiple-choice", "correct", 1)),
        (Level::Info, "Activity finished", finished("Vocabulary: Daily routines")),
        (
            Level::Info,
            "Completed media activity",
            vec![("title", json!("Video: At the station")), ("kind", json!("video"))],
        ),
        (Level::Info, "Opening activity", vec![("title", json!("Grammar: Present simple"))]),
        (Level::Info, "Running activity", activity("Grammar: Present simple", "exercise")),
        (Level::Info, "Question processed", question(1, "multiple-choice", "correct", 1)),
        (Level::Info, "Question processed", question(2, "drag-and-drop", "correct", 1)),
        (Level::Info, "Activity finished", finished("Grammar: Present simple")),
        (Level::Info, "Run finished", vec![("status", json!("complete")), ("total", json!(3))]),
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

    fn states(board: &Dashboard) -> Vec<(StepKind, StepState)> {
        board.steps.iter().map(|s| (s.kind, s.state)).collect()
    }

    #[test]
    fn counts_only_question_processed_statuses() {
        let board = board_after(&demo_events());
        assert_eq!((board.correct, board.incorrect), (5, 1), "the activities' own complete statuses are not a tally");
        assert_eq!(board.status, RunStatus::Finished("complete".to_string()));
        assert_eq!(board.lines.len(), demo_events().len());
        assert_eq!(board.lines[2].meta, r#"{"title":"Vocabulary: Daily routines"}"#);
        assert_eq!(board.lines[0].meta, r#"{"url":"https://lms.binus.ac.id/beelingua/unit-5"}"#);
    }

    #[test]
    fn activities_and_their_questions_become_one_step_list() {
        let board = board_after(&demo_events());
        use StepKind::{Activity, Question};
        use StepState::*;
        assert_eq!(
            states(&board),
            vec![
                (Activity, Done),
                (Question(1), Done),
                (Question(2), Retried(2)),
                (Question(3), Incorrect(3)),
                (Question(4), Done),
                (Activity, Done),
                (Activity, Done),
                (Question(1), Done),
                (Question(2), Done),
            ]
        );
        assert_eq!(board.steps[0].detail.as_deref(), Some("exercise"), "Running activity adds the mode to its row");
        assert_eq!(board.steps[5].detail.as_deref(), Some("video"));
        assert!(board.steps[1].nested && !board.steps[0].nested);
        assert_eq!((board.answered(), board.retried(), board.activities()), (6, 1, 3));
        assert_eq!(board.current(), None, "a finished run has no current row");
    }

    #[test]
    fn the_open_activity_is_current_while_running() {
        let events = demo_events();
        let board = board_after(&events[..14]);
        let current = board.current().and_then(|id| board.steps.iter().find(|s| s.id == id));
        assert_eq!(current.map(|s| s.title.as_str()), Some("Grammar: Present simple"));
        assert_eq!(
            board.steps.iter().filter(|s| s.kind == StepKind::Activity).count(),
            3,
            "opening then running is one row"
        );
    }

    #[test]
    fn questions_without_activities_are_not_nested_and_unhandled_ones_show() {
        let board = board_after(&[
            (
                Level::Info,
                "Question processed",
                vec![
                    ("questionNum", json!(1)),
                    ("type", json!("cloze")),
                    ("status", json!("correct")),
                    ("attempts", json!(1)),
                ],
            ),
            (Level::Warn, "Unhandled question type: stopping", vec![("questionNum", json!(2))]),
            (Level::Info, "Run finished", vec![("status", json!("unhandled"))]),
        ]);
        assert_eq!(
            states(&board),
            vec![(StepKind::Question(1), StepState::Done), (StepKind::Question(2), StepState::Unhandled)]
        );
        assert!(!board.steps[0].nested);
    }

    #[test]
    fn a_run_that_ends_early_marks_what_was_open() {
        let open = [
            (Level::Info, "Running course node", vec![("label", json!("Unit 5")), ("type", json!("unit"))]),
            (Level::Info, "Running activity", vec![("title", json!("Reading 1")), ("mode", json!("reading"))]),
        ];
        let mut stuck = open.to_vec();
        stuck.push((Level::Info, "Run finished", vec![("status", json!("stuck"))]));
        assert!(board_after(&stuck).steps.iter().all(|s| s.state == StepState::Stuck));
        let mut stopped = open.to_vec();
        stopped.push((Level::Warn, "Run stopped", vec![]));
        assert!(board_after(&stopped).steps.iter().all(|s| s.state == StepState::Stopped));
        let mut failed = open.to_vec();
        failed.push((Level::Error, "Run failed", vec![("error", json!("No LMS tab"))]));
        let failed = board_after(&failed);
        assert_eq!(failed.error.as_deref(), Some("No LMS tab"));
        let finished_node = [
            open[0].clone(),
            (Level::Info, "Course node finished", vec![("label", json!("Unit 5")), ("status", json!("stuck"))]),
        ];
        assert_eq!(states(&board_after(&finished_node)), vec![(StepKind::Activity, StepState::Stuck)]);
    }

    #[test]
    fn run_finished_and_run_failed_set_the_status() {
        let stuck = board_after(&[(Level::Info, "Run finished", vec![("status", json!("stuck"))])]);
        assert_eq!(stuck.status.label(), "stuck");
        let failed = board_after(&[(Level::Error, "Run failed", vec![("error", json!("boom"))])]);
        assert_eq!(failed.status.label(), "error");
        assert_eq!(Dashboard::new("unit").status.label(), "running");
        let stopped = board_after(&[(Level::Warn, "Run stopped", vec![])]);
        assert_eq!(stopped.status, RunStatus::Stopped);
        let detected = board_after(&[(Level::Info, "Detected", vec![("mode", json!("reading"))])]);
        assert_eq!(detected.mode, "reading");
    }

    #[test]
    fn question_types_read_as_words() {
        assert_eq!(humanize("multiple-choice"), "Multiple choice");
        assert_eq!(humanize("quiz_matching"), "Quiz matching");
        assert_eq!(humanize(""), "");
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
}
