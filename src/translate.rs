//! Translates selected text with Google Translate and samples the colors
//! needed to paint the translation over the original, like Google Lens.
//!
//! Uses the endpoint the Google Translate web client calls (client=gtx) and
//! its public web key; set QC_GOOGLE_KEY to use another key. It is not an
//! official API: Google may change or block it. Text is sent only when the
//! user asks for a translation.

use std::collections::HashMap;
#[cfg(windows)]
use std::ffi::c_void;
use std::sync::Mutex;

#[cfg(windows)]
use windows::Win32::Foundation::*;
#[cfg(windows)]
use windows::Win32::Networking::WinHttp::*;
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
#[cfg(windows)]
use windows::core::*;

use crate::common::{RECT, log};

#[cfg(windows)]
pub const WM_APP_TRANSLATED: u32 = WM_APP + 4; // lparam = Box<Translated>

const HOST: &str = "translate-pa.googleapis.com";
const DEFAULT_KEY: &str = "AIzaSyDLEeFI5OtFBwYBIoK_jj5m32rZK5CkCXA";
/// Keep request URLs well under common length limits.
const MAX_CHARS: usize = 1500;

pub type Rgb = [u8; 3];

/// One selected line: its highlight box (overlay coordinates) and text.
pub struct Line {
    pub rect: RECT,
    pub text: String,
}

pub struct Slice {
    pub rect: RECT,
    pub background: Rgb,
    pub foreground: Rgb,
    /// How the original text measures (size, baseline, stroke), if it could.
    pub ink: Option<crate::style::Ink>,
    pub original: String,
}

/// Consecutive lines translated together, and the translation.
pub struct UnitResult {
    pub translation: String,
    pub slices: Vec<Slice>,
}

pub struct Translated {
    /// The request this answers (the number given to `spawn`).
    pub request: u64,
    pub result: std::result::Result<Vec<UnitResult>, String>,
}

/// Translates the selected `lines` (reading order) on a background thread and
/// posts `WM_APP_TRANSLATED` to `hwnd`. `clip` is the image's screen rect.
#[cfg(windows)]
pub fn spawn(hwnd: HWND, request: u64, clip: RECT, target: String, lines: Vec<Line>) {
    let hwnd = hwnd.0 as isize;
    std::thread::spawn(move || {
        let result = run(clip, &target, lines);
        let ptr = Box::into_raw(Box::new(Translated { request, result }));
        unsafe {
            if PostMessageW(Some(HWND(hwnd as *mut c_void)), WM_APP_TRANSLATED, WPARAM(0), LPARAM(ptr as isize)).is_err() {
                drop(Box::from_raw(ptr));
            }
        }
    });
}

#[cfg(windows)]
fn run(clip: RECT, target: &str, lines: Vec<Line>) -> std::result::Result<Vec<UnitResult>, String> {
    let (w, h) = (clip.right - clip.left, clip.bottom - clip.top);
    let pixels = crate::worker::capture(clip, w, h).map_err(|e| e.to_string())?;
    let slices: Vec<Slice> = lines
        .into_iter()
        .map(|Line { rect, text }| {
            let (background, foreground) = sample_colors(&pixels, w, h, rect);
            // OCR boxes can clip ascenders and descenders; give the
            // measurement the whole line height.
            let pad = (rect.bottom - rect.top) / 4;
            let ink = crate::style::measure(&pixels, w, h, inflate(rect, 0, pad), background, foreground);
            Slice { rect, background, foreground, ink, original: text }
        })
        .collect();
    let groups = group(&slices);
    log(format!("{} line(s) in {} unit(s)", slices.len(), groups.len()));
    let texts: Vec<String> = groups.iter().map(|range| slices[range.clone()].iter().map(|s| s.original.trim()).collect::<Vec<_>>().join(" ")).collect();
    let t0 = std::time::Instant::now();
    let translations = translate_all(&texts.iter().map(String::as_str).collect::<Vec<_>>(), target);
    log(format!("{} unit(s) translated in {:?}", texts.len(), t0.elapsed()));
    // The groups are consecutive and cover every line, in order.
    let mut rest = slices.into_iter();
    let mut out = Vec::new();
    for (range, translation) in groups.into_iter().zip(translations) {
        out.push(UnitResult { translation: translation?, slices: rest.by_ref().take(range.len()).collect() });
    }
    Ok(out)
}

