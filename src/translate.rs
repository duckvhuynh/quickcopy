//! Translates selected text with Google Translate and samples the colors
//! needed to paint the translation over the original, like Google Lens.
//!
//! Uses the endpoint the Google Translate web client calls (client=gtx) and
//! its public web key; set QC_GOOGLE_KEY to use another key. It is not an
//! official API: Google may change or block it. Text is sent only when the
//! user asks for a translation.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;

use windows::Win32::Foundation::*;
use windows::Win32::Networking::WinHttp::*;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::*;

use crate::worker::log;

pub const WM_APP_TRANSLATED: u32 = WM_APP + 4; // lparam = Box<Translated>

const HOST: &str = "translate-pa.googleapis.com";
const DEFAULT_KEY: &str = "AIzaSyDLEeFI5OtFBwYBIoK_jj5m32rZK5CkCXA";
/// Keep request URLs well under common length limits.
const MAX_CHARS: usize = 1500;

pub type Rgb = [u8; 3];

/// One paragraph (or the selected part of it) to translate, and the boxes
/// (overlay coordinates) its selected text occupies, one per line.
pub struct Unit {
    pub text: String,
    pub slices: Vec<RECT>,
    /// The original text of each slice, used to match its look.
    pub slice_texts: Vec<String>,
}

pub struct Slice {
    pub rect: RECT,
    pub background: Rgb,
    pub foreground: Rgb,
    /// How the original text measures (size, baseline, stroke), if it could.
    pub ink: Option<crate::style::Ink>,
    pub original: String,
}

pub struct UnitResult {
    pub translation: String,
    pub slices: Vec<Slice>,
}

pub struct Translated {
    /// `OcrData::id` of the image this belongs to.
    pub id: u64,
    pub result: std::result::Result<Vec<UnitResult>, String>,
}

/// Translates `units` on a background thread and posts `WM_APP_TRANSLATED`
/// to `hwnd`. `clip` is the image's screen rect, used to sample colors.
pub fn spawn(hwnd: HWND, id: u64, clip: RECT, target: &'static str, units: Vec<Unit>) {
    let hwnd = hwnd.0 as isize;
    std::thread::spawn(move || {
        let result = run(clip, target, units);
        let ptr = Box::into_raw(Box::new(Translated { id, result }));
        unsafe {
            if PostMessageW(Some(HWND(hwnd as *mut c_void)), WM_APP_TRANSLATED, WPARAM(0), LPARAM(ptr as isize)).is_err() {
                drop(Box::from_raw(ptr));
            }
        }
    });
}

fn run(clip: RECT, target: &str, units: Vec<Unit>) -> std::result::Result<Vec<UnitResult>, String> {
    let (w, h) = (clip.right - clip.left, clip.bottom - clip.top);
    let pixels = crate::worker::capture(clip, w, h).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for unit in units {
        let translation = translate(&unit.text, target)?;
        let slices = unit
            .slices
            .iter()
            .zip(unit.slice_texts)
            .map(|(&rect, original)| {
                let (background, foreground) = sample_colors(&pixels, w, h, rect);
                // OCR boxes can clip ascenders and descenders; give the
                // measurement the whole line height.
                let pad = (rect.bottom - rect.top) / 4;
                let ink = crate::style::measure(&pixels, w, h, inflate(rect, 0, pad), background, foreground);
                Slice { rect, background, foreground, ink, original }
            })
            .collect();
        out.push(UnitResult { translation, slices });
    }
    Ok(out)
}

static CACHE: Mutex<Option<HashMap<(String, String), String>>> = Mutex::new(None);

