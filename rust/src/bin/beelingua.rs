//! Port of `src/cli.js`: `beelingua [mode] [url]`. Launches the browser with a debug port
//! unless one already answers, then attaches and runs the mode, auto-detected when omitted.

use std::io::{BufRead, IsTerminal, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use beelingua::browser::{attach_to_lms, Driver, FramedPage, Lms};
use beelingua::config::{build_config, environment, Config};
use beelingua::detect::{detect_mode, PageMode};
use beelingua::logger;
use beelingua::runners::course::{self, CourseOptions};
use beelingua::runners::unit::{self, UnitPage};
use beelingua::runners::{checkpoint, exercise, iframe, reading, Live, Report};
use serde_json::{json, Value};

const USAGE: &str = "usage: beelingua [course|unit|exercise|iframe|reading|checkpoint] [url]
       beelingua attach | open-row \"<row title>\" | eval-frame \"<script>\"
Env: CDP_PORT (default 9222), BROWSER (chrome|brave|edge), BROWSER_BINARY_PATH, LLM_PROVIDER=codex";

const DEFAULT_URL: &str = "https://lms.binus.ac.id";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunMode {
    Course,
    Unit,
    Exercise,
    Iframe,
    Reading,
    Checkpoint,
}

impl RunMode {
    fn parse(arg: &str) -> Option<RunMode> {
        match arg {
            "course" => Some(RunMode::Course),
            "unit" => Some(RunMode::Unit),
            "exercise" => Some(RunMode::Exercise),
            "iframe" => Some(RunMode::Iframe),
            "reading" => Some(RunMode::Reading),
            "checkpoint" => Some(RunMode::Checkpoint),
            _ => None,
        }
    }

    fn from_detected(mode: PageMode) -> RunMode {
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum Mode {
    /// A runner, or `None` to auto-detect after attaching.
    Run {
        mode: Option<RunMode>,
        url: String,
    },
    Attach,
    /// Dev: open a unit row and pass its gate, navigation only.
    OpenRow {
        title: String,
    },
    /// Dev: evaluate a script in the first iframe.
    EvalFrame {
        script: String,
    },
}

fn parse_args(args: &[String]) -> Result<Mode, String> {
    let first = args.first().map(String::as_str);
    let second = args.get(1).cloned();
    match first {
        Some("attach") => Ok(Mode::Attach),
        Some("open-row") => second.map(|title| Mode::OpenRow { title }).ok_or_else(|| USAGE.to_string()),
        Some("eval-frame") => second.map(|script| Mode::EvalFrame { script }).ok_or_else(|| USAGE.to_string()),
        Some("-h" | "--help" | "help") => Err(USAGE.to_string()),
        Some("final-test") => Err("The final-test runner is not ported to Rust yet.".to_string()),
        Some(arg) => match RunMode::parse(arg) {
            Some(mode) => Ok(Mode::Run { mode: Some(mode), url: second.unwrap_or_else(|| DEFAULT_URL.to_string()) }),
            // Like cli.js, an unknown first argument is the start URL.
            None => Ok(Mode::Run { mode: None, url: arg.to_string() }),
        },
        None => Ok(Mode::Run { mode: None, url: DEFAULT_URL.to_string() }),
    }
}

/// True when something answers `/json/version` on the debug port.
fn cdp_answers(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    // The DevTools server ignores HTTP/1.0 and keeps 1.1 connections open, so read only the first chunk.
    let request = format!("GET /json/version HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut response = [0u8; 64];
    let n = stream.read(&mut response).unwrap_or(0);
    response[..n].starts_with(b"HTTP/1.1 200")
}

/// cli.js resolveBinaryPath: BROWSER_BINARY_PATH (or CHROME_BINARY_PATH), else the platform default.
fn resolve_binary_path(env: &std::collections::HashMap<String, String>) -> Result<String, String> {
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
    let default = match (std::env::consts::OS, browser.as_str()) {
        ("macos", "chrome") => "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        ("macos", "brave") => "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
        ("macos", "edge") => "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        ("windows", "chrome") => r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        ("windows", "brave") => r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe",
        ("windows", "edge") => r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        (os, _) => {
            return Err(format!(
                "No default binary path known for BROWSER={browser} on {os}. Set BROWSER_BINARY_PATH in .env."
            ))
        }
    };
    if !Path::new(default).exists() {
        return Err(format!(
            "BROWSER={browser} defaults to \"{default}\", but it's not installed there. Set BROWSER to a browser you have (chrome/brave/edge) or BROWSER_BINARY_PATH to its location."
        ));
    }
    Ok(default.to_string())
}

/// Starts the browser with the debug port unless one already answers. True when it launched one.
fn ensure_browser(env: &std::collections::HashMap<String, String>, port: u16, url: &str) -> Result<bool, String> {
    if cdp_answers(port) {
        return Ok(false);
    }
    let binary = resolve_binary_path(env)?;
    println!("Launching browser: {binary}");
    let mut command = Command::new(&binary);
    command
        .args([format!("--remote-debugging-port={port}"), url.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    command.spawn().map_err(|e| format!("Failed to launch browser at \"{binary}\": {e}"))?;
    Ok(true)
}

fn wait_for_enter(prompt: &str) {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let _ = std::io::stdin().lock().read_line(&mut String::new());
}

/// Without a TTY to wait on, a freshly launched browser is polled until its port answers.
fn wait_for_port(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cdp_answers(port) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    false
}

async fn run_mode(mode: RunMode, lms: &Lms, config: &Config) -> Result<Vec<(&'static str, Value)>, String> {
    let mut runners = Live { config: config.clone() };
    let fields = match mode {
        RunMode::Course => course::run(lms, &mut runners, CourseOptions::default()).await.map(|o| o.fields()),
        RunMode::Unit => unit::run(lms, &mut runners).await.map(|o| o.fields()),
        RunMode::Exercise => exercise::run(lms, config).await.map(|o| o.fields()),
        RunMode::Iframe => iframe::run(lms, config).await.map(|o| o.fields()),
        RunMode::Reading => reading::run(lms).await.map(|o| o.fields()),
        RunMode::Checkpoint => checkpoint::run(lms, &mut checkpoint::LiveAttempt).await.map(|o| o.fields()),
    };
    fields.map_err(|e| e.to_string())
}

/// Pointer-clicks the row, waits for the route change, then clicks Start or Continue when one mounts.
async fn open_row(lms: &Lms, title: &str) -> Result<(), String> {
    let before = lms.current_url().await.map_err(|e| e.to_string())?;
    if !lms.click_row(title).await.map_err(|e| e.to_string())? {
        return Err(format!("No enabled row titled {title:?} on this page"));
    }
    let Some(url) = lms.wait_for_url_change(&before, Duration::from_secs(10)).await else {
        return Err(format!("Row {title:?} did not open (URL never changed)"));
    };
    println!("opened {url}");
    let gate = lms.click_start_gate(Duration::from_secs(8)).await.map_err(|e| e.to_string())?;
    println!("gate {}", if gate { "clicked" } else { "none" });
    if gate {
        lms.sleep(1500).await;
    }
    Ok(())
}

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = parse_args(&args).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2);
    });
    let env = environment(std::fs::read_to_string(".env").ok().as_deref());
    let config = build_config(&env);
    let port = env.get("CDP_PORT").and_then(|p| p.parse().ok()).unwrap_or(9222);

    if let Mode::Run { url, .. } = &mode {
        let launched = ensure_browser(&env, port, url).unwrap_or_else(|e| fail(&e));
        if std::io::stdin().is_terminal() {
            wait_for_enter("Log in and open the exercise, then press Enter to start the bot... ");
        } else if launched && !wait_for_port(port, Duration::from_secs(30)) {
            fail(&format!("The browser never opened its debug port {port}."));
        }
    }

    let lms = attach_to_lms(port).await.unwrap_or_else(|e| fail(&e.to_string()));
    let url = lms.current_url().await.unwrap_or_default();

    let run = match mode {
        Mode::Attach => {
            println!("attached to {url}");
            return;
        }
        Mode::OpenRow { title } => {
            if let Err(e) = open_row(&lms, &title).await {
                fail(&e);
            }
            return;
        }
        Mode::EvalFrame { script } => {
            match lms.execute_in_frame(&script, &[]).await {
                Ok(value) => println!("{value}"),
                Err(e) => fail(&e.to_string()),
            }
            return;
        }
        Mode::Run { mode: Some(run), .. } => run,
        Mode::Run { mode: None, .. } => {
            println!("Detecting exercise type...");
            let detected = detect_mode(&lms).await.unwrap_or_else(|e| fail(&e.to_string()));
            let Some(detected) = detected else {
                fail("Could not auto-detect the exercise type. Run with an explicit mode instead: beelingua <course|unit|exercise|iframe|reading|checkpoint>");
            };
            println!("Detected: {}", detected.name());
            RunMode::from_detected(detected)
        }
    };

    match run_mode(run, &lms, &config).await {
        Ok(fields) => logger::info("Run finished", fields),
        Err(e) => {
            logger::error("Run failed", [("error", json!(e))]);
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_args_reads_a_mode_and_url_or_treats_an_unknown_first_argument_as_the_url() {
        assert_eq!(parse_args(&args(&[])), Ok(Mode::Run { mode: None, url: DEFAULT_URL.to_string() }));
        assert_eq!(
            parse_args(&args(&["iframe", "https://x"])),
            Ok(Mode::Run { mode: Some(RunMode::Iframe), url: "https://x".to_string() })
        );
        assert_eq!(parse_args(&args(&["https://y"])), Ok(Mode::Run { mode: None, url: "https://y".to_string() }));
        assert_eq!(parse_args(&args(&["open-row", "Reading"])), Ok(Mode::OpenRow { title: "Reading".to_string() }));
        assert!(parse_args(&args(&["open-row"])).is_err());
        assert_eq!(
            parse_args(&args(&["checkpoint"])),
            Ok(Mode::Run { mode: Some(RunMode::Checkpoint), url: DEFAULT_URL.to_string() })
        );
        assert_eq!(RunMode::from_detected(PageMode::Checkpoint), RunMode::Checkpoint);
    }
}
