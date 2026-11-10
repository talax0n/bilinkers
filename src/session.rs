//! Everything around a run that both the terminal and the window need: the run modes, the
//! browser from `.env`, a read-only probe of the debug port, and a run on its own thread that
//! reports only through the logger.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::sync::oneshot;

use crate::browser::{attach_to_lms, Driver, Lms, LMS_HOST};
use crate::config::Config;
use crate::detect::{classify_page, detect_mode, PageKind, PageMode};
use crate::logger;
use crate::runners::course::{self, CourseOptions};
use crate::runners::{checkpoint, exercise, final_test_runner, iframe, reading, unit, Live, Report};
use crate::setup::{search_this_machine, BrowserKind};

/// Exit code 130, as for Ctrl-C, when a run is stopped before it ends.
pub const STOPPED: i32 = 130;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    Course,
    Unit,
    Exercise,
    Iframe,
    Reading,
    Checkpoint,
    FinalTest,
}

impl RunMode {
    pub const ALL: [RunMode; 7] = [
        RunMode::Course,
        RunMode::Unit,
        RunMode::Exercise,
        RunMode::Iframe,
        RunMode::Reading,
        RunMode::Checkpoint,
        RunMode::FinalTest,
    ];

    pub fn parse(arg: &str) -> Option<RunMode> {
        RunMode::ALL.into_iter().find(|mode| mode.name() == arg)
    }

    /// The CLI argument.
    pub fn name(self) -> &'static str {
        match self {
            RunMode::Course => "course",
            RunMode::Unit => "unit",
            RunMode::Exercise => "exercise",
            RunMode::Iframe => "iframe",
            RunMode::Reading => "reading",
            RunMode::Checkpoint => "checkpoint",
            RunMode::FinalTest => "final-test",
        }
    }

    /// The name a person reads.
    pub fn label(self) -> &'static str {
        match self {
            RunMode::Course => "Course",
            RunMode::Unit => "Unit",
            RunMode::Exercise => "Exercise",
            RunMode::Iframe => "Activity",
            RunMode::Reading => "Reading",
            RunMode::Checkpoint => "Checkpoint",
            RunMode::FinalTest => "Final test",
        }
    }

    pub fn from_detected(mode: PageMode) -> RunMode {
        match mode {
            PageMode::Course => RunMode::Course,
            PageMode::Unit => RunMode::Unit,
            PageMode::Exercise => RunMode::Exercise,
            PageMode::Iframe => RunMode::Iframe,
            PageMode::Reading => RunMode::Reading,
            PageMode::Checkpoint => RunMode::Checkpoint,
        }
    }
}

pub const DETECT_FAILED: &str = "Could not auto-detect the exercise type. Run with an explicit mode instead: beelingua <course|unit|exercise|iframe|reading|checkpoint>";

/// What `.env` selects: BROWSER (default chrome) and where its binary is, or why it isn't there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserSetup {
    pub name: String,
    pub binary: Result<String, String>,
}

impl BrowserSetup {
    pub fn from_env(env: &HashMap<String, String>) -> BrowserSetup {
        let name = env.get("BROWSER").filter(|b| !b.is_empty()).map_or("chrome".to_string(), |b| b.to_lowercase());
        BrowserSetup { name, binary: resolve_binary_path(env) }
    }

    /// The name a person reads: "Brave", or a picked program's file name.
    pub fn display_name(&self) -> String {
        if let Some(kind) = BrowserKind::parse(&self.name) {
            return kind.label().to_string();
        }
        let file = self.binary.as_ref().ok().and_then(|b| Path::new(b).file_stem()).map(|f| f.to_string_lossy());
        file.map_or_else(|| self.name.clone(), |f| f.into_owned())
    }
}