/// Splits lines (reading order) into units translated on their own: a
/// heading, a paragraph, a list item. A new unit starts where a sentence
/// ends at a line's end, where the look changes (text color, size, weight:
/// a bold white title over gray body text), or at a gap clearly wider than
/// the usual line spacing. Without this, a whole selection read as one
/// sentence and its translation flowed across headings and paragraphs.
pub fn group(lines: &[Slice]) -> Vec<std::ops::Range<usize>> {
    let gaps: Vec<i32> = lines.windows(2).map(|p| p[1].rect.top - p[0].rect.bottom).collect();
    let mut sorted: Vec<i32> = gaps.iter().copied().filter(|&g| g >= 0).collect();
    sorted.sort_unstable();
    let usual = sorted.get(sorted.len() / 2).copied().unwrap_or(0) as f32;

    // Right edge that wrapped lines reach.
    let block_right = lines.iter().map(|l| l.rect.right).max().unwrap_or(0);

    let mut out = Vec::new();
    let mut start = 0;
    for i in 1..lines.len() {
        let (prev, next) = (&lines[i - 1], &lines[i]);
        let h = (prev.rect.bottom - prev.rect.top).min(next.rect.bottom - next.rect.top).max(1) as f32;
        let gap = gaps[i - 1] as f32;
        let sentence_end = prev.original.trim_end().ends_with(['.', '!', '?', ':', ';', '…', '>']);
        // A line that stops although the next line's first word would have
        // fit after it was broken on purpose (a heading without a period, a
        // list item), not wrapped. Only when the next line starts a new
        // phrase (capital letter or digit), so a sentence is never cut.
        let next_text = next.original.trim();
        let first_word = next_text.split_whitespace().next().unwrap_or("");
        let chars = next_text.chars().count().max(1) as f32;
        let first_w = (next.rect.right - next.rect.left) as f32 * first_word.chars().count() as f32 / chars;
        let room = (block_right - prev.rect.right) as f32;
        let starts_new = next_text.chars().next().is_some_and(|c| c.is_uppercase() || c.is_ascii_digit());
        let continues = prev.original.trim_end().ends_with([',', '-', '–']);
        let deliberate_break = starts_new && !continues && room > first_w + 0.5 * h;
        let wide_gap = gap > 2.2 * h || (gap > 0.5 * h && gap > 1.5 * usual + 1.0);
        let color_change = distance(prev.foreground, next.foreground) > 90;
        let style_change = match (prev.ink, next.ink) {
            (Some(a), Some(b)) => {
                let size = a.body.max(b.body) / a.body.min(b.body).max(0.1);
                let weight = (a.stroke / a.body.max(0.1) - b.stroke / b.body.max(0.1)).abs();
                size > 1.2 || weight > 0.05
            }
            _ => false,
        };
        if sentence_end || deliberate_break || wide_gap || color_change || style_change {
            out.push(start..i);
            start = i;
        }
    }
    if start < lines.len() {
        out.push(start..lines.len());
    }
    out
}

/// Requests in flight at once (they share one HTTP/2 connection).
const PARALLEL: usize = 8;
const CACHE_LIMIT: usize = 256;

