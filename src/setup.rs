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
    Chromium,
}

impl BrowserKind {
    /// Chrome first, the order choices are shown and preselected in.
    pub const ALL: [BrowserKind; 4] =
        [BrowserKind::Chrome, BrowserKind::Brave, BrowserKind::Edge, BrowserKind::Chromium];

    pub fn key(self) -> &'static str {
        match self {
            BrowserKind::Chrome => "chrome",
            BrowserKind::Brave => "brave",
            BrowserKind::Edge => "edge",
            BrowserKind::Chromium => "chromium",
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
            BrowserKind::Chromium => "Chromium",
        }
    }

    /// The macOS app's name, which is also its executable's name inside Contents/MacOS.
    fn mac_app(self) -> &'static str {
        match self {
            BrowserKind::Chrome => "Google Chrome",
            BrowserKind::Brave => "Brave Browser",
            BrowserKind::Edge => "Microsoft Edge",
            BrowserKind::Chromium => "Chromium",
        }
    }

    fn bundle_id(self) -> &'static str {
        match self {
            BrowserKind::Chrome => "com.google.Chrome",
            BrowserKind::Brave => "com.brave.Browser",
            BrowserKind::Edge => "com.microsoft.edgemac",
            BrowserKind::Chromium => "org.chromium.Chromium",
        }
    }

    /// Below %ProgramFiles%, %ProgramFiles(x86)% or %LOCALAPPDATA%.
    fn windows_exe(self) -> &'static str {
        match self {
            BrowserKind::Chrome => r"Google\Chrome\Application\chrome.exe",
            BrowserKind::Brave => r"BraveSoftware\Brave-Browser\Application\brave.exe",
            BrowserKind::Edge => r"Microsoft\Edge\Application\msedge.exe",
            BrowserKind::Chromium => r"Chromium\Application\chrome.exe",
        }
    }

    fn linux_names(self) -> &'static [&'static str] {
        match self {
            BrowserKind::Chrome => &["google-chrome", "google-chrome-stable"],
            BrowserKind::Brave => &["brave-browser", "brave"],
            BrowserKind::Edge => &["microsoft-edge", "microsoft-edge-stable"],
            BrowserKind::Chromium => &["chromium", "chromium-browser"],
        }
    }

    /// Where `find_browser` looks on `os`, for an error that says what was searched.
    pub fn searched(self, os: &str) -> String {
        match os {
            "macos" => format!("/Applications, ~/Applications and Spotlight for {}.app", self.mac_app()),
            "windows" => format!("Program Files, Program Files (x86) and AppData\\Local for {}", self.windows_exe()),
            _ => format!("PATH for {}", self.linux_names().join(", ")),
        }
    }
}

/// A browser the search found, with the executable to launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundBrowser {
    pub kind: BrowserKind,
    pub path: PathBuf,
}

/// What the browser search reads from the machine, passed in so tests can fake it.
pub struct Probe<'a> {
    /// `std::env::consts::OS`.
    pub os: &'a str,
    pub var: &'a dyn Fn(&str) -> Option<OsString>,
    pub exists: &'a dyn Fn(&Path) -> bool,
    /// App bundles Spotlight knows for a bundle id; empty when it can't say.
    pub spotlight: &'a dyn Fn(&str) -> Vec<PathBuf>,
}