/// cli.js resolveBinaryPath: BROWSER_BINARY_PATH (or CHROME_BINARY_PATH), else where the search finds BROWSER.
pub fn resolve_binary_path(env: &HashMap<String, String>) -> Result<String, String> {
    let explicit = env.get("BROWSER_BINARY_PATH").or_else(|| env.get("CHROME_BINARY_PATH")).filter(|p| !p.is_empty());
    if let Some(path) = explicit {
        if !Path::new(path).exists() {
            return Err(format!(
                "BROWSER_BINARY_PATH (or CHROME_BINARY_PATH) is set to \"{path}\", but nothing exists there. Double check the path in .env."
            ));
        }
        return Ok(path.clone());
    }
    let browser = env.get("BROWSER").filter(|b| !b.is_empty()).map_or("chrome".to_string(), |b| b.to_lowercase());
    let os = std::env::consts::OS;
    let Some(kind) = BrowserKind::parse(&browser) else {
        return Err(format!(
            "BROWSER={browser} is not a browser the bot knows (chrome/brave/edge/chromium). Set BROWSER_BINARY_PATH in .env."
        ));
    };
    match search_this_machine(Some(kind)).pop() {
        Some(found) => Ok(found.path.display().to_string()),
        None => Err(format!(
            "BROWSER={browser}: searched {} and did not find {}. Set BROWSER to a browser you have (chrome/brave/edge/chromium) or BROWSER_BINARY_PATH to its location.",
            kind.searched(os),
            kind.label()
        )),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// Something already answered on the debug port.
    Reused,
    Launched {
        binary: String,
    },
}

/// Starts the browser with the debug port at `url` unless one already answers.
pub fn ensure_browser(env: &HashMap<String, String>, port: u16, url: &str) -> Result<Launch, String> {
    if cdp_answers(port) {
        return Ok(Launch::Reused);
    }
    let binary = resolve_binary_path(env)?;
    let mut command = Command::new(&binary);
    command
        .args([format!("--remote-debugging-port={port}"), url.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    command.spawn().map_err(|e| format!("Failed to launch browser at \"{binary}\": {e}"))?;
    Ok(Launch::Launched { binary })
}

/// One GET on the debug port: the status line's code and the body, None when nothing answers.
fn http_get(port: u16, path: &str) -> Option<(u16, Vec<u8>)> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(500)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let request = format!("GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    let mut data = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = stream.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
        // The DevTools server may keep the connection open, so stop once Content-Length is in.
        if let Some((head, body)) = split_response(&data) {
            if content_length(head).is_some_and(|len| body.len() >= len) {
                break;
            }
        }
    }
    let (head, body) = split_response(&data)?;
    let status = std::str::from_utf8(head).ok()?.split_whitespace().nth(1)?.parse().ok()?;
    let body = match content_length(head) {
        Some(len) => body[..len.min(body.len())].to_vec(),
        None => body.to_vec(),
    };
    Some((status, body))
}

fn split_response(data: &[u8]) -> Option<(&[u8], &[u8])> {
    let end = data.windows(4).position(|w| w == b"\r\n\r\n")?;
    Some((&data[..end], &data[end + 4..]))
}

fn content_length(head: &[u8]) -> Option<usize> {
    std::str::from_utf8(head).ok()?.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim().eq_ignore_ascii_case("content-length").then(|| value.trim().parse().ok())?
    })
}

/// True when something answers `/json/version` on the debug port.
pub fn cdp_answers(port: u16) -> bool {
    http_get(port, "/json/version").is_some_and(|(status, _)| status == 200)
}

/// Without a TTY to wait on, a freshly launched browser is polled until its port answers.
pub fn wait_for_port(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cdp_answers(port) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    false
}

/// What the debug port shows right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connection {
    /// No probe has answered yet.
    Checking,
    NotRunning,
    NoLmsTab,
    Lms {
        title: String,
        url: String,
    },
}

impl Connection {
    /// Reads the port's tab list (`/json/list`, which only lists), like attach_to_lms picks a tab.
    pub fn probe(port: u16) -> Connection {
        match http_get(port, "/json/list") {
            None => Connection::NotRunning,
            Some((_, body)) => Connection::from_targets(&serde_json::from_slice(&body).unwrap_or(Value::Null)),
        }
    }

    pub fn from_targets(targets: &Value) -> Connection {
        let pages = targets.as_array().into_iter().flatten().filter(|t| t["type"] == "page");
        let text = |t: &Value, key: &str| t[key].as_str().unwrap_or_default().to_string();
        pages
            .map(|t| (text(t, "title"), text(t, "url")))
            .find(|(_, url)| url.contains(LMS_HOST))
            .map_or(Connection::NoLmsTab, |(title, url)| Connection::Lms { title, url })
    }

