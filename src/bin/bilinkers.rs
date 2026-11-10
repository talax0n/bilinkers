//! Port of `src/cli.js`: `bilinkers [mode] [url]`. Launches the browser with a debug port
//! unless one already answers, then attaches and runs the mode, auto-detected when omitted.

use std::collections::HashMap;
use std::io::{BufRead, IsTerminal, Write};
use std::time::Duration;

use bilinkers::browser::{attach_to_lms, Driver, FramedPage, Lms};
use bilinkers::config::build_config;
use bilinkers::detect::detect_mode;
use bilinkers::runners::unit::UnitPage;
use bilinkers::session::{ensure_browser, run_and_report, wait_for_port, Launch, RunMode, DETECT_FAILED};
use bilinkers::setup::{settings_path, EnvLayers};

const VERSION: &str = concat!("bilinkers ", env!("CARGO_PKG_VERSION"), ", made by ", env!("CARGO_PKG_AUTHORS"));

const USAGE: &str =
    "usage: bilinkers [--no-ui] [island|course|unit|exercise|iframe|reading|checkpoint|final-test] [url]
       bilinkers attach | open-row \"<row title>\" | eval-frame \"<script>\" | ui-demo <rune flags>
       bilinkers --version
Bilinkers is made by TX25-2.
Env: CDP_PORT (default 9222), BROWSER (chrome|brave|edge|chromium), BROWSER_BINARY_PATH, LLM_PROVIDER=codex
Settings are read from the environment, then ./.env, then the window's settings.env.
On a terminal with a display a run opens a window that signs in, picks the browser and starts
on the page you open; --no-ui or NO_TUI=1 keeps the terminal prompts and prints JSON lines.
island, run on the Beelingua home, checks B1.1 to C2.2 and runs each unfinished course in order.
final-test always runs in the terminal.
final-test is read-only unless FINAL_TEST_AUTO_ANSWER=1 or FINAL_TEST_GUESS_LISTENING=1
(FINAL_TEST_CONFIDENCE_THRESHOLD, FINAL_TEST_REVIEWS_REQUIRED). It never clicks Submit.";

const DEFAULT_URL: &str = "https://lms.binus.ac.id";

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
    /// Prints the version and the credit.
    Version,
    /// Dev: render the dashboard headless from a fixed run, passing the rest to Rune.
    UiDemo {
        rune_args: Vec<String>,
    },
}

fn parse_args(args: &[String]) -> Result<Mode, String> {
    let first = args.first().map(String::as_str);
    let second = args.get(1).cloned();
    match first {
        Some("attach") => Ok(Mode::Attach),
        Some("open-row") => second.map(|title| Mode::OpenRow { title }).ok_or_else(|| USAGE.to_string()),
        Some("eval-frame") => second.map(|script| Mode::EvalFrame { script }).ok_or_else(|| USAGE.to_string()),
        Some("ui-demo") => Ok(Mode::UiDemo { rune_args: args[1..].to_vec() }),
        Some("-h" | "--help" | "help") => Err(USAGE.to_string()),
        Some("-V" | "--version") => Ok(Mode::Version),
        Some(arg) => match RunMode::parse(arg) {
            Some(mode) => Ok(Mode::Run { mode: Some(mode), url: second.unwrap_or_else(|| DEFAULT_URL.to_string()) }),
            // Like cli.js, an unknown first argument is the start URL.
            None => Ok(Mode::Run { mode: None, url: arg.to_string() }),
        },
        None => Ok(Mode::Run { mode: None, url: DEFAULT_URL.to_string() }),
    }
}

/// Starts the browser with the debug port unless one already answers. True when it launched one.
fn launch_browser(env: &HashMap<String, String>, port: u16, url: &str) -> Result<bool, String> {
    let launch = ensure_browser(env, port, url)?;
    if let Launch::Launched { binary } = &launch {
        println!("Launching browser: {binary}");
    }
    Ok(launch != Launch::Reused)
}

fn wait_for_enter(prompt: &str) {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let _ = std::io::stdin().lock().read_line(&mut String::new());
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

/// Removes every `flag` from `args`; true when there was one.
fn take_flag(args: &mut Vec<String>, flag: &str) -> bool {
    let before = args.len();
    args.retain(|a| a != flag);
    args.len() != before
}

/// Like cli.js: the dashboard needs a terminal on stdout and no NO_TUI. The window also needs a display.
fn wants_ui(no_ui: bool, env: &HashMap<String, String>, stdout_is_terminal: bool, display: bool) -> bool {
    !no_ui && env.get("NO_TUI").is_none_or(|v| v.is_empty()) && stdout_is_terminal && display
}

#[cfg(feature = "ui")]
fn display_available() -> bool {
    bilinkers::ui::display_available()
}

#[cfg(not(feature = "ui"))]
fn display_available() -> bool {
    false
}

/// Attaches and resolves the run mode; dev modes finish here and return None.
async fn prepare(mode: Mode, env: &HashMap<String, String>, port: u16) -> Option<(Lms, RunMode)> {
    if let Mode::Run { url, .. } = &mode {
        let launched = launch_browser(env, port, url).unwrap_or_else(|e| fail(&e));
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
            return None;
        }
        Mode::OpenRow { title } => {
            if let Err(e) = open_row(&lms, &title).await {
                fail(&e);
            }
            return None;
        }
        Mode::EvalFrame { script } => {
            match lms.execute_in_frame(&script, &[]).await {
                Ok(value) => println!("{value}"),
                Err(e) => fail(&e.to_string()),
            }
            return None;
        }
        Mode::UiDemo { .. } | Mode::Version => return None,
        Mode::Run { mode: Some(run), .. } => run,
        Mode::Run { mode: None, .. } => {
            println!("Detecting exercise type...");
            let detected = detect_mode(&lms).await.unwrap_or_else(|e| fail(&e.to_string()));
            let Some(detected) = detected else {
                fail(DETECT_FAILED);
            };
            println!("Detected: {}", detected.name());
            RunMode::from_detected(detected)
        }
    };
    Some((lms, run))
}

