//! What the welcome screen sets up: the Codex sign-in, which supported browsers are installed,
//! and the settings file the browser choice is saved to. Paths come from std alone: each
//! platform's config dir is one environment variable away, so a crate would add nothing.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use crate::config::parse_dotenv;

/// What `codex login status` says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexStatus {
    /// The CLI's own line, such as "Logged in using ChatGPT".
    SignedIn(String),
    SignedOut,
    /// No `codex` program on PATH or in the usual install places.
    Missing,
    /// It ran but said something unexpected, or could not start.
    Failed(String),
}

impl CodexStatus {
    pub fn signed_in(&self) -> bool {
        matches!(self, CodexStatus::SignedIn(_))
    }

    /// Reads a finished `codex login status`: exit 0 is signed in, exit 1 signed out.
    pub fn from_output(result: std::io::Result<Output>) -> CodexStatus {
        let output = match result {
            Ok(output) => output,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return CodexStatus::Missing,
            Err(e) => return CodexStatus::Failed(e.to_string()),
        };
        let first_line = |bytes: &[u8]| {
            String::from_utf8_lossy(bytes).lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string)
        };
        // Codex 0.167 prints the status on stderr, older builds on stdout.
        let line = first_line(&output.stdout).or_else(|| first_line(&output.stderr));
        match output.status.code() {
            Some(0) if line.as_deref().is_some_and(|l| l.to_ascii_lowercase().contains("not logged in")) => {
                CodexStatus::SignedOut
            }
            Some(0) => CodexStatus::SignedIn(line.unwrap_or_else(|| "Logged in".to_string())),
            Some(1) => CodexStatus::SignedOut,
            code => CodexStatus::Failed(line.unwrap_or_else(|| format!("codex exited with {code:?}"))),
        }
    }
}

/// Where `codex` is: the first hit on `path_var`, then the usual install places a Finder or
/// Start menu launch leaves off PATH.
pub fn find_codex(path_var: Option<&OsStr>, home: Option<&Path>, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) { &["codex.exe", "codex.cmd"] } else { &["codex"] };
    let mut dirs: Vec<PathBuf> = path_var.map(|p| std::env::split_paths(p).collect()).unwrap_or_default();
    if cfg!(windows) {
        dirs.extend(std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("npm")));
    } else {
        dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
        if let Some(home) = home {
            dirs.extend([".local/bin", ".npm-global/bin", ".volta/bin", ".bun/bin"].map(|d| home.join(d)));
        }
    }
    dirs.iter().flat_map(|dir| names.iter().map(move |name| dir.join(name))).find(|path| exists(path))
}

/// The program to run for `codex` and a PATH that also has its folder, so an npm install finds
/// the `node` next to it.
pub fn codex_program() -> (PathBuf, OsString) {
    let path_var = std::env::var_os("PATH");
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let program = find_codex(path_var.as_deref(), home.as_deref(), Path::is_file);
    let mut dirs: Vec<PathBuf> = path_var.as_deref().map(|p| std::env::split_paths(p).collect()).unwrap_or_default();
    if let Some(dir) = program.as_deref().and_then(Path::parent) {
        if !dirs.iter().any(|d| d == dir) {
            dirs.insert(0, dir.to_path_buf());
        }
    }
    let path = std::env::join_paths(dirs).unwrap_or_else(|_| path_var.unwrap_or_default());
    (program.unwrap_or_else(|| PathBuf::from("codex")), path)
}

fn codex_command(args: &[&str]) -> Command {
    let (program, path) = codex_program();
    let mut command = Command::new(program);
    command.args(args).env("PATH", path).stdin(Stdio::null());
    command
}

/// Runs `codex login status`. Never reads Codex's own credential files.
pub fn codex_status() -> CodexStatus {
    CodexStatus::from_output(codex_command(&["login", "status"]).output())
}

/// Starts `codex login`, which opens the system browser for the ChatGPT sign-in and exits when
/// it is done; the caller waits on (or kills) the child.
pub fn spawn_codex_login() -> std::io::Result<std::process::Child> {
    codex_command(&["login"]).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
}

/// A browser the bot can drive, by its BROWSER key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BrowserKind {
    Chrome,
    Brave,
    Edge,
}

