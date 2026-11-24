//! Downloads the VieSVTR model: at launch while it is missing, and from the
//! settings' Retry button.
//!
//! The model is licensed CC BY-NC 4.0, so it is not in the release zip (the
//! README in the zip names it and its license). This fetches the file from
//! its author's Hugging Face repository at a fixed revision, checks its
//! SHA-256 and saves it to `viet::download_dir()`, where
//! `viet::model_path()` finds it.

#![cfg(windows)]

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows::Win32::Foundation::*;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::update::{Failure, hex, sha256};
use crate::worker::log;

/// Posted to the main window whenever `status()` changes.
pub const WM_APP_MODEL_STATUS: u32 = WM_APP + 31;

const URL: &str = "https://huggingface.co/ngocthua830/viesvtr/resolve/61bcfb92a1010a01b8d7ed59961419667abe655f/weights/viesvtr_v7.onnx";
const SHA256: &str = "da8dc9cac938472d97373e7eb88768e7e4e98300c4c4687c3766fbcb29d36b5e";

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Download {
    Idle,
    /// Percent received.
    Running(u8),
    Failed(Failure),
}

static STATUS: Mutex<Download> = Mutex::new(Download::Idle);
static MAIN: AtomicIsize = AtomicIsize::new(0);

pub fn status() -> Download {
    *STATUS.lock().unwrap()
}

fn set(status: Download) {
    *STATUS.lock().unwrap() = status;
    let main = HWND(MAIN.load(Ordering::Relaxed) as *mut _);
    if !main.is_invalid() {
        unsafe {
            let _ = PostMessageW(Some(main), WM_APP_MODEL_STATUS, WPARAM(0), LPARAM(0));
        }
    }
}

/// Downloads the model on a background thread, unless a download runs.
/// `main` gets `WM_APP_MODEL_STATUS` on progress and at the end; the model
/// is in place once the status is back to `Idle` and `viet::model_path()`
/// finds it.
pub fn start(main: HWND) {
    MAIN.store(main.0 as isize, Ordering::Relaxed);
    {
        let mut s = STATUS.lock().unwrap();
        if matches!(*s, Download::Running(_)) {
            return;
        }
        *s = Download::Running(0);
    }
    set(Download::Running(0)); // tells the main window
    std::thread::spawn(|| {
        set(match fetch(&mut |p| set(Download::Running(p))) {
            Ok(path) => {
                log(format!("model: saved {}", path.display()));
                crate::settings::update(|s| s.vietnamese_ocr = true);
                Download::Idle
            }
            Err(f) => Download::Failed(f),
        })
    });
}

fn fail<T>(kind: Failure, detail: impl std::fmt::Display) -> Result<T, Failure> {
    log(format!("model: {kind:?}: {detail}"));
    Err(kind)
}

/// Whether `data` is the model file this version expects.
fn verify(data: &[u8]) -> Result<(), String> {
    let got = hex(&sha256(data));
    if got == SHA256 { Ok(()) } else { Err(format!("SHA-256 {got}, expected {SHA256}")) }
}

/// Downloads, checks and saves the model; `percent` hears whole-percent
/// steps.
fn fetch(percent: &mut dyn FnMut(u8)) -> Result<PathBuf, Failure> {
    let dir = viet_dir().or_else(|e| fail(Failure::Files, e))?;
    let mut last = 0u8;
    let response = crate::http::get_with(URL, "application/octet-stream", &mut |read, total| {
        let p = (read * 100).checked_div(total).map_or(0, |p| p.min(99) as u8);
        if p != last {
            last = p;
            percent(p);
        }
    });
    let data = match response {
        Ok(r) if r.status == 200 => r.body,
        Ok(r) => return fail(Failure::Server, format!("{URL}: HTTP {}", r.status)),
        Err(e) => return fail(Failure::Offline, format!("{URL}: {e}")),
    };
    // Nothing is written unless the hash matches.
    verify(&data).or_else(|e| fail(Failure::HashMismatch, e))?;
    let path = dir.join(crate::viet::MODEL_FILE);
    let part = dir.join(format!("{}.part", crate::viet::MODEL_FILE));
    std::fs::write(&part, &data)
        .and_then(|()| std::fs::rename(&part, &path))
        .or_else(|e| fail(Failure::Files, format!("writing {}: {e}", path.display())))?;
    Ok(path)
}

fn viet_dir() -> Result<PathBuf, String> {
    let dir = crate::viet::download_dir().ok_or("LOCALAPPDATA not set")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    Ok(dir)
}

/// `--download-model`: downloads the model, printing progress, as a launch
/// without it does.
pub fn print_download() -> bool {
    let mut shown = 0u8;
    match fetch(&mut |p| {
        if p >= shown + 10 {
            shown = p;
            println!("{p}%");
        }
    }) {
        Ok(path) => {
            println!("saved {}", path.display());
            true
        }
        Err(f) => {
            println!("failed: {f:?}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_the_pinned_model() {
        assert!(verify(b"not the model").is_err());
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("models/viesvtr/viesvtr_v7.onnx");
        if let Ok(data) = std::fs::read(path) {
            assert_eq!(verify(&data), Ok(()));
        }
    }
}
