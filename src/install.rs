//! Installs the standalone Codex CLI from the latest openai/codex GitHub release into the app's
//! own bin dir: no admin rights, npm or Homebrew. The system `curl` and `tar` do the download
//! and unpacking (both ship with macOS, Windows 10+ and Linux).

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;
use sha2::{Digest, Sha256};

pub const RELEASE_API: &str = "https://api.github.com/repos/openai/codex/releases/latest";
/// Every asset URL must start with this, so a release JSON can't point the download elsewhere.
pub const DOWNLOAD_PREFIX: &str = "https://github.com/openai/codex/releases/download/";

/// A platform the release publishes a standalone CLI for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacArm,
    MacIntel,
    WindowsX64,
    WindowsArm,
    LinuxX64,
    LinuxArm,
}

impl Platform {
    /// From `std::env::consts::{OS, ARCH}`.
    pub fn from(os: &str, arch: &str) -> Option<Platform> {
        match (os, arch) {
            ("macos", "aarch64") => Some(Platform::MacArm),
            ("macos", "x86_64") => Some(Platform::MacIntel),
            ("windows", "x86_64") => Some(Platform::WindowsX64),
            ("windows", "aarch64") => Some(Platform::WindowsArm),
            ("linux", "x86_64") => Some(Platform::LinuxX64),
            ("linux", "aarch64") => Some(Platform::LinuxArm),
            _ => None,
        }
    }

    pub fn current() -> Option<Platform> {
        Platform::from(std::env::consts::OS, std::env::consts::ARCH)
    }

    fn triple(self) -> &'static str {
        match self {
            Platform::MacArm => "aarch64-apple-darwin",
            Platform::MacIntel => "x86_64-apple-darwin",
            Platform::WindowsX64 => "x86_64-pc-windows-msvc",
            Platform::WindowsArm => "aarch64-pc-windows-msvc",
            // The musl builds are static, so they run on any distribution.
            Platform::LinuxX64 => "x86_64-unknown-linux-musl",
            Platform::LinuxArm => "aarch64-unknown-linux-musl",
        }
    }

    fn windows(self) -> bool {
        matches!(self, Platform::WindowsX64 | Platform::WindowsArm)
    }

    /// The one file inside the archive, such as `codex-aarch64-apple-darwin`.
    pub fn member(self) -> String {
        format!("codex-{}{}", self.triple(), if self.windows() { ".exe" } else { "" })
    }

    /// The plain CLI as a tar.gz, which the system tar unpacks everywhere (the .zst needs zstd).
    pub fn asset_name(self) -> String {
        format!("{}.tar.gz", self.member())
    }

    pub fn exe_name(self) -> &'static str {
        if self.windows() {
            "codex.exe"
        } else {
            "codex"
        }
    }
}

/// The release asset to download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub tag: String,
    pub name: String,
    pub url: String,
    pub size: u64,
    /// Lowercase hex, from the digest GitHub records for each uploaded asset.
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallError {
    Unsupported { os: String, arch: String },
    NoFolder,
    Release(String),
    NoAsset { tag: String, name: String },
    NoChecksum(String),
    UntrustedUrl(String),
    Download(String),
    Mismatch { name: String, expected: String, got: String },
    Unpack(String),
    Io(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::Unsupported { os, arch } => write!(f, "Codex has no download for {os} on {arch}."),
            InstallError::NoFolder => f.write_str("No app folder is known for this computer."),
            InstallError::Release(e) => write!(f, "Could not read the latest Codex release: {e}"),
            InstallError::NoAsset { tag, name } => write!(f, "Codex {tag} has no {name} download."),
            InstallError::NoChecksum(name) => write!(f, "{name} has no published checksum, so it was not installed."),
            InstallError::UntrustedUrl(url) => write!(f, "Refused to download from {url}."),
            InstallError::Download(e) => write!(f, "Download failed: {e}"),
            InstallError::Mismatch { name, expected, got } => {
                write!(f, "{name} did not match its checksum (expected {expected}, got {got}).")
            }
            InstallError::Unpack(e) => write!(f, "Could not unpack Codex: {e}"),
            InstallError::Io(e) => write!(f, "Could not write Codex: {e}"),
        }
    }
}

impl std::error::Error for InstallError {}

fn io(e: std::io::Error) -> InstallError {
    InstallError::Io(e.to_string())
}