impl BrowserKind {
    /// Chrome first, the order choices are shown and preselected in.
    pub const ALL: [BrowserKind; 3] = [BrowserKind::Chrome, BrowserKind::Brave, BrowserKind::Edge];

    pub fn key(self) -> &'static str {
        match self {
            BrowserKind::Chrome => "chrome",
            BrowserKind::Brave => "brave",
            BrowserKind::Edge => "edge",
        }
    }

    pub fn parse(key: &str) -> Option<BrowserKind> {
        BrowserKind::ALL.into_iter().find(|kind| kind.key().eq_ignore_ascii_case(key))
    }

    pub fn label(self) -> &'static str {
        match self {
            BrowserKind::Chrome => "Google Chrome",
            BrowserKind::Brave => "Brave",
            BrowserKind::Edge => "Microsoft Edge",
        }
    }

    /// Where the installer puts it on `os` (`std::env::consts::OS`).
    pub fn default_path(self, os: &str) -> Option<&'static str> {
        Some(match (os, self) {
            ("macos", BrowserKind::Chrome) => "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            ("macos", BrowserKind::Brave) => "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
            ("macos", BrowserKind::Edge) => "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            ("windows", BrowserKind::Chrome) => r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            ("windows", BrowserKind::Brave) => r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe",
            ("windows", BrowserKind::Edge) => r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
            _ => return None,
        })
    }
}

/// The browsers installed at their default paths on `os`, Chrome first.
pub fn installed_browsers(os: &str, exists: impl Fn(&Path) -> bool) -> Vec<BrowserKind> {
    BrowserKind::ALL.into_iter().filter(|kind| kind.default_path(os).is_some_and(|p| exists(Path::new(p)))).collect()
}

/// The executable inside a picked macOS app bundle, or the picked file itself.
pub fn browser_executable(picked: &Path) -> PathBuf {
    if picked.extension().is_some_and(|e| e == "app") {
        let macos = picked.join("Contents").join("MacOS");
        let named = picked.file_stem().map(|stem| macos.join(stem));
        if let Some(named) = named.filter(|p| p.is_file()) {
            return named;
        }
        let files: Vec<PathBuf> = std::fs::read_dir(&macos)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|p| p.is_file())
            .collect();
        if let [only] = files.as_slice() {
            return only.clone();
        }
    }
    picked.to_path_buf()
}

pub const SETTINGS_FILE: &str = "settings.env";