    pub fn has_lms_tab(&self) -> bool {
        matches!(self, Connection::Lms { .. })
    }
}

/// One look at the browser while the window waits to start: the tab list, then what the LMS
/// tab shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sighting {
    NotRunning,
    NoLmsTab,
    Lms {
        title: String,
        url: String,
        /// None when it was not looked at, or could not be read yet (the page is loading).
        page: Option<PageKind>,
    },
}

/// Watches the LMS tab over one CDP session, reattaching only when that session stops
/// answering. Every look is read-only.
#[derive(Default)]
pub struct Watcher {
    lms: Option<Lms>,
}

const LOOK_TIMEOUT: Duration = Duration::from_secs(5);

impl Watcher {
    /// Lists the tabs (blocking, so run it off the runtime's workers) and, when `classify` and
    /// an LMS tab is open, reads what it shows. A page that can start a run drops the session,
    /// since the run attaches its own.
    pub async fn look(&mut self, port: u16, classify: bool) -> Sighting {
        let (title, url) = match Connection::probe(port) {
            Connection::Lms { title, url } => (title, url),
            other => {
                self.lms = None;
                return if other == Connection::NoLmsTab { Sighting::NoLmsTab } else { Sighting::NotRunning };
            }
        };
        if !classify {
            return Sighting::Lms { title, url, page: None };
        }
        let page = tokio::time::timeout(LOOK_TIMEOUT, self.classify(port)).await.ok().flatten();
        if page.is_none() || matches!(page, Some(PageKind::Mode(_))) {
            self.lms = None;
        }
        Sighting::Lms { title, url, page }
    }

    async fn classify(&mut self, port: u16) -> Option<PageKind> {
        let alive = match &self.lms {
            Some(lms) => lms.current_url().await.is_ok_and(|url| url.contains(LMS_HOST)),
            None => false,
        };
        if !alive {
            self.lms = Some(attach_to_lms(port).await.ok()?);
        }
        classify_page(self.lms.as_ref()?).await.ok()
    }
}

pub async fn run_mode(
    mode: RunMode,
    lms: &Lms,
    config: &Config,
    env: &HashMap<String, String>,
) -> Result<Vec<(&'static str, Value)>, String> {
    let mut runners = Live { config: config.clone() };
    let fields = match mode {
        RunMode::Course => course::run(lms, &mut runners, CourseOptions::default()).await.map(|o| o.fields()),
        RunMode::Unit => unit::run(lms, &mut runners).await.map(|o| o.fields()),
        RunMode::Exercise => exercise::run(lms, config).await.map(|o| o.fields()),
        RunMode::Iframe => iframe::run(lms, config).await.map(|o| o.fields()),
        RunMode::Reading => reading::run(lms).await.map(|o| o.fields()),
        RunMode::Checkpoint => checkpoint::run(lms, &mut checkpoint::LiveAttempt).await.map(|o| o.fields()),
        RunMode::FinalTest => final_test_runner::run(lms, config, env).await.map(|o| o.fields()),
    };
    fields.map_err(|e| e.to_string())
}

/// Runs `mode` and logs "Run finished" or "Run failed"; the exit code.
pub async fn run_and_report(mode: RunMode, lms: &Lms, config: &Config, env: &HashMap<String, String>) -> i32 {
    match run_mode(mode, lms, config, env).await {
        Ok(fields) => {
            logger::info("Run finished", fields);
            0
        }
        Err(e) => {
            fail(&e);
            1
        }
    }
}

fn fail(error: &str) {
    logger::error("Run failed", [("error", json!(error))]);
}

/// Attaches, detects the mode when `mode` is None, runs it, and logs every step.
async fn attach_and_run(port: u16, mode: Option<RunMode>, config: &Config, env: &HashMap<String, String>) -> i32 {
    let lms = match attach_to_lms(port).await {
        Ok(lms) => lms,
        Err(e) => {
            fail(&e.to_string());
            return 1;
        }
    };
    logger::info("Attached to LMS", [("url", json!(lms.current_url().await.unwrap_or_default()))]);
    let mode = match mode {
        Some(mode) => mode,
        None => match detect_mode(&lms).await {
            Ok(Some(detected)) => {
                let mode = RunMode::from_detected(detected);
                logger::info("Detected", [("mode", json!(mode.name()))]);
                mode
            }
            Ok(None) => {
                fail(DETECT_FAILED);
                return 1;
            }
            Err(e) => {
                fail(&e.to_string());
                return 1;
            }
        },
    };
    run_and_report(mode, &lms, config, env).await
}

