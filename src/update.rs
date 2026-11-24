//! Finds new versions in the GitHub releases and installs them.
//!
//! A release is tagged vX.Y.Z and carries `quickcopy-X.Y.Z-windows-x64.exe`
//! and `SHA256SUMS.txt` (`<sha256 hex>  <file name>` lines). Installing
//! downloads both, checks the exe's SHA-256, puts it in place of the running
//! exe (which is renamed to `.old`), starts it with `--updated` and quits
//! once it reports that it runs; if it does not, the old exe is put back.
//! The new process waits for this one's single-instance mutex, then deletes
//! the `.old` file.
//!
//! QC_UPDATE_API (testing hook) replaces the URL of the latest-release JSON,
//! e.g. a local `python -m http.server`; plain http is then allowed.

#![cfg(windows)]

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::{Duration, SystemTime};

use serde::Deserialize;
use windows::Win32::Foundation::*;
use windows::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};
use windows::Win32::System::Threading::{CreateEventW, GetCurrentProcessId, SetEvent, WaitForSingleObject};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::HSTRING;

use crate::worker::log;

/// Posted to the main window whenever `status()` changes.
pub const WM_APP_UPDATE_STATUS: u32 = WM_APP + 30;

const LATEST: &str = "https://api.github.com/repos/duckvhuynh/quickcopy/releases/latest";
const SUMS: &str = "SHA256SUMS.txt";
const FIRST_CHECK: Duration = Duration::from_secs(10);
const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// How often the background thread wakes to see if a check is due. Sleep
/// does not count while the PC sleeps, so waking often keeps checks daily.
const TICK: Duration = Duration::from_secs(15 * 60);
/// How long the new version has to report that it runs.
const START_TIMEOUT_MS: u32 = 10_000;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Version(u32, u32, u32);

impl Version {
    /// "1.2.3" or "v1.2.3"; pre-release and build suffixes are rejected.
    pub fn parse(s: &str) -> Option<Self> {
        let mut parts = s.strip_prefix('v').unwrap_or(s).split('.').map(|p| p.parse::<u32>().ok());
        let v = Version(parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(v)
    }

    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).expect("Cargo version is X.Y.Z")
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Release {
    pub version: Version,
    pub page: String,
    pub exe: Asset,
    pub sums: Asset,
}

/// Why a check or an install did not work (details go to the log).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Failure {
    Offline,
    /// 404: nothing published yet, or the repository is private.
    NoRelease,
    /// Rate limit, another HTTP error, or a release we cannot use.
    Server,
    HashMismatch,
    /// Writing or renaming files next to the exe.
    Files,
    /// The new exe did not start, or quit right away.
    Launch,
}

impl Failure {
    pub fn text(self) -> &'static str {
        let t = crate::i18n::t();
        match self {
            Failure::Offline => t.fail_offline,
            Failure::NoRelease => t.fail_no_release,
            Failure::Server => t.fail_server,
            Failure::HashMismatch => t.fail_hash,
            Failure::Files => t.fail_files,
            Failure::Launch => t.fail_launch,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum Status {
    NotChecked,
    Checking,
    UpToDate,
    Available(Release),
    Installing(Release),
    /// Installed and started; this process is about to quit.
    Restarting,
    CheckFailed(Failure),
    InstallFailed(Release, Failure),
}

impl Status {
    /// The release the tray and settings offer to install.
    pub fn offer(&self) -> Option<&Release> {
        match self {
            Status::Available(r) | Status::InstallFailed(r, _) => Some(r),
            _ => None,
        }
    }

    pub fn busy(&self) -> bool {
        matches!(self, Status::Checking | Status::Installing(_) | Status::Restarting)
    }
}

/// The status after a check. A failed check keeps a release already found.
fn after_check(prev: Status, result: Result<Release, Failure>, current: Version) -> Status {
    match result {
        Ok(r) if r.version > current => Status::Available(r),
        Ok(_) => Status::UpToDate,
        Err(f) => prev.offer().cloned().map_or(Status::CheckFailed(f), Status::Available),
    }
}

// ---------------------------------------------------------------- parsing

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    html_url: String,
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
}

fn exe_name(v: Version) -> String {
    format!("quickcopy-{v}-windows-x64.exe")
}

/// A GitHub "latest release" response, with the assets this platform needs.
fn parse_release(json: &[u8]) -> Result<Release, String> {
    let api: ApiRelease = serde_json::from_slice(json).map_err(|e| format!("bad release JSON: {e}"))?;
    let version = Version::parse(&api.tag_name).ok_or_else(|| format!("tag {:?} is not vX.Y.Z", api.tag_name))?;
    let pick = |name: &str| {
        api.assets
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(name))
            .map(|a| Asset { name: a.name.clone(), url: a.browser_download_url.clone() })
            .ok_or_else(|| format!("release {} has no {name}", api.tag_name))
    };
    Ok(Release { version, page: api.html_url.clone(), exe: pick(&exe_name(version))?, sums: pick(SUMS)? })
}

