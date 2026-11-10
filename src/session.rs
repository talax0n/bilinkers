//! Everything around a run that both the terminal and the window need: the run modes, the
//! browser from `.env`, a read-only probe of the debug port, and a run on its own thread that
//! reports only through the logger.

use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::sync::oneshot;

use crate::browser::{attach_to_lms, open_lms, Driver, Lms};
use crate::config::Config;
use crate::detect::{classify_page, detect_mode, PageKind, PageMode};
use crate::devtools::{http_get, page_tabs, ListedTab};
use crate::logger;
use crate::runners::course::{self, CourseOptions};
use crate::runners::island::{self, IslandOptions};
use crate::runners::{checkpoint, exercise, final_test_runner, iframe, reading, unit, Live, Report};
use crate::setup::{search_this_machine, BrowserKind};

/// Exit code 130, as for Ctrl-C, when a run is stopped before it ends.
pub const STOPPED: i32 = 130;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    Island,
    Course,
    Unit,
    Exercise,
    Iframe,
    Reading,
    Checkpoint,
    FinalTest,
}

impl RunMode {
    pub const ALL: [RunMode; 8] = [
        RunMode::Island,
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
            RunMode::Island => "island",
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
            RunMode::Island => "All courses",
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
            PageMode::Island => RunMode::Island,
            PageMode::Course => RunMode::Course,
            PageMode::Unit => RunMode::Unit,
            PageMode::Exercise => RunMode::Exercise,
            PageMode::Iframe => RunMode::Iframe,
            PageMode::Reading => RunMode::Reading,
            PageMode::Checkpoint => RunMode::Checkpoint,
        }
    }
}