/// Translates several texts at the same time; results in the same order.
pub fn translate_all(texts: &[&str], target: &str) -> Vec<std::result::Result<String, String>> {
    if let [text] = texts {
        return vec![translate(text, target)];
    }
    let mut out = Vec::with_capacity(texts.len());
    for batch in texts.chunks(PARALLEL) {
        std::thread::scope(|s| {
            let running: Vec<_> = batch.iter().map(|text| s.spawn(move || translate(text, target))).collect();
            out.extend(running.into_iter().map(|t| t.join().unwrap_or_else(|_| Err("translation failed".into()))));
        });
    }
    out
}

static CACHE: Mutex<Option<HashMap<(String, String), String>>> = Mutex::new(None);

/// Translates `text` to `target` (see the fallback below when it already
/// is in `target`).
pub fn translate(text: &str, target: &str) -> std::result::Result<String, String> {
    let key = (text.to_string(), target.to_string());
    if let Some(hit) = CACHE.lock().unwrap().as_ref().and_then(|c| c.get(&key)) {
        return Ok(hit.clone());
    }
    let t0 = std::time::Instant::now();
    let mut translated = String::new();
    for chunk in chunks(text, MAX_CHARS) {
        let (mut part, source) = request(chunk, target)?;
        // Text already in the target language: translate it into English,
        // or into Vietnamese when English was the target.
        if source == target {
            part = request(chunk, if target == "en" { "vi" } else { "en" })?.0;
        }
        if !translated.is_empty() {
            translated.push(' ');
        }
        translated.push_str(&part);
    }
    log(format!("translated {} chars in {:?}", text.chars().count(), t0.elapsed()));
    let mut cache = CACHE.lock().unwrap();
    let cache = cache.get_or_insert_with(HashMap::new);
    if cache.len() >= CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(key, translated.clone());
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
    let body = https_get(&path).map_err(|e| format!("request failed: {e}"))?;
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

#[cfg(windows)]
struct Handle(*mut c_void);

#[cfg(windows)]
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

// WinHTTP handles may be used from any thread.
#[cfg(windows)]
unsafe impl Send for Handle {}

#[cfg(windows)]
fn check(h: *mut c_void) -> Result<Handle> {
    if h.is_null() { Err(Error::from_thread()) } else { Ok(Handle(h)) }
}

/// The connection to `HOST` that every request shares, opened on first use
/// and kept for the life of the process. A session per request repeated
/// proxy detection, DNS and the TLS handshake each time (measured: three
/// requests took 0.4-2.9 s one after another on sessions of their own, and
/// 0.14 s sent together on this one).
#[cfg(windows)]
fn connection() -> Result<*mut c_void> {
    static SHARED: Mutex<Option<(Handle, Handle)>> = Mutex::new(None);
    let mut shared = SHARED.lock().unwrap();
    if shared.is_none() {
        unsafe {
            let session = check(WinHttpOpen(w!("QuickCopy/0.1"), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0))?;
            WinHttpSetTimeouts(session.0, 5000, 5000, 8000, 8000)?;
            // HTTP/2, so requests sent together share the connection.
            let _ = WinHttpSetOption(Some(session.0), WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL, Some(&WINHTTP_PROTOCOL_FLAG_HTTP2.to_ne_bytes()));
            let connect = check(WinHttpConnect(session.0, &HSTRING::from(HOST), INTERNET_DEFAULT_HTTPS_PORT, 0))?;
            *shared = Some((session, connect));
        }
    }
    Ok(shared.as_ref().map_or(std::ptr::null_mut(), |(_, connect)| connect.0))
}

/// HTTPS GET with WinHTTP (built into Windows: no TLS library to ship).
#[cfg(windows)]
fn https_get(path: &str) -> Result<Vec<u8>> {
    unsafe {
        let request = check(WinHttpOpenRequest(
            connection()?,
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

/// No HTTPS transport off Windows yet (docs/linux.md).
#[cfg(not(windows))]
fn https_get(_path: &str) -> std::result::Result<Vec<u8>, String> {
    Err("no HTTPS transport on this platform yet".into())
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

    fn line(top: i32, text: &str, fg: Rgb, body: f32, stroke: f32) -> Slice {
        Slice {
            rect: RECT { left: 0, top, right: 300, bottom: top + 14 },
            background: [30, 35, 45],
            foreground: fg,
            ink: Some(crate::style::Ink { body, baseline: top as f32 + 11.0, stroke }),
            original: text.into(),
        }
    }

    #[test]
    fn groups_headings_paragraphs_and_sentences() {
        // A Telegram-like screen: white bold heading, gray body lines 7 px
        // apart, then the next section 27 px below.
        let white = [255, 255, 255];
        let gray = [150, 150, 155];
        let lines = [
            line(0, "Phí SMS cao", white, 7.0, 2.2),
            line(14, "Vì tình hình tài khoản giả mạo và phí SMS", gray, 7.0, 1.1),
            line(35, "cao ở quốc gia của bạn, bạn sẽ cần trả phí", gray, 7.0, 1.1),
            line(56, "đăng ký.", gray, 7.0, 1.1),
            line(97, "Yêu cầu xác thực", white, 7.0, 2.2),
            line(118, "Telegram cần gửi cho bạn SMS chứa mã", gray, 7.0, 1.1),
        ];
        assert_eq!(group(&lines), vec![0..1, 1..4, 4..5, 5..6]);
    }

    #[test]
    fn splits_a_short_heading_line_but_not_a_wrapped_sentence() {
        let black = [0, 0, 0];
        let mut heading = line(0, "Hướng dẫn sử dụng nhanh", black, 10.0, 1.6);
        heading.rect.right = 180;
        let mut wrapped = line(21, "you can restore any earlier version. Finally, the", black, 10.0, 1.6);
        wrapped.rect.right = 250;
        let lines = [
            heading,
            line(21, "Bước một: mở ứng dụng và đăng nhập bằng tài khoản.", black, 10.0, 1.6),
            wrapped,
            line(42, "settings page has been reorganized for you", black, 10.0, 1.6),
        ];
        // Heading: next line starts with a capital and its first word fits
        // after it. "Finally, the" / "settings": lowercase, same sentence.
        assert_eq!(group(&lines), vec![0..1, 1..2, 2..4]);
    }

    #[test]
    fn keeps_a_sentence_across_evenly_spaced_lines() {
        let black = [0, 0, 0];
        let lines = [
            line(0, "Do nhu cầu sắp xếp lại công việc cuối năm, ban giám đốc quyết định", black, 10.0, 1.6),
            line(30, "điều chỉnh thời gian làm việc từ ngày 15 đến ngày 31 như sau:", black, 10.0, 1.6),
            line(60, "1. Buổi sáng bắt đầu lúc 7 giờ 30 phút.", black, 10.0, 1.6),
        ];
        assert_eq!(group(&lines), vec![0..2, 2..3]);
    }

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

/// Request timings against the real service:
///   cargo test --release bench_translate -- --ignored --nocapture
#[cfg(test)]
mod bench {
    use super::*;

    #[test]
    #[ignore = "network"]
    fn bench_translate() {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let texts: Vec<String> = ["The settings page has been reorganized", "Open the app and sign in", "Your order ships tomorrow", "Press any key to continue", "Changes are saved automatically", "This file is read only"]
            .iter()
            .map(|t| format!("{t} ({stamp})."))
            .collect();
        let t_all = std::time::Instant::now();
        for text in &texts[..3] {
            let t0 = std::time::Instant::now();
            let out = translate(text, "vi").unwrap();
            println!("one by one: {:?}  {out:?}", t0.elapsed());
        }
        println!("3 requests one by one: {:?}", t_all.elapsed());
        let t0 = std::time::Instant::now();
        let out = translate_all(&texts[3..].iter().map(String::as_str).collect::<Vec<_>>(), "vi");
        println!("3 requests via translate_all: {:?}  ok: {}", t0.elapsed(), out.iter().filter(|r| r.is_ok()).count());
    }
}