/// The settings file in the platform config dir: Application Support on macOS, %APPDATA% on
/// Windows, $XDG_CONFIG_HOME (else ~/.config) elsewhere.
pub fn settings_path_for(os: &str, var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let set = |name: &str| var(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    let dir = match os {
        "macos" => set("HOME")?.join("Library").join("Application Support").join("Beelingua"),
        "windows" => set("APPDATA")?.join("Beelingua"),
        _ => set("XDG_CONFIG_HOME").or_else(|| set("HOME").map(|h| h.join(".config")))?.join("beelingua"),
    };
    Some(dir.join(SETTINGS_FILE))
}

pub fn settings_path() -> Option<PathBuf> {
    settings_path_for(std::env::consts::OS, |name| std::env::var_os(name))
}

/// The file's entries; a missing or unreadable file has none.
pub fn read_settings(path: &Path) -> Vec<(String, String)> {
    std::fs::read_to_string(path).map(|text| parse_dotenv(&text)).unwrap_or_default()
}

fn quote(value: &str) -> String {
    if value.is_empty() || value.chars().all(|c| c.is_ascii_alphanumeric() || "/\\:._-".contains(c)) {
        value.to_string()
    } else {
        format!("\"{value}\"")
    }
}

/// Sets (Some) or removes (None) keys and keeps every other entry. The new file is written
/// next to the old one and renamed over it, so a crash never leaves half a file.
pub fn save_settings(path: &Path, updates: &[(&str, Option<&str>)]) -> std::io::Result<()> {
    let mut entries = read_settings(path);
    for (key, value) in updates {
        entries.retain(|(k, _)| k != key);
        if let Some(value) = value {
            entries.push((key.to_string(), value.to_string()));
        }
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut text = String::from("# Saved by the Beelingua Bot window. .env and the environment win over this file.\n");
    for (key, value) in &entries {
        text.push_str(&format!("{key}={}\n", quote(value)));
    }
    let tmp = dir.join(format!(".{SETTINGS_FILE}.{}.tmp", std::process::id()));
    let written = std::fs::File::create(&tmp).and_then(|mut file| {
        file.write_all(text.as_bytes())?;
        file.sync_all()
    });
    match written.and_then(|()| std::fs::rename(&tmp, path)) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// What the welcome screen saves for the browser step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserPick {
    Installed(BrowserKind),
    /// A browser picked with Choose file, as its executable.
    File(PathBuf),
}

impl BrowserPick {
    /// The settings.env lines it becomes.
    pub fn settings(&self) -> [(&'static str, Option<String>); 2] {
        match self {
            BrowserPick::Installed(kind) => [("BROWSER", Some(kind.key().to_string())), ("BROWSER_BINARY_PATH", None)],
            BrowserPick::File(path) => {
                [("BROWSER", Some("custom".to_string())), ("BROWSER_BINARY_PATH", Some(path.display().to_string()))]
            }
        }
    }
}

/// Where a setting came from, first one that sets it wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Process,
    DotEnv,
    Settings,
}

/// The three places settings come from, in the order they win.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvLayers {
    pub process: HashMap<String, String>,
    pub dotenv: Vec<(String, String)>,
    pub settings: Vec<(String, String)>,
}

impl EnvLayers {
    /// The process environment, `./.env` and the settings file at `settings`.
    pub fn load(settings: Option<&Path>) -> EnvLayers {
        EnvLayers {
            process: std::env::vars().collect(),
            dotenv: std::fs::read_to_string(".env").map(|t| parse_dotenv(&t)).unwrap_or_default(),
            settings: settings.map(read_settings).unwrap_or_default(),
        }
    }

    /// One map: a key keeps the first non-empty value, so `BROWSER_BINARY_PATH=` in `.env`
    /// does not hide a saved path. A key set only to empty values stays, empty.
    pub fn merged(&self) -> HashMap<String, String> {
        let mut env = self.process.clone();
        for (key, value) in self.dotenv.iter().chain(&self.settings) {
            let slot = env.entry(key.clone()).or_default();
            if slot.is_empty() {
                *slot = value.clone();
            }
        }
        env
    }

    /// Which layer gives `key` its value.
    pub fn origin(&self, key: &str) -> Option<Origin> {
        let sets = |pairs: &[(String, String)]| pairs.iter().any(|(k, v)| k == key && !v.is_empty());
        if self.process.get(key).is_some_and(|v| !v.is_empty()) {
            Some(Origin::Process)
        } else if sets(&self.dotenv) {
            Some(Origin::DotEnv)
        } else if sets(&self.settings) {
            Some(Origin::Settings)
        } else {
            None
        }
    }

    /// Where the browser is chosen when it is not the settings file, which the window can't change.
    pub fn browser_locked_by(&self) -> Option<Origin> {
        ["BROWSER_BINARY_PATH", "CHROME_BINARY_PATH", "BROWSER"]
            .iter()
            .filter_map(|key| self.origin(key))
            .find(|origin| *origin != Origin::Settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::ExitStatus;

    #[cfg(unix)]
    fn exit(code: i32) -> ExitStatus {
        std::os::unix::process::ExitStatusExt::from_raw(code << 8)
    }

    #[cfg(windows)]
    fn exit(code: i32) -> ExitStatus {
        std::os::windows::process::ExitStatusExt::from_raw(code as u32)
    }

    fn output(code: i32, stdout: &str, stderr: &str) -> std::io::Result<Output> {
        Ok(Output { status: exit(code), stdout: stdout.as_bytes().to_vec(), stderr: stderr.as_bytes().to_vec() })
    }

    #[test]
    fn codex_status_follows_the_exit_code_and_reads_either_stream() {
        assert_eq!(
            CodexStatus::from_output(output(0, "", "Logged in using ChatGPT\n")),
            CodexStatus::SignedIn("Logged in using ChatGPT".into()),
            "codex 0.167 writes the status to stderr"
        );
        assert_eq!(
            CodexStatus::from_output(output(0, "\nLogged in using an API key - sk-...\n", "")),
            CodexStatus::SignedIn("Logged in using an API key - sk-...".into())
        );
        assert_eq!(CodexStatus::from_output(output(1, "", "Not logged in\n")), CodexStatus::SignedOut);
        assert_eq!(CodexStatus::from_output(output(0, "Not logged in", "")), CodexStatus::SignedOut);
        assert_eq!(
            CodexStatus::from_output(output(2, "", "error: unknown flag")),
            CodexStatus::Failed("error: unknown flag".into())
        );
        assert!(CodexStatus::from_output(output(0, "", "")).signed_in());
    }

    #[test]
    fn a_missing_codex_binary_is_missing_not_signed_out() {
        let missing = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        assert_eq!(CodexStatus::from_output(Err(missing)), CodexStatus::Missing);
        let spawned = Command::new("/nonexistent/beelingua-test/codex").arg("login").output();
        assert_eq!(CodexStatus::from_output(spawned), CodexStatus::Missing);
        let denied = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        assert_eq!(CodexStatus::from_output(Err(denied)), CodexStatus::Failed("denied".into()));
    }

    #[cfg(unix)]
    #[test]
    fn codex_is_found_on_path_first_then_in_the_usual_install_places() {
        let path = OsString::from("/a/bin:/b/bin");
        let at = |hit: &'static str| move |p: &Path| p == Path::new(hit);
        assert_eq!(find_codex(Some(&path), None, at("/b/bin/codex")), Some(PathBuf::from("/b/bin/codex")));
        assert_eq!(
            find_codex(None, None, at("/opt/homebrew/bin/codex")),
            Some(PathBuf::from("/opt/homebrew/bin/codex"))
        );
        let home = Path::new("/home/u");
        assert_eq!(find_codex(None, Some(home), at("/home/u/.local/bin/codex")), Some(home.join(".local/bin/codex")));
        assert_eq!(find_codex(Some(&path), Some(home), |_| false), None);
    }

    #[test]
    fn installed_browsers_are_the_ones_at_their_default_paths_chrome_first() {
        let brave = BrowserKind::Brave.default_path("macos").unwrap();
        assert_eq!(installed_browsers("macos", |p| p == Path::new(brave)), vec![BrowserKind::Brave]);
        assert_eq!(installed_browsers("macos", |_| true), BrowserKind::ALL.to_vec());
        assert_eq!(installed_browsers("macos", |_| false), vec![]);
        assert_eq!(installed_browsers("linux", |_| true), vec![], "no default paths on Linux");
        let edge = BrowserKind::Edge.default_path("windows").unwrap();
        assert!(edge.ends_with("msedge.exe"));
        assert_eq!(installed_browsers("windows", |p| p == Path::new(edge)), vec![BrowserKind::Edge]);
        assert_eq!(BrowserKind::parse("Brave"), Some(BrowserKind::Brave));
        assert_eq!(BrowserKind::parse("custom"), None);
    }

    #[test]
    fn the_settings_file_lives_in_each_platforms_config_dir() {
        let vars = |pairs: &'static [(&str, &str)]| {
            move |name: &str| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| OsString::from(v))
        };
        assert_eq!(
            settings_path_for("macos", vars(&[("HOME", "/Users/u")])),
            Some(PathBuf::from("/Users/u/Library/Application Support/Beelingua/settings.env"))
        );
        assert_eq!(
            settings_path_for("windows", vars(&[("APPDATA", r"C:\Users\u\AppData\Roaming")])),
            Some(PathBuf::from(r"C:\Users\u\AppData\Roaming").join("Beelingua").join("settings.env"))
        );
        assert_eq!(
            settings_path_for("linux", vars(&[("XDG_CONFIG_HOME", "/x"), ("HOME", "/home/u")])),
            Some(PathBuf::from("/x/beelingua/settings.env"))
        );
        assert_eq!(
            settings_path_for("linux", vars(&[("XDG_CONFIG_HOME", ""), ("HOME", "/home/u")])),
            Some(PathBuf::from("/home/u/.config/beelingua/settings.env"))
        );
        assert_eq!(settings_path_for("macos", vars(&[])), None);
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn the_environment_wins_then_dotenv_then_the_settings_file() {
        let layers = EnvLayers {
            process: pairs(&[("CDP_PORT", "9333"), ("NO_TUI", "")]).into_iter().collect(),
            dotenv: pairs(&[("BROWSER", "chrome"), ("CDP_PORT", "1"), ("BROWSER_BINARY_PATH", "")]),
            settings: pairs(&[("BROWSER", "brave"), ("BROWSER_BINARY_PATH", "/x/brave"), ("ONLY_SAVED", "1")]),
        };
        let env = layers.merged();
        assert_eq!(env["CDP_PORT"], "9333");
        assert_eq!(env["BROWSER"], "chrome", ".env beats the settings file");
        assert_eq!(env["BROWSER_BINARY_PATH"], "/x/brave", "an empty .env value sets nothing");
        assert_eq!(env["ONLY_SAVED"], "1");
        assert_eq!(env["NO_TUI"], "", "an empty-only key stays empty");
        assert_eq!(layers.origin("BROWSER"), Some(Origin::DotEnv));
        assert_eq!(layers.origin("BROWSER_BINARY_PATH"), Some(Origin::Settings));
        assert_eq!(layers.origin("CDP_PORT"), Some(Origin::Process));
        assert_eq!(layers.origin("MISSING"), None);
        assert_eq!(layers.browser_locked_by(), Some(Origin::DotEnv));
        let saved_only = EnvLayers { settings: pairs(&[("BROWSER", "brave")]), ..EnvLayers::default() };
        assert_eq!(saved_only.browser_locked_by(), None);
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("beelingua-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn saving_settings_creates_the_dir_replaces_keys_and_keeps_the_rest() {
        let dir = temp_dir("settings");
        let path = dir.join("nested").join(SETTINGS_FILE);
        save_settings(&path, &[("BROWSER", Some("chrome")), ("OTHER", Some("kept"))]).unwrap();
        let path_with_space = "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser";
        let pick = BrowserPick::File(PathBuf::from(path_with_space));
        let [(k1, v1), (k2, v2)] = pick.settings();
        save_settings(&path, &[(k1, v1.as_deref()), (k2, v2.as_deref())]).unwrap();
        assert_eq!(
            read_settings(&path),
            pairs(&[("OTHER", "kept"), ("BROWSER", "custom"), ("BROWSER_BINARY_PATH", path_with_space)])
        );
        let [(k1, v1), (k2, v2)] = BrowserPick::Installed(BrowserKind::Edge).settings();
        save_settings(&path, &[(k1, v1.as_deref()), (k2, v2.as_deref())]).unwrap();
        assert_eq!(read_settings(&path), pairs(&[("OTHER", "kept"), ("BROWSER", "edge")]));
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(leftovers, vec![SETTINGS_FILE.to_string()], "no temp file is left behind");
        assert_eq!(read_settings(&dir.join("absent.env")), vec![]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_save_keeps_the_old_file_whole() {
        let dir = temp_dir("settings-fail");
        let path = dir.join(SETTINGS_FILE);
        save_settings(&path, &[("BROWSER", Some("brave"))]).unwrap();
        // A directory where the temp file would go makes the write fail before the rename.
        let tmp = dir.join(format!(".{SETTINGS_FILE}.{}.tmp", std::process::id()));
        std::fs::create_dir(&tmp).unwrap();
        assert!(save_settings(&path, &[("BROWSER", Some("edge"))]).is_err());
        assert_eq!(read_settings(&path), pairs(&[("BROWSER", "brave")]));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_picked_app_bundle_becomes_its_executable() {
        let dir = temp_dir("bundle");
        let macos = dir.join("Chromium.app").join("Contents").join("MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::write(macos.join("Chromium"), "").unwrap();
        assert_eq!(browser_executable(&dir.join("Chromium.app")), macos.join("Chromium"));
        assert_eq!(browser_executable(Path::new("/usr/bin/chromium")), PathBuf::from("/usr/bin/chromium"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Read-only against this machine: `cargo test -- --ignored this_machine`.
    #[test]
    #[ignore]
    fn this_machine_codex_status_and_browsers() {
        println!("codex program: {:?}", codex_program().0);
        println!("codex status: {:?}", codex_status());
        println!("installed: {:?}", installed_browsers(std::env::consts::OS, Path::exists));
        println!("settings file: {:?}", settings_path());
    }
}
