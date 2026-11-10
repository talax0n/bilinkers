//! The run dashboard, the Rune port of `src/dashboard.js`: the mode and run status, the
//! correct and incorrect tallies, the current question and a scrolling log, all built from
//! logger events.

use std::collections::VecDeque;

use rune::tok::{self, type_};
use rune::{col, el, row, text, Color, El, ScrollSpec};
use serde_json::{json, Value};

use crate::logger::{Event, Level};
use crate::ui::style::{card, dot, section, Tones, FIGURE, HEADING, MONO, MONO_LEVEL, SUBHEADING};

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
            "Run stopped" => self.status = RunStatus::Stopped,
            "Detected" => {
                if let Some(mode) = field("mode").and_then(Value::as_str) {
                    self.mode = mode.to_string();
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
}

fn status_color(status: &RunStatus, tones: &Tones) -> Color {
    match status {
        RunStatus::Running => tones.accent,
        RunStatus::Finished(s) if s == "complete" || s == "passed" => tones.good,
        RunStatus::Finished(_) | RunStatus::Stopped => tones.warn,
        RunStatus::Failed => tones.bad,
    }
}

fn stat(key: &str, label: &str, value: El, t: &tok::Theme) -> El {
    card(key, t).grow(1.0).gap(2.0).child(section(label, t)).child(value)
}

fn figure(key: &str, value: String, color: Color) -> El {
    text(value).key(format!("{key}-value")).font(FIGURE).color(color)
}

/// The run's mode and status, its tallies and its log, with `action` (Stop or New run) at the
/// header's end.
pub fn view(board: &Dashboard, action: El, t: &tok::Theme, tones: &Tones) -> El {
    let status_color = status_color(&board.status, tones);
    let status = row()
        .key("status")
        .gap(6.0)
        .center_y()
        .child(dot(status_color))
        .child(text(board.status.label().to_string()).key("status-label").font(type_::LABEL).color(status_color));
    let header = row()
        .key("header")
        .w_full()
        .center_y()
        .gap(8.0)
        .child(text("Bilinkers").font(HEADING).color(t.text))
        .child(text(board.mode.clone()).key("mode").font(SUBHEADING).color(t.text_secondary))
        .child(el().grow(1.0))
        .child(status)
        .child(el().w(4.0))
        .child(action);

    let number = board.question.map_or("none yet".to_string(), |n| format!("#{n}"));
    let mut question = row().gap(10.0).center_y().child(figure("question", number, t.text));
    if let Some(kind) = &board.question_type {
        question = question.child(text(kind.clone()).key("question-type").font(type_::BODY).color(t.text_secondary));
    }
    let stats = row()
        .w_full()
        .gap(10.0)
        .child(stat("correct", "Correct", figure("correct", board.correct.to_string(), tones.good), t))
        .child(stat("incorrect", "Incorrect", figure("incorrect", board.incorrect.to_string(), tones.bad), t))
        .child(stat("question", "Question", question, t).grow(2.0));

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

    col().key("dashboard").w_full().h_full().gap(12.0).child(header).child(stats).child(log)
}

/// A fixed run for headless renders: three correct answers, one incorrect, a few info and
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
        let stopped = board_after(&[(Level::Warn, "Run stopped", vec![])]);
        assert_eq!(stopped.status, RunStatus::Stopped);
        let detected = board_after(&[(Level::Info, "Detected", vec![("mode", json!("reading"))])]);
        assert_eq!(detected.mode, "reading");
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