/// The hash `SHA256SUMS.txt` lists for `name`.
fn listed_hash(sums: &str, name: &str) -> Option<[u8; 32]> {
    sums.lines().find_map(|line| {
        let (hex, file) = line.trim().split_once(char::is_whitespace)?;
        // `sha256sum -b` marks binary files with '*'.
        let file = file.trim_start().trim_start_matches('*');
        if !file.eq_ignore_ascii_case(name) || hex.len() != 64 {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()?;
        }
        Some(out)
    })
}

pub(crate) fn sha256(data: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let status = unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut out) };
    assert!(status.is_ok(), "BCryptHash: {status:?}");
    out
}

/// Whether `data` is the file `SHA256SUMS.txt` lists as `name`.
fn verify(data: &[u8], sums: &str, name: &str) -> Result<(), String> {
    let want = listed_hash(sums, name).ok_or_else(|| format!("{SUMS} does not list {name}"))?;
    let got = sha256(data);
    if got == want { Ok(()) } else { Err(format!("{name}: SHA-256 {} but {SUMS} lists {}", hex(&got), hex(&want))) }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ------------------------------------------------------------------ files

fn sibling(exe: &Path, suffix: &str) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    exe.with_file_name(name)
}

/// Puts `new` in place of `exe` and keeps the old file as `exe.old`
/// (renaming works while the exe runs). On failure `exe` is as it was.
fn swap(exe: &Path, new: &[u8]) -> io::Result<()> {
    let (new_path, old_path) = (sibling(exe, ".new"), sibling(exe, ".old"));
    std::fs::write(&new_path, new)?;
    let result = (|| {
        if old_path.exists() {
            std::fs::remove_file(&old_path)?;
        }
        std::fs::rename(exe, &old_path)?;
        std::fs::rename(&new_path, exe).inspect_err(|_| {
            let _ = std::fs::rename(&old_path, exe);
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&new_path);
    }
    result
}

/// Puts `exe.old` back in place of `exe`.
fn rollback(exe: &Path) -> io::Result<()> {
    std::fs::remove_file(exe)?;
    std::fs::rename(sibling(exe, ".old"), exe)
}

/// Deletes the `.old` exe an update left; it stays locked for a moment
/// while the previous process exits.
fn remove_leftover(exe: &Path) {
    let old = sibling(exe, ".old");
    for _ in 0..40 {
        match std::fs::remove_file(&old) {
            Ok(()) => return log(format!("removed {}", old.display())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return,
            Err(_) => std::thread::sleep(Duration::from_millis(250)),
        }
    }
    log(format!("could not remove {}", old.display()));
}

// ---------------------------------------------------------------- network

fn testing() -> bool {
    std::env::var_os("QC_UPDATE_API").is_some()
}

fn api_url() -> String {
    std::env::var("QC_UPDATE_API").unwrap_or_else(|_| LATEST.to_string())
}

fn fail<T>(kind: Failure, detail: impl std::fmt::Display) -> Result<T, Failure> {
    log(format!("update: {kind:?}: {detail}"));
    Err(kind)
}

fn fetch_latest() -> Result<Release, Failure> {
    let url = api_url();
    let response = match crate::http::get(&url, "application/vnd.github+json") {
        Ok(r) => r,
        Err(e) => return fail(Failure::Offline, format!("{url}: {e}")),
    };
    match response.status {
        200 => parse_release(&response.body).or_else(|e| fail(Failure::Server, e)),
        404 => fail(Failure::NoRelease, format!("{url}: HTTP 404 (no release, or the repository is private)")),
        s => fail(Failure::Server, format!("{url}: HTTP {s} {}", String::from_utf8_lossy(&response.body).chars().take(200).collect::<String>())),
    }
}

fn download(url: &str) -> Result<Vec<u8>, Failure> {
    if !url.starts_with("https://") && !testing() {
        return fail(Failure::Server, format!("refusing a download over plain http: {url}"));
    }
    match crate::http::get(url, "application/octet-stream") {
        Ok(r) if r.status == 200 => Ok(r.body),
        Ok(r) => fail(Failure::Server, format!("{url}: HTTP {}", r.status)),
        Err(e) => fail(Failure::Offline, format!("{url}: {e}")),
    }
}

// ------------------------------------------------------------------ state

static STATUS: Mutex<Status> = Mutex::new(Status::NotChecked);
static LAST_CHECK: Mutex<Option<SystemTime>> = Mutex::new(None);
static MAIN: AtomicIsize = AtomicIsize::new(0);

pub fn status() -> Status {
    STATUS.lock().unwrap().clone()
}

fn set(status: Status) {
    *STATUS.lock().unwrap() = status;
    notify();
}

fn notify() {
    let main = HWND(MAIN.load(Ordering::Relaxed) as *mut _);
    if !main.is_invalid() {
        unsafe {
            let _ = PostMessageW(Some(main), WM_APP_UPDATE_STATUS, WPARAM(0), LPARAM(0));
        }
    }
}

/// Cleans up after an update, then checks 10 s after start and daily after
/// that while the setting is on. `main` gets `WM_APP_UPDATE_STATUS`.
pub fn start(main: HWND) {
    MAIN.store(main.0 as isize, Ordering::Relaxed);
    std::thread::spawn(|| {
        if let Ok(exe) = std::env::current_exe() {
            remove_leftover(&exe);
        }
        std::thread::sleep(FIRST_CHECK);
        loop {
            let due = LAST_CHECK.lock().unwrap().is_none_or(|t| !t.elapsed().is_ok_and(|d| d < INTERVAL));
            if due && crate::settings::get().check_updates {
                check();
            }
            std::thread::sleep(TICK);
        }
    });
}

/// "Check now": checks on a background thread.
pub fn check_now() {
    std::thread::spawn(check);
}

fn check() {
    let prev = {
        let mut s = STATUS.lock().unwrap();
        if s.busy() {
            return;
        }
        std::mem::replace(&mut *s, Status::Checking)
    };
    notify();
    let result = fetch_latest();
    *LAST_CHECK.lock().unwrap() = Some(SystemTime::now());
    match &result {
        Ok(r) => log(format!("update: latest {} (running {})", r.version, Version::current())),
        Err(f) => log(format!("update: check failed: {f:?}")),
    }
    set(after_check(prev, result, Version::current()));
}

/// Installs the release on offer, on a background thread. Ends in
/// `Restarting` (the caller quits) or `InstallFailed`.
pub fn install() {
    let release = {
        let mut s = STATUS.lock().unwrap();
        let Some(r) = s.offer().cloned() else { return };
        *s = Status::Installing(r.clone());
        r
    };
    notify();
    std::thread::spawn(move || {
        set(match install_release(&release) {
            Ok(()) => Status::Restarting,
            Err(f) => Status::InstallFailed(release, f),
        })
    });
}

fn started_event(pid: u32) -> HSTRING {
    HSTRING::from(format!("QuickCopy.Updated.{pid}"))
}

/// Run first by an exe started with `--updated`: tells the process that
/// installed it that it runs. The event stays open for the life of the
/// process, so it is there whenever the installer opens it.
pub fn signal_started() {
    unsafe {
        if let Ok(event) = CreateEventW(None, true, false, &started_event(GetCurrentProcessId())) {
            let _ = SetEvent(event);
        }
    }
}

fn install_release(r: &Release) -> Result<(), Failure> {
    let exe = std::env::current_exe().or_else(|e| fail(Failure::Files, e))?;
    let data = download(&r.exe.url)?;
    let sums = download(&r.sums.url)?;
    // Nothing is written, let alone run, unless the hash matches.
    verify(&data, &String::from_utf8_lossy(&sums), &r.exe.name).or_else(|e| fail(Failure::HashMismatch, e))?;
    swap(&exe, &data).or_else(|e| fail(Failure::Files, format!("replacing {}: {e}", exe.display())))?;
    log(format!("update: installed {} at {}", r.version, exe.display()));
    // The new exe says it runs (`signal_started`) before it waits for our
    // mutex; one that does not (it crashed, or is not QuickCopy) is replaced
    // by the old exe.
    let started = match std::process::Command::new(&exe).arg("--updated").spawn() {
        Ok(mut child) => {
            let started = unsafe {
                CreateEventW(None, true, false, &started_event(child.id())).is_ok_and(|event| {
                    let signaled = WaitForSingleObject(event, START_TIMEOUT_MS) == WAIT_OBJECT_0;
                    let _ = CloseHandle(event);
                    signaled
                })
            };
            if !started {
                let _ = child.kill();
                let _ = child.wait();
            }
            started
        }
        Err(e) => {
            log(format!("update: starting {}: {e}", exe.display()));
            false
        }
    };
    if started {
        return Ok(());
    }
    if let Err(e) = rollback(&exe) {
        log(format!("update: rolling back {}: {e}", exe.display()));
    }
    fail(Failure::Launch, "the new version did not start; the old one is back")
}

/// `--check-update`: prints what a check finds, installs nothing.
pub fn print_check() -> bool {
    println!("running: {}", Version::current());
    println!("source:  {}", api_url());
    match fetch_latest() {
        Ok(r) => {
            println!("latest:  {} ({})", r.version, r.page);
            println!("newer:   {}", if r.version > Version::current() { "yes" } else { "no" });
            println!("exe:     {}", r.exe.url);
            println!("sums:    {}", r.sums.url);
            true
        }
        Err(f) => {
            println!("check failed: {f:?} (QC_LOG=1 shows details)");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release_json(tag: &str, names: &[&str]) -> Vec<u8> {
        let assets: Vec<_> = names
            .iter()
            .map(|n| serde_json::json!({ "name": n, "browser_download_url": format!("https://github.com/d/q/releases/download/{tag}/{n}"), "size": 1 }))
            .collect();
        serde_json::to_vec(&serde_json::json!({ "tag_name": tag, "html_url": format!("https://github.com/d/q/releases/tag/{tag}"), "draft": false, "assets": assets })).unwrap()
    }

    #[test]
    fn versions_compare_numerically() {
        let v = |s| Version::parse(s).unwrap();
        assert!(v("v0.10.0") > v("v0.9.9"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert!(v("0.1.10") > v("0.1.9"));
        assert_eq!(v("v1.2.3"), v("1.2.3"));
        assert_eq!(v("1.2.3").to_string(), "1.2.3");
        for bad in ["1.2", "1.2.3.4", "v1.2.3-beta.1", "1.x.3", "", "vv1.2.3"] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
        assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn picks_the_exe_and_sums_of_the_tagged_version() {
        let json = release_json(
            "v0.3.1",
            &["QuickCopy-0.3.1-windows-x64.zip", "quickcopy-0.3.0-windows-x64.exe", "quickcopy-0.3.1-windows-x64.exe", "SHA256SUMS.txt"],
        );
        let r = parse_release(&json).unwrap();
        assert_eq!(r.version, Version(0, 3, 1));
        assert_eq!(r.page, "https://github.com/d/q/releases/tag/v0.3.1");
        assert_eq!(r.exe.name, "quickcopy-0.3.1-windows-x64.exe");
        assert_eq!(r.exe.url, "https://github.com/d/q/releases/download/v0.3.1/quickcopy-0.3.1-windows-x64.exe");
        assert_eq!(r.sums.name, "SHA256SUMS.txt");
        // A release without the bare exe (or the sums) cannot be installed.
        assert!(parse_release(&release_json("v0.3.1", &["QuickCopy-0.3.1-windows-x64.zip", "SHA256SUMS.txt"])).is_err());
        assert!(parse_release(&release_json("v0.3.1", &["quickcopy-0.3.1-windows-x64.exe"])).is_err());
        assert!(parse_release(&release_json("nightly", &["SHA256SUMS.txt"])).is_err());
        assert!(parse_release(b"{\"message\":\"Not Found\"}").is_err());
    }

    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn sha256_matches_the_standard_vectors() {
        assert_eq!(hex(&sha256(b"abc")), ABC);
        assert_eq!(hex(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    #[test]
    fn reads_sha256sums_lines() {
        let sums = format!("{}  QuickCopy-0.2.0-windows-x64.zip\r\n{ABC}  quickcopy-0.2.0-windows-x64.exe\n", "0".repeat(64));
        assert_eq!(listed_hash(&sums, "quickcopy-0.2.0-windows-x64.exe").map(|h| hex(&h)).as_deref(), Some(ABC));
        assert_eq!(listed_hash(&sums, "QuickCopy-0.2.0-windows-x64.zip"), Some([0; 32]));
        assert_eq!(listed_hash(&sums, "SHA256SUMS.txt"), None);
        // Binary-mode marker, and a malformed hash.
        assert!(listed_hash(&format!("{ABC} *a.exe"), "a.exe").is_some());
        assert!(listed_hash("abc  a.exe", "a.exe").is_none());
    }

    #[test]
    fn verify_accepts_only_the_listed_hash() {
        let sums = format!("{ABC}  a.exe\n");
        assert!(verify(b"abc", &sums, "a.exe").is_ok());
        assert!(verify(b"abd", &sums, "a.exe").is_err());
        assert!(verify(b"abc", &sums, "b.exe").is_err());
    }

    #[test]
    fn a_failed_check_keeps_a_release_already_found() {
        let r = parse_release(&release_json("v9.0.0", &["quickcopy-9.0.0-windows-x64.exe", "SHA256SUMS.txt"])).unwrap();
        let current = Version(1, 0, 0);
        assert_eq!(after_check(Status::NotChecked, Ok(r.clone()), current), Status::Available(r.clone()));
        assert_eq!(after_check(Status::NotChecked, Ok(r.clone()), Version(9, 0, 0)), Status::UpToDate);
        assert_eq!(after_check(Status::UpToDate, Err(Failure::Offline), current), Status::CheckFailed(Failure::Offline));
        assert_eq!(after_check(Status::Available(r.clone()), Err(Failure::Offline), current), Status::Available(r.clone()));
        assert_eq!(after_check(Status::InstallFailed(r.clone(), Failure::Files), Err(Failure::NoRelease), current), Status::Available(r));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("quickcopy-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn swap_replaces_the_exe_and_rollback_restores_it() {
        let dir = temp_dir("swap");
        let exe = dir.join("quickcopy.exe");
        std::fs::write(&exe, "v1").unwrap();
        // A leftover from an earlier update is replaced.
        std::fs::write(dir.join("quickcopy.exe.old"), "v0").unwrap();

        swap(&exe, b"v2").unwrap();
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "v2");
        assert_eq!(std::fs::read_to_string(dir.join("quickcopy.exe.old")).unwrap(), "v1");
        assert!(!dir.join("quickcopy.exe.new").exists());

        rollback(&exe).unwrap();
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "v1");
        assert!(!dir.join("quickcopy.exe.old").exists());

        swap(&exe, b"v2").unwrap();
        remove_leftover(&exe);
        assert!(!dir.join("quickcopy.exe.old").exists());
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "v2");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_swap_leaves_the_exe_alone() {
        let dir = temp_dir("swap-fail");
        let exe = dir.join("quickcopy.exe");
        std::fs::write(&exe, "v1").unwrap();
        // The `.old` name is taken by a folder that cannot be removed.
        std::fs::create_dir(dir.join("quickcopy.exe.old")).unwrap();
        std::fs::write(dir.join("quickcopy.exe.old").join("x"), "").unwrap();
        assert!(swap(&exe, b"v2").is_err());
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "v1");
        assert!(!dir.join("quickcopy.exe.new").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