#[cfg(feature = "ui")]
mod window {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::process::Child;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use bilinkers::config::Config;
    use bilinkers::session::{
        ensure_browser, BrowserSetup, Connection, RunHandle, RunMode, Sighting, Watcher, STOPPED,
    };
    use bilinkers::setup::{
        codex_status, save_settings, search_this_machine, spawn_codex_login, BrowserPick, CodexStatus, EnvLayers,
    };
    use bilinkers::ui::{BrowserStep, Host, StartInfo};

    /// The window's way to Codex, the browser and the bot: everything slow runs on other threads.
    struct LiveHost {
        runtime: tokio::runtime::Handle,
        port: u16,
        url: String,
        settings: Option<PathBuf>,
        /// Reread when the browser choice changes.
        env: Mutex<HashMap<String, String>>,
        config: Config,
        run: Mutex<Runs>,
        watcher: Mutex<Watcher>,
        login: Mutex<Option<Child>>,
    }

    #[derive(Default)]
    struct Runs {
        current: Option<RunHandle>,
        last_code: i32,
    }

    impl Runs {
        fn reap(&mut self) {
            if let Some(run) = self.current.take() {
                self.last_code = run.join();
            }
        }
    }

    fn browser_step(layers: &EnvLayers) -> BrowserStep {
        let installed = search_this_machine(None);
        BrowserStep::new(installed, BrowserSetup::from_env(&layers.merged()), layers.browser_locked_by())
    }

    impl LiveHost {
        fn reload(&self) -> BrowserStep {
            let layers = EnvLayers::load(self.settings.as_deref());
            *self.env.lock().unwrap_or_else(|e| e.into_inner()) = layers.merged();
            browser_step(&layers)
        }

        fn kill_login(&self) {
            if let Some(mut child) = self.login.lock().unwrap_or_else(|e| e.into_inner()).take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    impl Host for LiveHost {
        fn codex_status(&self) -> CodexStatus {
            codex_status()
        }

        fn install_codex(&self) -> Result<(), String> {
            let installed = bilinkers::install::install_here().map_err(|e| e.to_string())?;
            bilinkers::logger::info(
                "Installed Codex",
                [("path", installed.path.display().to_string().into()), ("tag", installed.tag.into())],
            );
            Ok(())
        }

        fn sign_in(&self) -> Result<(), String> {
            let child = spawn_codex_login().map_err(|e| format!("Codex could not start: {e}"))?;
            *self.login.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);
            loop {
                std::thread::sleep(Duration::from_millis(200));
                let mut login = self.login.lock().unwrap_or_else(|e| e.into_inner());
                let Some(child) = login.as_mut() else {
                    return Err("Sign-in was cancelled.".into());
                };
                match child.try_wait() {
                    Ok(None) => continue,
                    Ok(Some(status)) => {
                        login.take();
                        return if status.success() { Ok(()) } else { Err(format!("Codex exited with {status}.")) };
                    }
                    Err(e) => {
                        login.take();
                        return Err(e.to_string());
                    }
                }
            }
        }

        fn browsers(&self) -> BrowserStep {
            self.reload()
        }

        fn save_browser(&self, pick: &BrowserPick) -> Result<BrowserStep, String> {
            let path = self.settings.as_deref().ok_or("No settings folder is known for this computer.")?;
            let lines = pick.settings();
            let updates: Vec<(&str, Option<&str>)> = lines.iter().map(|(k, v)| (*k, v.as_deref())).collect();
            save_settings(path, &updates).map_err(|e| format!("Could not save to {}: {e}", path.display()))?;
            Ok(self.reload())
        }

        fn probe(&self) -> Connection {
            let mut watcher = self.watcher.lock().unwrap_or_else(|e| e.into_inner());
            self.runtime.block_on(watcher.connection(self.port))
        }

        fn open_browser(&self) -> Result<(), String> {
            let env = self.env.lock().unwrap_or_else(|e| e.into_inner()).clone();
            ensure_browser(&env, self.port, &self.url).map(|_| ())
        }

        fn watch(&self, classify: bool) -> Sighting {
            let mut watcher = self.watcher.lock().unwrap_or_else(|e| e.into_inner());
            self.runtime.block_on(watcher.look(self.port, classify))
        }

        fn start(&self, mode: RunMode) {
            let mut runs = self.run.lock().unwrap_or_else(|e| e.into_inner());
            runs.reap();
            let config = self.config.clone();
            let env = self.env.lock().unwrap_or_else(|e| e.into_inner()).clone();
            runs.current = Some(RunHandle::spawn(self.runtime.clone(), self.port, Some(mode), config, env));
        }

        fn stop(&self) {
            if let Some(run) = self.run.lock().unwrap_or_else(|e| e.into_inner()).current.as_mut() {
                run.stop();
            }
        }
    }