/// Translates `text` to `target`, or to English when it already is in
/// `target` (so Vietnamese text still gets a useful translation).
pub fn translate(text: &str, target: &str) -> std::result::Result<String, String> {
    let key = (text.to_string(), target.to_string());
    if let Some(hit) = CACHE.lock().unwrap().as_ref().and_then(|c| c.get(&key)) {
        return Ok(hit.clone());
    }
    let t0 = std::time::Instant::now();
    let mut translated = String::new();
    for chunk in chunks(text, MAX_CHARS) {
        let (mut part, source) = request(chunk, target)?;
        if source == target && target != "en" {
            part = request(chunk, "en")?.0;
        }
        if !translated.is_empty() {
            translated.push(' ');
        }
        translated.push_str(&part);
    }
    log(format!("translated {} chars in {:?}", text.chars().count(), t0.elapsed()));
    CACHE.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, translated.clone());
    Ok(translated)
}

/// Splits long text at sentence ends (or spaces) into pieces of at most
/// `max` characters.
fn chunks(text: &str, max: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text.trim();
    while rest.chars().count() > max {
        let cut_at = rest.char_indices().nth(max).map_or(rest.len(), |(i, _)| i);
        let head = &rest[..cut_at];
        let split = head.rfind(". ").map(|i| i + 1).or_else(|| head.rfind(' ')).unwrap_or(cut_at);
        out.push(rest[..split].trim());
        rest = rest[split..].trim_start();
    }
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

/// One request; returns the translation and the detected source language.
fn request(text: &str, target: &str) -> std::result::Result<(String, String), String> {
    let key = std::env::var("QC_GOOGLE_KEY").unwrap_or_else(|_| DEFAULT_KEY.to_string());
    let path = format!(
        "/v1/translate?params.client=gtx&query.source_language=auto&query.target_language={}&query.display_language=en-US&query.text={}&key={}&data_types=TRANSLATION",
        encode(target),
        encode(text),
        encode(&key)
    );
    let body = https_get(HOST, &path).map_err(|e| format!("request failed: {e}"))?;
    let json: serde_json::Value = serde_json::from_slice(&body).map_err(|e| format!("bad response: {e}"))?;
    let translation = json["translation"]
        .as_str()
        .ok_or_else(|| format!("no translation in response: {}", String::from_utf8_lossy(&body).chars().take(200).collect::<String>()))?;
    let source = json["sourceLanguage"].as_str().unwrap_or("").to_string();
    Ok((translation.to_string(), source))
}

/// Percent-encodes a query value as UTF-8.
fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// HTTPS GET with WinHTTP (built into Windows: no TLS library to ship).
fn https_get(host: &str, path: &str) -> Result<Vec<u8>> {
    struct Handle(*mut c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let _ = WinHttpCloseHandle(self.0);
                }
            }
        }
    }
    let check = |h: *mut c_void| if h.is_null() { Err(Error::from_thread()) } else { Ok(Handle(h)) };
    unsafe {
        let session = check(WinHttpOpen(w!("QuickCopy/0.1"), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0))?;
        WinHttpSetTimeouts(session.0, 5000, 5000, 8000, 8000)?;
        let connect = check(WinHttpConnect(session.0, &HSTRING::from(host), INTERNET_DEFAULT_HTTPS_PORT, 0))?;
        let request = check(WinHttpOpenRequest(
            connect.0,
            w!("GET"),
            &HSTRING::from(path),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ))?;
        WinHttpSendRequest(request.0, None, None, 0, 0, 0)?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut())?;
        let mut body = Vec::new();
        loop {
            let mut available = 0u32;
            WinHttpQueryDataAvailable(request.0, &mut available)?;
            if available == 0 {
                break;
            }
            let start = body.len();
            body.resize(start + available as usize, 0);
            let mut read = 0u32;
            WinHttpReadData(request.0, body[start..].as_mut_ptr() as *mut c_void, available, &mut read)?;
            body.truncate(start + read as usize);
        }
        Ok(body)
    }
}