pub const DETECT_FAILED: &str = "Could not auto-detect the exercise type. Run with an explicit mode instead: bilinkers <island|course|unit|exercise|iframe|reading|checkpoint>";

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
    /// The LMS tab when exactly one is open, from the tab list alone (`/json/list` only lists).
    /// With several, `Watcher::connection` asks the tabs which one is visible.
    pub fn probe(port: u16) -> Connection {
        match page_tabs(port) {
            None => Connection::NotRunning,
            Some(tabs) => Connection::from_tabs(&tabs),
        }
    }

    /// The first listed LMS tab, else NoLmsTab.
    pub fn from_tabs(tabs: &[ListedTab]) -> Connection {
        tabs.iter().find(|t| t.is_lms()).map_or(Connection::NoLmsTab, Connection::of)
    }

    fn of(tab: &ListedTab) -> Connection {
        Connection::Lms { title: tab.title.clone(), url: tab.url.clone() }
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
/// answering. Every look is read-only and never brings a tab to front.
#[derive(Default)]
pub struct Watcher {
    lms: Option<Lms>,
}

const LOOK_TIMEOUT: Duration = Duration::from_secs(5);

impl Watcher {
    /// The LMS tab a run would pick. With one LMS tab the tab list says it; with several, the
    /// tabs are asked over this watcher's session, opened once and reused.
    pub async fn connection(&mut self, port: u16) -> Connection {
        let Some(tabs) = page_tabs(port) else {
            self.lms = None;
            return Connection::NotRunning;
        };
        if tabs.iter().filter(|t| t.is_lms()).count() < 2 {
            return Connection::from_tabs(&tabs);
        }
        match tokio::time::timeout(LOOK_TIMEOUT, self.pick(port, &tabs)).await.ok().flatten() {
            Some(tab) => Connection::of(&tab),
            None => Connection::from_tabs(&tabs),
        }
    }

    /// Picks over the session, opening one when there is none or it stopped answering.
    async fn pick(&mut self, port: u16, tabs: &[ListedTab]) -> Option<ListedTab> {
        if let Some(lms) = self.lms.as_mut() {
            if lms.repick(tabs).await.is_ok_and(|why| why.is_some()) {
                return Some(lms.tab.clone());
            }
        }
        self.lms = None;
        let lms = open_lms(port).await.ok()?;
        let tab = lms.tab.clone();
        self.lms = Some(lms);
        Some(tab)
    }

    /// Lists the tabs (blocking, so run it off the runtime's workers) and, when `classify` and
    /// an LMS tab is open, reads what the picked tab shows. A page that can start a run drops
    /// the session, since the run attaches its own.
    pub async fn look(&mut self, port: u16, classify: bool) -> Sighting {
        if !classify {
            return match self.connection(port).await {
                Connection::Lms { title, url } => Sighting::Lms { title, url, page: None },
                Connection::NoLmsTab => Sighting::NoLmsTab,
                Connection::NotRunning | Connection::Checking => Sighting::NotRunning,
            };
        }
        let Some(tabs) = page_tabs(port) else {
            self.lms = None;
            return Sighting::NotRunning;
        };
        let Some(first) = tabs.iter().find(|t| t.is_lms()) else {
            self.lms = None;
            return Sighting::NoLmsTab;
        };
        let looked = tokio::time::timeout(LOOK_TIMEOUT, async {
            let tab = self.pick(port, &tabs).await?;
            let page = classify_page(self.lms.as_ref()?).await.ok();
            Some((tab, page))
        })
        .await
        .ok()
        .flatten();
        let (tab, page) = looked.unwrap_or_else(|| (first.clone(), None));
        if page.is_none() || matches!(page, Some(PageKind::Mode(_))) {
            self.lms = None;
        }
        Sighting::Lms { title: tab.title, url: tab.url, page }
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
        RunMode::Island => island::run(lms, &mut runners, IslandOptions::default()).await.map(|o| o.fields()),
        RunMode::Course => course::run(lms, &mut runners, CourseOptions::default()).await.map(|o| o.fields()),
        RunMode::Unit => unit::run(lms, &mut runners).await.map(|o| o.fields()),
        RunMode::Exercise => exercise::run(lms, config).await.map(|o| o.fields()),
        RunMode::Iframe => iframe::run(lms, config).await.map(|o| o.fields()),
        RunMode::Reading => reading::run(lms).await.map(|o| o.fields()),
        RunMode::Checkpoint => checkpoint::run(lms, &mut checkpoint::LiveAttempt).await.map(|o| o.fields()),
        RunMode::FinalTest => final_test_runner::run(lms, config, env).await.map(|o| o.fields()),
    };
    let mut fields = fields.map_err(|e| e.to_string())?;
    // The window words a finished run by its mode, e.g. a unit that had nothing left to do.
    fields.push(("mode", json!(mode.name())));
    Ok(fields)
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
        for name in ["island", "course", "unit", "exercise", "iframe", "reading", "checkpoint", "final-test"] {
            assert_eq!(RunMode::parse(name).map(RunMode::name), Some(name));
        }
        assert_eq!(RunMode::from_detected(PageMode::Checkpoint), RunMode::Checkpoint);
        assert_eq!(RunMode::from_detected(PageMode::Island), RunMode::Island);
        assert_eq!(RunMode::FinalTest.label(), "Final test");
        assert_eq!(RunMode::Iframe.label(), "Activity", "no jargon in the window");
    }

    #[test]
    fn without_asking_the_tabs_the_first_listed_lms_tab_is_the_lms_tab() {
        let tab = |id: &str, url: &str| ListedTab { id: id.into(), title: id.into(), url: url.into() };
        let tabs = [
            tab("New Tab", "chrome://newtab/"),
            tab("Unit 5", "https://lms.binus.ac.id/course/5"),
            tab("Other", "https://lms.binus.ac.id/course/6"),
        ];
        assert_eq!(
            Connection::from_tabs(&tabs),
            Connection::Lms { title: "Unit 5".into(), url: "https://lms.binus.ac.id/course/5".into() }
        );
        assert_eq!(Connection::from_tabs(&tabs[..1]), Connection::NoLmsTab);
        assert_eq!(Connection::from_tabs(&[]), Connection::NoLmsTab, "an unreadable list still means running");
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