/// The plain CLI asset for `platform` in a GitHub release JSON.
pub fn pick_asset(release: &Value, platform: Platform) -> Result<Asset, InstallError> {
    let tag = release["tag_name"].as_str().unwrap_or("latest").to_string();
    let name = platform.asset_name();
    let asset = release["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["name"] == name.as_str())
        .ok_or_else(|| InstallError::NoAsset { tag: tag.clone(), name: name.clone() })?;
    let url = asset["browser_download_url"].as_str().unwrap_or_default().to_string();
    if !url.starts_with(DOWNLOAD_PREFIX) {
        return Err(InstallError::UntrustedUrl(url));
    }
    let sha256 = asset["digest"]
        .as_str()
        .and_then(|d| d.strip_prefix("sha256:"))
        .filter(|hex| hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(|| InstallError::NoChecksum(name.clone()))?
        .to_ascii_lowercase();
    Ok(Asset { tag, name, url, size: asset["size"].as_u64().unwrap_or(0), sha256 })
}

/// The app's own bin dir: Application Support on macOS, %APPDATA% on Windows,
/// $XDG_DATA_HOME (else ~/.local/share) elsewhere.
pub fn bin_dir_for(os: &str, var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let set = |name: &str| var(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    let dir = match os {
        "macos" => set("HOME")?.join("Library").join("Application Support").join("Bilinkers"),
        "windows" => set("APPDATA")?.join("Bilinkers"),
        _ => set("XDG_DATA_HOME").or_else(|| set("HOME").map(|h| h.join(".local").join("share")))?.join("bilinkers"),
    };
    Some(dir.join("bin"))
}

pub fn bin_dir() -> Option<PathBuf> {
    bin_dir_for(std::env::consts::OS, |name| std::env::var_os(name))
}

/// The network half of an install, faked in tests.
pub trait Fetch {
    fn release(&self) -> Result<Value, String>;
    fn download(&self, url: &str, to: &Path) -> Result<(), String>;
}

/// `curl` over HTTPS only, redirects included (GitHub serves assets from its own CDN).
pub struct Curl;

fn curl(args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("curl")
        .args(["--fail", "--location", "--silent", "--show-error", "--proto", "=https", "--proto-redir", "=https"])
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("curl could not start: {e}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if err.is_empty() { format!("curl exited with {}", output.status) } else { err });
    }
    Ok(output.stdout)
}

impl Fetch for Curl {
    fn release(&self) -> Result<Value, String> {
        let body = curl(&["-H", "Accept: application/vnd.github+json", "-A", "bilinkers", RELEASE_API])?;
        serde_json::from_slice(&body).map_err(|e| e.to_string())
    }

    fn download(&self, url: &str, to: &Path) -> Result<(), String> {
        let to = to.to_str().ok_or("the download path is not valid UTF-8")?;
        curl(&["--retry", "2", "-o", to, url]).map(|_| ())
    }
}

fn sha256_of(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn untar(archive: &Path, member: &str, into: &Path) -> Result<(), InstallError> {
    let output = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .arg(member)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| InstallError::Unpack(format!("tar could not start: {e}")))?;
    if !output.status.success() {
        return Err(InstallError::Unpack(String::from_utf8_lossy(&output.stderr).trim().to_string()));
    }
    Ok(())
}

/// What an install put where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub path: PathBuf,
    pub tag: String,
    /// The archive's size.
    pub bytes: u64,
}

/// Downloads the latest CLI for `platform` into `dir`, checks its SHA-256, unpacks it in a
/// staging folder inside `dir` and renames it into place, so `dir` never holds half a binary.
pub fn install(fetch: &impl Fetch, platform: Platform, dir: &Path) -> Result<Installed, InstallError> {
    let release = fetch.release().map_err(InstallError::Release)?;
    let asset = pick_asset(&release, platform)?;
    std::fs::create_dir_all(dir).map_err(io)?;
    let staging = dir.join(format!(".codex-install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(io)?;
    let result = install_in(fetch, platform, &asset, &staging, dir);
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn install_in(
    fetch: &impl Fetch,
    platform: Platform,
    asset: &Asset,
    staging: &Path,
    dir: &Path,
) -> Result<Installed, InstallError> {
    let archive = staging.join(&asset.name);
    fetch.download(&asset.url, &archive).map_err(InstallError::Download)?;
    let got = sha256_of(&archive).map_err(io)?;
    if got != asset.sha256 {
        return Err(InstallError::Mismatch { name: asset.name.clone(), expected: asset.sha256.clone(), got });
    }
    let bytes = std::fs::metadata(&archive).map_err(io)?.len();
    untar(&archive, &platform.member(), staging)?;
    let unpacked = staging.join(platform.member());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&unpacked, std::fs::Permissions::from_mode(0o755)).map_err(io)?;
    }
    let path = dir.join(platform.exe_name());
    std::fs::rename(&unpacked, &path).map_err(io)?;
    Ok(Installed { path, tag: asset.tag.clone(), bytes })
}

/// Installs for this machine into the app's bin dir.
pub fn install_here() -> Result<Installed, InstallError> {
    let platform = Platform::current().ok_or_else(|| InstallError::Unsupported {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
    })?;
    install(&Curl, platform, &bin_dir().ok_or(InstallError::NoFolder)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;

    const SHA: &str = "4b22a2a971be2791c3e31f912f3e77a88ebd64a65812a7289ccba65dd6425fe9";

    fn asset(name: &str) -> Value {
        json!({
            "name": name,
            "size": 108379331,
            "content_type": "application/x-gtar",
            "digest": format!("sha256:{SHA}"),
            "browser_download_url": format!("{DOWNLOAD_PREFIX}rust-v0.167.0/{name}"),
        })
    }

    /// The shape of `gh api repos/openai/codex/releases/latest` for rust-v0.167.0, with the
    /// look-alike assets that must not be picked.
    fn release() -> Value {
        let names = [
            "codex",
            "codex-aarch64-apple-darwin.dmg",
            "codex-aarch64-apple-darwin.tar.gz",
            "codex-aarch64-apple-darwin.zst",
            "codex-app-server-aarch64-apple-darwin.tar.gz",
            "codex-package-aarch64-apple-darwin.tar.gz",
            "codex-npm-darwin-arm64-0.167.0.tgz",
            "codex-x86_64-apple-darwin.tar.gz",
            "codex-x86_64-pc-windows-msvc.exe",
            "codex-x86_64-pc-windows-msvc.exe.tar.gz",
            "codex-x86_64-pc-windows-msvc.exe.zip",
            "codex-aarch64-pc-windows-msvc.exe.tar.gz",
            "codex-x86_64-unknown-linux-musl.sigstore",
            "codex-x86_64-unknown-linux-musl.tar.gz",
            "codex-aarch64-unknown-linux-musl.tar.gz",
            "codex-package_SHA256SUMS",
        ];
        json!({ "tag_name": "rust-v0.167.0", "name": "0.167.0", "assets": names.map(asset) })
    }

    #[test]
    fn each_platform_picks_the_plain_cli_tarball() {
        let picked = |os, arch| pick_asset(&release(), Platform::from(os, arch).unwrap()).unwrap().name;
        assert_eq!(picked("macos", "aarch64"), "codex-aarch64-apple-darwin.tar.gz");
        assert_eq!(picked("macos", "x86_64"), "codex-x86_64-apple-darwin.tar.gz");
        assert_eq!(picked("windows", "x86_64"), "codex-x86_64-pc-windows-msvc.exe.tar.gz");
        assert_eq!(picked("windows", "aarch64"), "codex-aarch64-pc-windows-msvc.exe.tar.gz");
        assert_eq!(picked("linux", "x86_64"), "codex-x86_64-unknown-linux-musl.tar.gz");
        assert_eq!(picked("linux", "aarch64"), "codex-aarch64-unknown-linux-musl.tar.gz");
        assert_eq!(Platform::from("freebsd", "x86_64"), None);
        assert_eq!(Platform::WindowsX64.member(), "codex-x86_64-pc-windows-msvc.exe");
        assert_eq!((Platform::WindowsX64.exe_name(), Platform::LinuxArm.exe_name()), ("codex.exe", "codex"));
        let mac = pick_asset(&release(), Platform::MacArm).unwrap();
        assert_eq!(mac.tag, "rust-v0.167.0");
        assert_eq!(mac.sha256, SHA);
        assert_eq!(mac.size, 108379331);
    }

    #[test]
    fn a_missing_asset_checksum_or_foreign_url_is_refused() {
        let mut no_mac = release();
        no_mac["assets"].as_array_mut().unwrap().retain(|a| a["name"] != "codex-aarch64-apple-darwin.tar.gz");
        assert_eq!(
            pick_asset(&no_mac, Platform::MacArm),
            Err(InstallError::NoAsset {
                tag: "rust-v0.167.0".into(),
                name: "codex-aarch64-apple-darwin.tar.gz".into()
            })
        );
        let mut no_digest = release();
        no_digest["assets"][2]["digest"] = Value::Null;
        assert!(matches!(pick_asset(&no_digest, Platform::MacArm), Err(InstallError::NoChecksum(_))));
        let mut elsewhere = release();
        elsewhere["assets"][2]["browser_download_url"] = json!("https://example.com/codex.tar.gz");
        assert!(matches!(pick_asset(&elsewhere, Platform::MacArm), Err(InstallError::UntrustedUrl(_))));
    }

    #[test]
    fn the_bin_dir_lives_in_each_platforms_app_data_dir() {
        let vars = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| OsString::from(v))
        };
        assert_eq!(
            bin_dir_for("macos", vars(&[("HOME", "/Users/u")])),
            Some(PathBuf::from("/Users/u/Library/Application Support/Bilinkers/bin"))
        );
        assert_eq!(
            bin_dir_for("windows", vars(&[("APPDATA", r"C:\Users\u\AppData\Roaming")])),
            Some(PathBuf::from(r"C:\Users\u\AppData\Roaming").join("Bilinkers").join("bin"))
        );
        assert_eq!(
            bin_dir_for("linux", vars(&[("HOME", "/home/u")])),
            Some(PathBuf::from("/home/u/.local/share/bilinkers/bin"))
        );
        assert_eq!(
            bin_dir_for("linux", vars(&[("HOME", "/home/u"), ("XDG_DATA_HOME", "/data")])),
            Some(PathBuf::from("/data/bilinkers/bin"))
        );
        assert_eq!(bin_dir_for("windows", vars(&[])), None);
    }

    /// Serves `release` and copies `archive` as the download, recording each URL.
    struct FakeFetch {
        release: Result<Value, String>,
        archive: Result<PathBuf, String>,
        urls: RefCell<Vec<String>>,
    }

    impl Fetch for FakeFetch {
        fn release(&self) -> Result<Value, String> {
            self.release.clone()
        }
        fn download(&self, url: &str, to: &Path) -> Result<(), String> {
            self.urls.borrow_mut().push(url.to_string());
            let archive = self.archive.clone()?;
            std::fs::copy(archive, to).map(|_| ()).map_err(|e| e.to_string())
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bilinkers-install-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        names.sort();
        names
    }

    fn release_for(archive: &Path) -> Value {
        let mut release = release();
        release["assets"][2]["digest"] = json!(format!("sha256:{}", sha256_of(archive).unwrap()));
        release
    }

    #[test]
    fn a_failed_release_download_or_checksum_leaves_no_codex_behind() {
        let dir = temp_dir("fail");
        let bin = dir.join("bin");
        let junk = dir.join("junk.tar.gz");
        std::fs::write(&junk, b"not the release").unwrap();
        let fake = |release, archive| FakeFetch { release, archive, urls: RefCell::default() };

        let offline = fake(Err("Could not resolve host".into()), Ok(junk.clone()));
        assert_eq!(
            install(&offline, Platform::MacArm, &bin),
            Err(InstallError::Release("Could not resolve host".into()))
        );

        let cut = fake(Ok(release()), Err("HTTP 404".into()));
        assert_eq!(install(&cut, Platform::MacArm, &bin), Err(InstallError::Download("HTTP 404".into())));
        assert_eq!(
            cut.urls.borrow().as_slice(),
            [format!("{DOWNLOAD_PREFIX}rust-v0.167.0/codex-aarch64-apple-darwin.tar.gz")]
        );

        let tampered = fake(Ok(release()), Ok(junk.clone()));
        assert!(matches!(install(&tampered, Platform::MacArm, &bin), Err(InstallError::Mismatch { .. })));

        let bad_tar = fake(Ok(release_for(&junk)), Ok(junk.clone()));
        assert!(matches!(install(&bad_tar, Platform::MacArm, &bin), Err(InstallError::Unpack(_))));
        assert_eq!(entries(&bin), Vec::<String>::new(), "no codex and no staging folder");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_verified_archive_is_unpacked_made_executable_and_renamed_into_place() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("ok");
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join(Platform::MacArm.member()), "#!/bin/sh\necho codex-cli 0.167.0\n").unwrap();
        let archive = dir.join("codex.tar.gz");
        let packed = Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&src)
            .arg(Platform::MacArm.member())
            .status()
            .unwrap();
        assert!(packed.success());
        let bin = dir.join("bin");
        let fetch =
            FakeFetch { release: Ok(release_for(&archive)), archive: Ok(archive.clone()), urls: RefCell::default() };
        let installed = install(&fetch, Platform::MacArm, &bin).unwrap();
        assert_eq!(installed.path, bin.join("codex"));
        assert_eq!(installed.tag, "rust-v0.167.0");
        assert_eq!(installed.bytes, std::fs::metadata(&archive).unwrap().len());
        assert_eq!(entries(&bin), ["codex"], "the staging folder is gone");
        assert_eq!(std::fs::metadata(&installed.path).unwrap().permissions().mode() & 0o777, 0o755);
        let out = Command::new(&installed.path).arg("--version").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "codex-cli 0.167.0");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Downloads the real release into a temp dir, never the app's own:
    /// `cargo test -- --ignored real_install --nocapture`.
    #[test]
    #[ignore]
    fn real_install_into_a_temp_dir() {
        let dir = temp_dir("real");
        let installed = install(&Curl, Platform::current().unwrap(), &dir).unwrap();
        let version = Command::new(&installed.path).arg("--version").output().unwrap();
        println!("installed {} ({} bytes) at {}", installed.tag, installed.bytes, installed.path.display());
        println!("--version: {}", String::from_utf8_lossy(&version.stdout).trim());
        let xattr = Command::new("xattr").arg(&installed.path).output().map(|o| o.stdout).unwrap_or_default();
        println!("xattr: {:?}", String::from_utf8_lossy(&xattr).trim());
        assert!(version.status.success());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