    /// Runs the window until it closes; the last run's exit code, or why no window opened.
    pub fn run(
        runtime: &tokio::runtime::Runtime,
        preset: Option<RunMode>,
        url: &str,
        settings: Option<PathBuf>,
        config: Config,
        port: u16,
    ) -> Result<i32, String> {
        let layers = EnvLayers::load(settings.as_deref());
        // Checked before the window opens, so it opens on the right screen.
        let info = StartInfo { codex: codex_status(), browser: browser_step(&layers), preset };
        let host = Arc::new(LiveHost {
            runtime: runtime.handle().clone(),
            port,
            url: url.to_string(),
            settings,
            env: Mutex::new(layers.merged()),
            config,
            run: Mutex::new(Runs::default()),
            watcher: Mutex::new(Watcher::default()),
            login: Mutex::new(None),
        });
        let result = bilinkers::ui::run(info, host.clone());
        host.kill_login();
        result?;
        let mut runs = host.run.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(run) = runs.current.as_mut().filter(|run| !run.is_finished()) {
            eprintln!("Window closed; stopping the run.");
            run.stop();
            runs.reap();
            return Ok(STOPPED);
        }
        runs.reap();
        Ok(runs.last_code)
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let no_ui = take_flag(&mut args, "--no-ui");
    let mode = parse_args(&args).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2);
    });
    if mode == Mode::Version {
        println!("{VERSION}");
        return;
    }
    #[cfg(feature = "ui")]
    if let Mode::UiDemo { rune_args } = mode {
        bilinkers::ui::render_headless(rune_args);
        return;
    }
    #[cfg(not(feature = "ui"))]
    if let Mode::UiDemo { .. } = mode {
        fail("ui-demo needs the ui feature");
    }
    let settings = settings_path();
    let env = EnvLayers::load(settings.as_deref()).merged();
    let config = build_config(&env);
    let port = env.get("CDP_PORT").and_then(|p| p.parse().ok()).unwrap_or(9222);

    let runtime = tokio::runtime::Runtime::new().unwrap_or_else(|e| fail(&format!("Could not start tokio: {e}")));
    let final_test = matches!(mode, Mode::Run { mode: Some(RunMode::FinalTest), .. });
    let ui = !final_test && wants_ui(no_ui, &env, std::io::stdout().is_terminal(), display_available());
    #[cfg(feature = "ui")]
    if let (true, Mode::Run { mode: preset, url }) = (ui, &mode) {
        match window::run(&runtime, *preset, url, settings.clone(), config.clone(), port) {
            Ok(code) => std::process::exit(code),
            Err(e) => eprintln!("No window ({e}); using the terminal instead."),
        }
    }
    #[cfg(not(feature = "ui"))]
    let _ = ui;
    let Some((lms, run)) = runtime.block_on(prepare(mode, &env, port)) else {
        return;
    };
    let code = runtime.block_on(run_and_report(run, &lms, &config, &env));
    std::process::exit(code);
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
        assert_eq!(
            parse_args(&args(&["ui-demo", "--snap", "a.png"])),
            Ok(Mode::UiDemo { rune_args: args(&["--snap", "a.png"]) })
        );
        assert_eq!(parse_args(&args(&["--version"])), Ok(Mode::Version));
        assert_eq!(VERSION, format!("bilinkers {}, made by TX25-2", env!("CARGO_PKG_VERSION")));
        assert!(USAGE.contains("Bilinkers is made by TX25-2."));
        assert_eq!(
            parse_args(&args(&["final-test"])),
            Ok(Mode::Run { mode: Some(RunMode::FinalTest), url: DEFAULT_URL.to_string() })
        );
    }

    #[test]
    fn the_ui_opens_only_on_a_terminal_with_a_display_and_without_an_opt_out() {
        let mut args = args(&["--no-ui", "course", "--no-ui"]);
        assert!(take_flag(&mut args, "--no-ui"));
        assert_eq!(args, vec!["course".to_string()]);

        let env = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let none = env(&[]);
        assert!(wants_ui(false, &none, true, true));
        assert!(!wants_ui(true, &none, true, true), "--no-ui");
        assert!(!wants_ui(false, &env(&[("NO_TUI", "1")]), true, true), "NO_TUI=1");
        assert!(wants_ui(false, &env(&[("NO_TUI", "")]), true, true), "an empty NO_TUI is unset, as in JS");
        assert!(!wants_ui(false, &none, false, true), "piped stdout keeps JSON lines");
        assert!(!wants_ui(false, &none, true, false), "no display");
    }
}
