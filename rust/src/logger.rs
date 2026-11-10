//! Port of the logging half of `src/logger.js`: one event per call, sent to a pluggable sink.
//! The default sink prints `{time, level, message, ...fields}` as one JSON line on stdout.

use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn name(self) -> &'static str {
        match self {
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub time: SystemTime,
    pub level: Level,
    pub message: String,
    /// In call order, so the JSON line reads like the JS one.
    pub fields: Vec<(String, Value)>,
}

impl Event {
    pub fn to_json_line(&self) -> String {
        let mut line = format!(
            "{{\"time\":{},\"level\":{},\"message\":{}",
            Value::from(iso_time(self.time)),
            Value::from(self.level.name()),
            Value::from(self.message.as_str())
        );
        for (key, value) in &self.fields {
            line.push_str(&format!(",{}:{}", Value::from(key.as_str()), value));
        }
        line.push('}');
        line
    }
}

pub trait Sink: Send + Sync {
    fn emit(&self, event: &Event);
}

pub struct StdoutSink;

impl Sink for StdoutSink {
    fn emit(&self, event: &Event) {
        println!("{}", event.to_json_line());
    }
}

static SINK: RwLock<Option<Box<dyn Sink>>> = RwLock::new(None);

/// Routes every later event to `sink`; `None` restores the stdout JSON lines.
pub fn set_sink(sink: Option<Box<dyn Sink>>) {
    *SINK.write().unwrap_or_else(|e| e.into_inner()) = sink;
}

pub fn log<'a>(level: Level, message: &str, fields: impl IntoIterator<Item = (&'a str, Value)>) {
    let event = Event {
        time: SystemTime::now(),
        level,
        message: message.to_string(),
        fields: fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
    };
    match SINK.read().unwrap_or_else(|e| e.into_inner()).as_deref() {
        Some(sink) => sink.emit(&event),
        None => StdoutSink.emit(&event),
    }
}

pub fn info<'a>(message: &str, fields: impl IntoIterator<Item = (&'a str, Value)>) {
    log(Level::Info, message, fields);
}

pub fn warn<'a>(message: &str, fields: impl IntoIterator<Item = (&'a str, Value)>) {
    log(Level::Warn, message, fields);
}

pub fn error<'a>(message: &str, fields: impl IntoIterator<Item = (&'a str, Value)>) {
    log(Level::Error, message, fields);
}

/// `Date.prototype.toISOString`: `2026-11-10T09:30:00.123Z`.
pub fn iso_time(time: SystemTime) -> String {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs();
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        since.subsec_millis()
    )
}

/// Howard Hinnant's days-to-civil algorithm for the proleptic Gregorian calendar.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn iso_time_matches_to_iso_string() {
        assert_eq!(iso_time(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        // new Date(1794303000123).toISOString()
        assert_eq!(iso_time(UNIX_EPOCH + Duration::from_millis(1_794_303_000_123)), "2026-11-10T09:30:00.123Z");
        assert_eq!(iso_time(UNIX_EPOCH + Duration::from_millis(951_782_400_000)), "2000-02-29T00:00:00.000Z");
    }

    #[test]
    fn json_line_spreads_fields_after_time_level_and_message() {
        let event = Event {
            time: UNIX_EPOCH,
            level: Level::Warn,
            message: "Could not click course node — stopping".to_string(),
            fields: vec![("label".to_string(), json!("Unit5")), ("progress".to_string(), json!({ "done": 4 }))],
        };
        assert_eq!(
            event.to_json_line(),
            r#"{"time":"1970-01-01T00:00:00.000Z","level":"warn","message":"Could not click course node — stopping","label":"Unit5","progress":{"done":4}}"#
        );
    }
}