/// A run on its own thread. It always ends with one "Run finished", "Run failed" or "Run stopped".
pub struct RunHandle {
    stop: Option<oneshot::Sender<()>>,
    thread: JoinHandle<i32>,
}

impl RunHandle {
    pub fn spawn(
        runtime: tokio::runtime::Handle,
        port: u16,
        mode: Option<RunMode>,
        config: Config,
        env: HashMap<String, String>,
    ) -> RunHandle {
        let (stop, stopped) = oneshot::channel::<()>();
        let thread = std::thread::spawn(move || {
            runtime.block_on(async move {
                tokio::select! {
                    code = attach_and_run(port, mode, &config, &env) => code,
                    Ok(()) = stopped => {
                        logger::warn("Run stopped", []);
                        STOPPED
                    }
                }
            })
        });
        RunHandle { stop: Some(stop), thread }
    }

    /// Asks the run to stop; dropping its future cancels it at the next await.
    pub fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }

    pub fn is_finished(&self) -> bool {
        self.thread.is_finished()
    }

    pub fn join(self) -> i32 {
        self.thread.join().unwrap_or(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_mode_names_parse_back_to_the_same_mode() {
        for name in ["course", "unit", "exercise", "iframe", "reading", "checkpoint", "final-test"] {
            assert_eq!(RunMode::parse(name).map(RunMode::name), Some(name));
        }
        assert_eq!(RunMode::from_detected(PageMode::Checkpoint), RunMode::Checkpoint);
        assert_eq!(RunMode::FinalTest.label(), "Final test");
        assert_eq!(RunMode::Iframe.label(), "Activity", "no jargon in the window");
    }

    #[test]
    fn the_first_lms_page_target_is_the_lms_tab() {
        let targets = json!([
            { "type": "service_worker", "title": "sw", "url": "https://lms.binus.ac.id/sw.js" },
            { "type": "page", "title": "New Tab", "url": "chrome://newtab/" },
            { "type": "page", "title": "Unit 5", "url": "https://lms.binus.ac.id/course/5" },
            { "type": "page", "title": "Other", "url": "https://lms.binus.ac.id/course/6" },
        ]);
        assert_eq!(
            Connection::from_targets(&targets),
            Connection::Lms { title: "Unit 5".into(), url: "https://lms.binus.ac.id/course/5".into() }
        );
        assert_eq!(Connection::from_targets(&json!([{ "type": "page", "url": "about:blank" }])), Connection::NoLmsTab);
        assert_eq!(
            Connection::from_targets(&Value::Null),
            Connection::NoLmsTab,
            "an unreadable list still means running"
        );
    }

    #[test]
    fn a_response_is_cut_at_its_content_length() {
        let data = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\ncontent-length: 2\r\n\r\n[]extra";
        let (head, body) = split_response(data).unwrap();
        assert_eq!(content_length(head), Some(2));
        assert_eq!(&body[..2], b"[]");
        assert_eq!(split_response(b"HTTP/1.1 200 OK\r\n"), None);
    }

    #[test]
    fn the_browser_defaults_to_chrome_and_reports_a_missing_override() {
        let env: HashMap<String, String> =
            [("BROWSER_BINARY_PATH".to_string(), "/nonexistent/browser".to_string())].into_iter().collect();
        let setup = BrowserSetup::from_env(&env);
        assert_eq!(setup.name, "chrome");
        assert!(setup.binary.unwrap_err().contains("/nonexistent/browser"));
        let brave: HashMap<String, String> = [("BROWSER".to_string(), "Brave".to_string())].into_iter().collect();
        assert_eq!(BrowserSetup::from_env(&brave).name, "brave");
        assert_eq!(BrowserSetup::from_env(&brave).display_name(), "Brave");
        let custom =
            BrowserSetup { name: "custom".into(), binary: Ok("/opt/Chromium.app/Contents/MacOS/Chromium".into()) };
        assert_eq!(custom.display_name(), "Chromium");
    }
}