/// Background: median color of a thin ring just outside `rect`.
/// Foreground: average of the pixels inside that differ most from it.
/// `px` is a top-down BGRA capture of size `w` x `h`; `rect` is inside it.
pub fn sample_colors(px: &[u8], w: i32, h: i32, rect: RECT) -> (Rgb, Rgb) {
    let at = |x: i32, y: i32| -> Rgb {
        let i = ((y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) * 4) as usize;
        [px[i + 2], px[i + 1], px[i]]
    };
    let (outer, inner) = (inflate(rect, 4, 4), inflate(rect, 1, 1));
    let mut ring = Vec::new();
    for y in outer.top.max(0)..outer.bottom.min(h) {
        for x in outer.left.max(0)..outer.right.min(w) {
            if x < inner.left || x >= inner.right || y < inner.top || y >= inner.bottom {
                ring.push(at(x, y));
            }
        }
    }
    let background = median(&mut ring).unwrap_or([255, 255, 255]);

    let mut inside: Vec<(i32, Rgb)> = Vec::new();
    for y in rect.top.max(0)..rect.bottom.min(h) {
        for x in rect.left.max(0)..rect.right.min(w) {
            let c = at(x, y);
            inside.push((distance(c, background), c));
        }
    }
    inside.sort_by_key(|&(d, _)| std::cmp::Reverse(d));
    // Only the stroke cores: a wider share pulls in antialiased edge pixels,
    // which are a blend with the background (black text came out brown).
    let top = &inside[..(inside.len() / 100).max(3).min(inside.len())];
    let foreground = if top.is_empty() || top[0].0 < 60 {
        // Too little contrast to tell: plain black or white, whichever reads.
        if luminance(background) > 128 { [0, 0, 0] } else { [255, 255, 255] }
    } else {
        let n = top.len() as i32;
        let sum = top.iter().fold([0i32; 3], |s, &(_, c)| [s[0] + c[0] as i32, s[1] + c[1] as i32, s[2] + c[2] as i32]);
        // Thin strokes rarely have a pixel of pure ink: push the color a
        // quarter further away from the background.
        let mut c = [0u8; 3];
        for i in 0..3 {
            let avg = sum[i] / n;
            let b = background[i] as i32;
            c[i] = (b + (avg - b) * 5 / 4).clamp(0, 255) as u8;
        }
        c
    };
    (background, foreground)
}

fn median(colors: &mut [Rgb]) -> Option<Rgb> {
    if colors.is_empty() {
        return None;
    }
    let mid = colors.len() / 2;
    let mut out = [0u8; 3];
    for (c, slot) in out.iter_mut().enumerate() {
        let mut channel: Vec<u8> = colors.iter().map(|p| p[c]).collect();
        channel.sort_unstable();
        *slot = channel[mid];
    }
    Some(out)
}

fn distance(a: Rgb, b: Rgb) -> i32 {
    (0..3).map(|i| (a[i] as i32 - b[i] as i32).abs()).sum()
}

fn luminance(c: Rgb) -> u32 {
    (c[0] as u32 * 299 + c[1] as u32 * 587 + c[2] as u32 * 114) / 1000
}

pub fn inflate(r: RECT, dx: i32, dy: i32) -> RECT {
    RECT { left: r.left - dx, top: r.top - dy, right: r.right + dx, bottom: r.bottom + dy }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_utf8_query_values() {
        assert_eq!(encode("Hôm nay & 18%"), "H%C3%B4m%20nay%20%26%2018%25");
    }

    #[test]
    fn chunks_split_at_sentences() {
        let text = "One two three. Four five six. Seven eight.";
        assert_eq!(chunks(text, 20), vec!["One two three.", "Four five six.", "Seven eight."]);
        assert_eq!(chunks("short", 20), vec!["short"]);
    }

    #[test]
    fn samples_background_and_text_colors() {
        // 40x20 white image with a dark-blue "text" block in the middle.
        let (w, h) = (40, 20);
        let mut px = vec![255u8; (w * h * 4) as usize];
        for y in 7..13 {
            for x in 12..28 {
                let i = ((y * w + x) * 4) as usize;
                px[i..i + 3].copy_from_slice(&[120, 30, 10]); // BGR
            }
        }
        let (bg, fg) = sample_colors(&px, w, h, RECT { left: 10, top: 6, right: 30, bottom: 14 });
        assert_eq!(bg, [255, 255, 255]);
        // (10, 30, 120) pushed a quarter further from white.
        assert_eq!(fg, [0, 0, 87]);
    }
}