/// The `kind` executable on this machine: the usual install places for the OS, then (macOS
/// only) Spotlight, which finds an app wherever it was dragged.
pub fn find_browser(kind: BrowserKind, probe: &Probe) -> Option<PathBuf> {
    let set = |name: &str| (probe.var)(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    let exists = |path: &PathBuf| (probe.exists)(path);
    match probe.os {
        "macos" => {
            let app = format!("{}.app", kind.mac_app());
            let mut dirs = vec![PathBuf::from("/Applications")];
            dirs.extend(set("HOME").map(|home| home.join("Applications")));
            let mut in_dirs = dirs.into_iter().map(|dir| bundle_executable(&dir.join(&app), Some(kind.mac_app())));
            in_dirs.find(exists).or_else(|| {
                (probe.spotlight)(kind.bundle_id())
                    .iter()
                    .flat_map(|app| [bundle_executable(app, Some(kind.mac_app())), bundle_executable(app, None)])
                    .find(exists)
            })
        }
        "windows" => ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
            .into_iter()
            .filter_map(set)
            .map(|dir| dir.join(kind.windows_exe()))
            .find(exists),
        _ => {
            let dirs: Vec<PathBuf> =
                (probe.var)("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
            dirs.iter().flat_map(|dir| kind.linux_names().iter().map(move |name| dir.join(name))).find(exists)
        }
    }
}

/// Every supported browser the search finds, Chrome first.
pub fn find_browsers(probe: &Probe) -> Vec<FoundBrowser> {
    BrowserKind::ALL
        .into_iter()
        .filter_map(|kind| find_browser(kind, probe).map(|path| FoundBrowser { kind, path }))
        .collect()
}

/// Asks Spotlight for the app bundles with `bundle_id`; a missing or failing mdfind finds none.
pub fn spotlight_apps(bundle_id: &str) -> Vec<PathBuf> {
    let query = format!("kMDItemCFBundleIdentifier == '{bundle_id}'");
    let Ok(output) = Command::new("mdfind").arg(query).stdin(Stdio::null()).stderr(Stdio::null()).output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Searches this machine with the real environment, file system and Spotlight.
pub fn search_this_machine(kind: Option<BrowserKind>) -> Vec<FoundBrowser> {
    let var = |name: &str| std::env::var_os(name);
    let spotlight = |id: &str| if cfg!(target_os = "macos") { spotlight_apps(id) } else { Vec::new() };
    let probe = Probe { os: std::env::consts::OS, var: &var, exists: &Path::is_file, spotlight: &spotlight };
    match kind {
        Some(kind) => find_browser(kind, &probe).map(|path| FoundBrowser { kind, path }).into_iter().collect(),
        None => find_browsers(&probe),
    }
}

/// `app`/Contents/MacOS/`name`, the bundle's own stem when `name` is None.
fn bundle_executable(app: &Path, name: Option<&str>) -> PathBuf {
    let name = name.map(OsString::from).or_else(|| app.file_stem().map(OsStr::to_os_string)).unwrap_or_default();
    app.join("Contents").join("MacOS").join(name)
}

/// The executable inside a picked macOS app bundle, or the picked file itself.
pub fn browser_executable(picked: &Path) -> PathBuf {
    if picked.extension().is_some_and(|e| e == "app") {
        let named = bundle_executable(picked, None);
        if named.is_file() {
            return named;
        }
        let macos = picked.join("Contents").join("MacOS");
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

    type Vars = Vec<(&'static str, OsString)>;

    /// Runs the search on `os` with these variables, files and Spotlight answers.
    fn search(os: &str, vars: Vars, files: &[PathBuf], spotlight: &[(&str, &str)]) -> Vec<FoundBrowser> {
        let var = |name: &str| vars.iter().find(|(k, _)| *k == name).map(|(_, v)| v.clone());
        let exists = |p: &Path| files.iter().any(|f| f == p);
        let apps = |id: &str| spotlight.iter().filter(|(i, _)| *i == id).map(|(_, app)| PathBuf::from(app)).collect();
        find_browsers(&Probe { os, var: &var, exists: &exists, spotlight: &apps })
    }

    fn found(kind: BrowserKind, path: impl Into<PathBuf>) -> FoundBrowser {
        FoundBrowser { kind, path: path.into() }
    }

    #[test]
    fn on_macos_the_search_checks_both_applications_folders_then_spotlight() {
        let home = vec![("HOME", OsString::from("/Users/u"))];
        let brave = PathBuf::from("/Applications/Brave Browser.app/Contents/MacOS/Brave Browser");
        let edge = PathBuf::from("/Users/u/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge");
        let chrome = PathBuf::from("/Volumes/Apps/Chrome.app/Contents/MacOS/Google Chrome");
        let files = [brave.clone(), edge.clone(), chrome.clone()];
        let spotlight = [
            ("com.google.Chrome", "/Volumes/Gone/Google Chrome.app"),
            ("com.google.Chrome", "/Volumes/Apps/Chrome.app"),
            ("org.chromium.Chromium", "/nowhere/Chromium.app"),
        ];
        assert_eq!(
            search("macos", home.clone(), &files, &spotlight),
            vec![
                found(BrowserKind::Chrome, chrome),
                found(BrowserKind::Brave, brave.clone()),
                found(BrowserKind::Edge, edge)
            ],
            "Chrome only through Spotlight, its first hit without an executable skipped; Chromium's hit has none"
        );
        let renamed = PathBuf::from("/opt/My Chromium.app/Contents/MacOS/My Chromium");
        let spotlight = [("org.chromium.Chromium", "/opt/My Chromium.app")];
        assert_eq!(
            search("macos", vec![], std::slice::from_ref(&renamed), &spotlight),
            vec![found(BrowserKind::Chromium, renamed)],
            "a renamed bundle's own executable"
        );
        assert_eq!(search("macos", home, &[], &[]), vec![], "Spotlight failing finds nothing");
        assert_eq!(
            search("macos", vec![], std::slice::from_ref(&brave), &[]),
            vec![found(BrowserKind::Brave, brave)],
            "no HOME"
        );
    }

    #[test]
    fn on_windows_the_search_checks_program_files_both_ways_and_local_app_data() {
        let (pf, x86, local) = (r"C:\Program Files", r"C:\Program Files (x86)", r"C:\Users\u\AppData\Local");
        let vars = vec![
            ("ProgramFiles", OsString::from(pf)),
            ("ProgramFiles(x86)", OsString::from(x86)),
            ("LOCALAPPDATA", OsString::from(local)),
        ];
        let edge = PathBuf::from(x86).join(r"Microsoft\Edge\Application\msedge.exe");
        let chrome = PathBuf::from(local).join(r"Google\Chrome\Application\chrome.exe");
        let chromium = PathBuf::from(local).join(r"Chromium\Application\chrome.exe");
        let brave = PathBuf::from(pf).join(r"BraveSoftware\Brave-Browser\Application\brave.exe");
        let files = [edge.clone(), chrome.clone(), chromium.clone(), brave.clone()];
        assert_eq!(
            search("windows", vars.clone(), &files, &[]),
            vec![
                found(BrowserKind::Chrome, chrome),
                found(BrowserKind::Brave, brave),
                found(BrowserKind::Edge, edge),
                found(BrowserKind::Chromium, chromium),
            ]
        );
        let both = [PathBuf::from(pf).join(r"Google\Chrome\Application\chrome.exe"), files[1].clone()];
        assert_eq!(search("windows", vars, &both, &[])[0].path, both[0], "Program Files before AppData");
        assert_eq!(search("windows", vec![], &files, &[]), vec![], "no folders known");
    }

    #[test]
    fn on_linux_the_search_looks_on_path_for_each_browsers_names() {
        let path = std::env::join_paths(["/usr/local/bin", "/usr/bin", "/snap/bin"]).unwrap();
        let vars = vec![("PATH", path)];
        let files = [
            PathBuf::from("/usr/bin/google-chrome-stable"),
            PathBuf::from("/usr/local/bin/brave"),
            PathBuf::from("/usr/bin/brave-browser"),
            PathBuf::from("/snap/bin/chromium"),
        ];
        assert_eq!(
            search("linux", vars, &files, &[]),
            vec![
                found(BrowserKind::Chrome, "/usr/bin/google-chrome-stable"),
                found(BrowserKind::Brave, "/usr/local/bin/brave"),
                found(BrowserKind::Chromium, "/snap/bin/chromium"),
            ],
            "the first PATH dir wins, whichever name"
        );
        assert_eq!(search("linux", vec![], &files, &[]), vec![]);
        assert_eq!(BrowserKind::parse("Chromium"), Some(BrowserKind::Chromium));
        assert_eq!(BrowserKind::parse("custom"), None);
        assert!(BrowserKind::Edge.searched("linux").contains("microsoft-edge-stable"));
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

    /// Read-only against this machine: `cargo test -- --ignored this_machine --nocapture`.
    #[test]
    #[ignore]
    fn this_machine_codex_status_and_browsers() {
        println!("codex program: {:?}", codex_program().0);
        println!("codex status: {:?}", codex_status());
        println!("found browsers: {:#?}", search_this_machine(None));
        println!("settings file: {:?}", settings_path());
    }
}
