//! Background worker: finds the image under the cursor (UI Automation),
//! captures it from the screen (GDI) and runs Windows OCR on it.
//! Results are posted back to the UI thread as window messages.

use std::collections::HashMap;
use std::ffi::c_void;
use std::hash::Hasher;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;
use std::time::Instant;

use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

use crate::viet;

pub const WM_APP_SHOW: u32 = WM_APP + 1; // lparam = Box<OcrData>
pub const WM_APP_HIDE: u32 = WM_APP + 2;

/// Smallest image (in physical pixels) worth OCR-ing.
const MIN_W: i32 = 24;
const MIN_H: i32 = 12;
const CACHE_LIMIT: usize = 64;
/// Unload the Vietnamese model after this long without any request.
const UNLOAD_AFTER: Duration = Duration::from_secs(300);

#[derive(Clone)]
pub struct Word {
    /// Relative to the top-left of `OcrData::rect`.
    pub rect: RECT,
    pub text: String,
    pub line: usize,
}

pub struct OcrData {
    /// Screen rect (physical pixels) the overlay must cover.
    pub rect: RECT,
    pub words: Vec<Word>,
    /// Union of word rects per line, relative to `rect`.
    pub lines: Vec<RECT>,
}

pub enum Request {
    Detect(POINT),
    Revalidate,
    Forget,
}

struct Current {
    /// The UIA image element and its rect, when the image was found through UIA.
    element: Option<(IUIAutomationElement, RECT)>,
    clip: RECT,
    hash: u64,
}

pub struct Detector {
    uia: IUIAutomation,
    engine: OcrEngine,
    max_dim: u32,
    cache: HashMap<u64, Vec<Word>>,
    own_pid: i32,
    /// --probe tests a point that is not where the real cursor is.
    ignore_cursor: bool,
    viet: Option<viet::Recognizer>,
    viet_failed: bool,
}

/// Read text with the Vietnamese model (set from the tray menu).
pub static VIETNAMESE: AtomicBool = AtomicBool::new(false);

impl Detector {
    /// Must be called on a thread where COM is initialized (MTA).
    pub fn new() -> Result<Self> {
        let uia: IUIAutomation = unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)? };
        let engine = make_engine()?;
        let max_dim = OcrEngine::MaxImageDimension()?;
        Ok(Self {
            uia,
            engine,
            max_dim,
            cache: HashMap::new(),
            own_pid: std::process::id() as i32,
            ignore_cursor: false,
            viet: None,
            viet_failed: false,
        })
    }

    pub fn engine_language(&self) -> String {
        self.engine
            .RecognizerLanguage()
            .and_then(|l| l.LanguageTag())
            .map(|t| t.to_string())
            .unwrap_or_default()
    }

    /// Returns the OCR result for the image under `pt`, if there is one with text.
    fn detect(&mut self, pt: POINT) -> Result<Option<(OcrData, Current)>> {
        let t0 = Instant::now();
        let element = unsafe { self.uia.ElementFromPoint(pt)? };
        if unsafe { element.CurrentProcessId()? } == self.own_pid {
            return Ok(None);
        }
        let control_type = unsafe { element.CurrentControlType()? };
        let (clip, tracked) = if control_type == UIA_ImageControlTypeId {
            let element_rect = unsafe { element.CurrentBoundingRectangle()? };
            let Some(clip) = visible_rect(pt, element_rect) else {
                return Ok(None);
            };
            (clip, Some((element, element_rect)))
        } else if control_type == UIA_PaneControlTypeId && self.is_chromium_without_tree(pt, &element) {
            // Electron app with accessibility turned off (e.g. Zalo): find the image from pixels.
            if !self.ignore_cursor && cursor_is_ibeam() {
                log("pixel mode: cursor is over real text".into());
                return Ok(None);
            }
            let Some(clip) = image_from_pixels(pt)? else {
                return Ok(None);
            };
            (clip, None)
        } else {
            if cfg!(debug_assertions) {
                let class = unsafe { element.CurrentClassName() }.map(|s| s.to_string()).unwrap_or_default();
                log(format!("not an image: control type {} class {class:?}", control_type.0));
            }
            return Ok(None);
        };
        let w = clip.right - clip.left;
        let h = clip.bottom - clip.top;

        let pixels = capture(clip, w, h)?;
        let hash = hash_pixels(&pixels, w, h);
        // The same pixels read differently in Vietnamese mode.
        let key = hash ^ if VIETNAMESE.load(Ordering::Relaxed) { 0x9E37_79B9_7F4A_7C15 } else { 0 };

        let words = match self.cache.get(&key) {
            Some(words) => {
                log(format!("cache hit {w}x{h} in {:?}", t0.elapsed()));
                words.clone()
            }
            None => {
                let words = self.ocr(clip)?;
                log(format!("ocr {w}x{h}: {} words in {:?}", words.len(), t0.elapsed()));
                if self.cache.len() >= CACHE_LIMIT {
                    self.cache.clear();
                }
                self.cache.insert(key, words.clone());
                words
            }
        };
        if words.is_empty() {
            return Ok(None);
        }
        let data = OcrData {
            rect: clip,
            lines: line_rects(&words),
            words,
        };
        Ok(Some((data, Current { element: tracked, clip, hash })))
    }

    /// A Chromium/Electron content window whose accessibility tree is empty.
    fn is_chromium_without_tree(&self, pt: POINT, element: &IUIAutomationElement) -> bool {
        let mut class = [0u16; 64];
        let len = unsafe { GetClassNameW(WindowFromPoint(pt), &mut class) } as usize;
        if String::from_utf16_lossy(&class[..len]) != "Chrome_RenderWidgetHostHWND" {
            return false;
        }
        let Ok(walker) = (unsafe { self.uia.RawViewWalker() }) else {
            return false;
        };
        unsafe { walker.GetFirstChildElement(element) }.is_err()
    }

    /// The Vietnamese recognizer, loaded on first use.
    fn recognizer(&mut self) -> Option<&mut viet::Recognizer> {
        if self.viet.is_none() && !self.viet_failed {
            let t0 = Instant::now();
            match viet::model_path().ok_or_else(|| "model file not found".to_string()).and_then(|p| viet::Recognizer::load(&p)) {
                Ok(r) => {
                    log(format!("Vietnamese model loaded in {:?}", t0.elapsed()));
                    self.viet = Some(r);
                }
                Err(e) => {
                    log(format!("Vietnamese OCR unavailable: {e}"));
                    self.viet_failed = true;
                }
            }
        }
        self.viet.as_mut()
    }

    /// Captures `clip` (upscaled for small text) and OCRs it.
    fn ocr(&mut self, clip: RECT) -> Result<Vec<Word>> {
        let w = clip.right - clip.left;
        let h = clip.bottom - clip.top;
        let longest = w.max(h) as f32;
        // Windows OCR is much more accurate on small screen text when upscaled.
        let mut scale: f32 = if longest <= 1600.0 { 2.0 } else { 1.0 };
        scale = scale.min(self.max_dim as f32 / longest);
        let sw = ((w as f32 * scale).round() as i32).max(1);
        let sh = ((h as f32 * scale).round() as i32).max(1);

        let pixels = capture(clip, sw, sh)?;
        self.ocr_pixels(&pixels, sw, sh, scale)
    }

    /// OCRs a BGRA buffer that is `scale` times the size of the image it shows;
    /// word rects come back in the unscaled image's coordinates.
    fn ocr_pixels(&mut self, pixels: &[u8], sw: i32, sh: i32, scale: f32) -> Result<Vec<Word>> {
        let writer = DataWriter::new()?;
        writer.WriteBytes(pixels)?;
        let buffer = writer.DetachBuffer()?;
        let bitmap = SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, sw, sh)?;
        let result = self.engine.RecognizeAsync(&bitmap)?.join()?;

        // Windows OCR finds the lines and word boxes. In Vietnamese mode each
        // line is then re-read by VieSVTR, which knows the tone marks.
        let vietnamese = VIETNAMESE.load(Ordering::Relaxed) && self.recognizer().is_some();
        let t_read = Instant::now();
        let to_screen = |b: Box4| RECT {
            left: (b.0 / scale).floor() as i32,
            top: (b.1 / scale).floor() as i32,
            right: (b.2 / scale).ceil() as i32,
            bottom: (b.3 / scale).ceil() as i32,
        };
        let mut words = Vec::new();
        let lines = result.Lines()?;
        for li in 0..lines.Size()? {
            let line_words = lines.GetAt(li)?.Words()?;
            let mut boxes: Vec<(Box4, String)> = Vec::new();
            for wi in 0..line_words.Size()? {
                let word = line_words.GetAt(wi)?;
                let r = word.BoundingRect()?;
                boxes.push(((r.X, r.Y, r.X + r.Width, r.Y + r.Height), word.Text()?.to_string()));
            }
            if vietnamese {
                if let Some(recognizer) = self.viet.as_mut() {
                    match read_line(recognizer, pixels, sw as usize, sh as usize, &boxes) {
                        Ok(read) if !read.is_empty() => boxes = read,
                        Ok(_) => {}
                        Err(e) => log(format!("Vietnamese line {li} failed: {e}")),
                    }
                }
            }
            for (b, text) in boxes {
                words.push(Word { rect: to_screen(b), text, line: li as usize });
            }
        }
        if vietnamese {
            log(format!("Vietnamese read {} lines in {:?}", lines.Size()?, t_read.elapsed()));
        }
        Ok(words)
    }

    /// True if the image the overlay covers is still where it was and unchanged.
    fn still_valid(&self, cur: &Current) -> bool {
        if let Some((element, element_rect)) = &cur.element {
            let Ok(rect) = (unsafe { element.CurrentBoundingRectangle() }) else {
                return false;
            };
            if rect != *element_rect {
                log("revalidate: image moved".into());
                return false;
            }
        }
        // Pixel-detected images have no element: scrolling shows up as changed pixels.
        let w = cur.clip.right - cur.clip.left;
        let h = cur.clip.bottom - cur.clip.top;
        let same = matches!(capture(cur.clip, w, h), Ok(px) if hash_pixels(&px, w, h) == cur.hash);
        if !same {
            log("revalidate: pixels changed".into());
        }
        same
    }
}

fn make_engine() -> Result<OcrEngine> {
    let en = Language::CreateLanguage(&HSTRING::from("en-US"))?;
    if OcrEngine::IsLanguageSupported(&en)? {
        if let Ok(engine) = OcrEngine::TryCreateFromLanguage(&en) {
            return Ok(engine);
        }
    }
    OcrEngine::TryCreateFromUserProfileLanguages()
}

/// Worker thread entry point.
pub fn run(rx: Receiver<Request>, hwnd: isize) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let mut detector = match Detector::new() {
        Ok(d) => d,
        Err(e) => {
            log(format!("failed to start OCR: {e}"));
            return;
        }
    };
    log(format!("OCR language: {}", detector.engine_language()));
    // Load the Vietnamese model now rather than on the first hover (takes seconds).
    if VIETNAMESE.load(Ordering::Relaxed) {
        detector.recognizer();
    }
    let hwnd = HWND(hwnd as *mut c_void);
    let mut current: Option<Current> = None;

    loop {
        let first = match rx.recv_timeout(UNLOAD_AFTER) {
            Ok(r) => r,
            Err(RecvTimeoutError::Timeout) => {
                // Idle: give back the ~80 MB the Vietnamese model holds; it
                // reloads in a few hundred ms on the next image.
                if detector.viet.take().is_some() {
                    log("Vietnamese model unloaded (idle)".into());
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        // Coalesce queued requests: the newest Detect wins; Forget clears state.
        let mut detect = None;
        let mut revalidate = false;
        let mut handle = |r: Request, current: &mut Option<Current>| match r {
            Request::Detect(pt) => detect = Some(pt),
            Request::Revalidate => revalidate = true,
            Request::Forget => *current = None,
        };
        handle(first, &mut current);
        while let Ok(r) = rx.try_recv() {
            handle(r, &mut current);
        }

        if let Some(pt) = detect {
            match detector.detect(pt) {
                Ok(Some((data, cur))) => {
                    current = Some(cur);
                    let ptr = Box::into_raw(Box::new(data));
                    unsafe {
                        if PostMessageW(Some(hwnd), WM_APP_SHOW, WPARAM(0), LPARAM(ptr as isize)).is_err() {
                            drop(Box::from_raw(ptr));
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => log(format!("detect error: {e}")),
            }
        } else if revalidate {
            if let Some(cur) = &current {
                if !detector.still_valid(cur) {
                    current = None;
                    unsafe {
                        let _ = PostMessageW(Some(hwnd), WM_APP_HIDE, WPARAM(0), LPARAM(0));
                    }
                }
            }
        }
    }
}

/// One-shot detection at a screen point, printed to stdout. Used for testing.
pub fn probe(pt: POINT) -> Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let mut detector = Detector::new()?;
    detector.ignore_cursor = true;
    println!("OCR language: {}", detector.engine_language());
    match detector.detect(pt)? {
        None => println!("no image with text at ({}, {})", pt.x, pt.y),
        Some((data, _)) => {
            let r = data.rect;
            println!("image rect: ({}, {}) - ({}, {})", r.left, r.top, r.right, r.bottom);
            let all: Vec<usize> = (0..data.words.len()).collect();
            println!("--- text ---\n{}", crate::selection_text(&data.words, &all));
            if std::env::var_os("QC_BOXES").is_some() {
                println!("--- word boxes (relative to image) ---");
                for w in &data.words {
                    let b = w.rect;
                    println!("line {} x {}..{} y {}..{} {}", w.line, b.left, b.right, b.top, b.bottom, w.text);
                }
            }
        }
    }
    Ok(())
}

/// OCRs an image file (upscaled 2x like screen captures) and prints the text
/// and word boxes. Used for testing without driving the screen.
pub fn ocr_file(path: &str) -> Result<()> {
    use windows::Graphics::Imaging::*;
    use windows::Storage::{FileAccessMode, StorageFile};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let mut detector = Detector::new()?;
    let abs = std::fs::canonicalize(path).map_err(|e| Error::new(E_FAIL, e.to_string()))?;
    let abs = abs.to_string_lossy().trim_start_matches(r"\\?\").to_string();
    let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(abs))?.join()?;
    let stream = file.OpenAsync(FileAccessMode::Read)?.join()?;
    let decoder = BitmapDecoder::CreateAsync(&stream)?.join()?;
    let (w, h) = (decoder.PixelWidth()?, decoder.PixelHeight()?);
    let transform = BitmapTransform::new()?;
    transform.SetScaledWidth(w * 2)?;
    transform.SetScaledHeight(h * 2)?;
    transform.SetInterpolationMode(BitmapInterpolationMode::Fant)?;
    let data = decoder
        .GetPixelDataTransformedAsync(
            BitmapPixelFormat::Bgra8,
            BitmapAlphaMode::Ignore,
            &transform,
            ExifOrientationMode::IgnoreExifOrientation,
            ColorManagementMode::DoNotColorManage,
        )?
        .join()?;
    let pixels = data.DetachPixelData()?.to_vec();
    let t0 = Instant::now();
    let words = detector.ocr_pixels(&pixels, (w * 2) as i32, (h * 2) as i32, 2.0)?;
    println!("{}x{}, {} words in {:?}", w, h, words.len(), t0.elapsed());
    let all: Vec<usize> = (0..words.len()).collect();
    println!("--- text ---\n{}\n--- word boxes ---", crate::selection_text(&words, &all));
    for word in &words {
        let b = word.rect;
        println!("line {} x {}..{} y {}..{} {}", word.line, b.left, b.right, b.top, b.bottom, word.text);
    }
    Ok(())
}

/// Clips the element rect to the window under the cursor and the virtual screen,
/// so partially scrolled-out images only cover what is actually visible.
fn visible_rect(pt: POINT, r: RECT) -> Option<RECT> {
    let mut clip = r;
    unsafe {
        let hwnd = WindowFromPoint(pt);
        let mut wr = RECT::default();
        if !hwnd.is_invalid() && GetWindowRect(hwnd, &mut wr).is_ok() {
            clip = intersect(clip, wr)?;
        }
        clip = intersect(clip, virtual_screen())?;
    }
    if clip.right - clip.left < MIN_W || clip.bottom - clip.top < MIN_H {
        return None;
    }
    Some(clip)
}

fn virtual_screen() -> RECT {
    unsafe {
        let (x, y) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
        RECT {
            left: x,
            top: y,
            right: x + GetSystemMetrics(SM_CXVIRTUALSCREEN),
            bottom: y + GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

/// Captures the window under `pt` and looks for an image rectangle around it.
fn image_from_pixels(pt: POINT) -> Result<Option<RECT>> {
    let t0 = Instant::now();
    let mut wr = RECT::default();
    unsafe { GetWindowRect(WindowFromPoint(pt), &mut wr)? };
    let Some(area) = intersect(wr, virtual_screen()) else {
        return Ok(None);
    };
    if !(area.left..area.right).contains(&pt.x) || !(area.top..area.bottom).contains(&pt.y) {
        return Ok(None);
    }
    let (w, h) = (area.right - area.left, area.bottom - area.top);
    let px = capture(area, w, h)?;
    if let Some(path) = std::env::var_os("QC_DUMP").filter(|_| cfg!(debug_assertions)) {
        let _ = save_bmp(std::path::Path::new(&path), &px, w, h);
    }
    let found = crate::rectfind::find_image_rect(
        &px,
        w as usize,
        h as usize,
        (pt.x - area.left) as usize,
        (pt.y - area.top) as usize,
    );
    log(format!("pixel search in {w}x{h} took {:?}: {found:?}", t0.elapsed()));
    Ok(found.map(|(l, t, r, b)| RECT {
        left: area.left + l as i32,
        top: area.top + t as i32,
        right: area.left + r as i32,
        bottom: area.top + b as i32,
    }))
}

/// left, top, right, bottom in capture pixels.
type Box4 = (f32, f32, f32, f32);

/// Re-reads one text line with VieSVTR. `boxes` are Windows OCR's word boxes
/// for the line; the returned words get boxes from where in the line each of
/// their characters was read.
fn read_line(
    rec: &mut viet::Recognizer,
    px: &[u8],
    w: usize,
    h: usize,
    boxes: &[(Box4, String)],
) -> std::result::Result<Vec<(Box4, String)>, String> {
    if boxes.is_empty() {
        return Ok(Vec::new());
    }
    let (l, t, r, b) = boxes.iter().fold((f32::MAX, f32::MAX, f32::MIN, f32::MIN), |a, (bx, _)| {
        (a.0.min(bx.0), a.1.min(bx.1), a.2.max(bx.2), a.3.max(bx.3))
    });
    let lh = (b - t).max(1.0);
    // Tone marks sit above, and dots below, the box Windows OCR reports.
    let cx0 = (l - 0.2 * lh).max(0.0) as usize;
    let cy0 = (t - 0.35 * lh).max(0.0) as usize;
    let cx1 = ((r + 0.2 * lh).ceil() as usize).min(w);
    let cy1 = ((b + 0.2 * lh).ceil() as usize).min(h);
    if cx1 <= cx0 + 1 || cy1 <= cy0 + 1 {
        return Ok(Vec::new());
    }
    let (cw, ch) = (cx1 - cx0, cy1 - cy0);
    let mut crop = Vec::with_capacity(cw * ch * 4);
    for y in cy0..cy1 {
        crop.extend_from_slice(&px[(y * w + cx0) * 4..(y * w + cx1) * 4]);
    }

    let to_x = |f: f32| cx0 as f32 + f * cw as f32;
    let mut out = Vec::new();
    let mut word = String::new();
    let mut span: Option<(f32, f32)> = None;
    let mut flush = |word: &mut String, span: &mut Option<(f32, f32)>| {
        if let Some((x0, x1)) = span.take() {
            let left = to_x(x0);
            out.push(((left, t, to_x(x1).max(left + 1.0), b), std::mem::take(word)));
        }
        word.clear();
    };
    for c in rec.recognize(&crop, cw, ch)? {
        if c.ch == ' ' {
            flush(&mut word, &mut span);
            continue;
        }
        word.push(c.ch);
        span = Some(match span {
            Some((x0, _)) => (x0, c.x1),
            None => (c.x0, c.x1),
        });
    }
    flush(&mut word, &mut span);
    // CTC spans are a little narrow; when both engines split the line into the
    // same number of words, Windows OCR's boxes are the better outlines.
    if out.len() == boxes.len() {
        for (o, (b, _)) in out.iter_mut().zip(boxes) {
            o.0 = *b;
        }
    }
    Ok(out)
}

/// Debug helper: writes a top-down BGRA buffer as a 32-bit BMP.
fn save_bmp(path: &std::path::Path, px: &[u8], w: i32, h: i32) -> std::io::Result<()> {
    let mut f = Vec::with_capacity(54 + px.len());
    let size = 54 + px.len() as u32;
    f.extend_from_slice(b"BM");
    f.extend_from_slice(&size.to_le_bytes());
    f.extend_from_slice(&[0; 4]);
    f.extend_from_slice(&54u32.to_le_bytes());
    f.extend_from_slice(&40u32.to_le_bytes());
    f.extend_from_slice(&w.to_le_bytes());
    f.extend_from_slice(&(-h).to_le_bytes());
    f.extend_from_slice(&1u16.to_le_bytes());
    f.extend_from_slice(&32u16.to_le_bytes());
    f.extend_from_slice(&[0; 24]);
    f.extend_from_slice(px);
    std::fs::write(path, f)
}

/// Chromium shows the system I-beam over selectable text, which is not an image.
fn cursor_is_ibeam() -> bool {
    unsafe {
        let mut info = CURSORINFO { cbSize: size_of::<CURSORINFO>() as u32, ..Default::default() };
        if GetCursorInfo(&mut info).is_err() {
            return false;
        }
        LoadCursorW(None, IDC_IBEAM).is_ok_and(|ibeam| ibeam == info.hCursor)
    }
}

fn intersect(a: RECT, b: RECT) -> Option<RECT> {
    let r = RECT {
        left: a.left.max(b.left),
        top: a.top.max(b.top),
        right: a.right.min(b.right),
        bottom: a.bottom.min(b.bottom),
    };
    (r.right > r.left && r.bottom > r.top).then_some(r)
}

pub fn line_rects(words: &[Word]) -> Vec<RECT> {
    let mut lines: Vec<RECT> = Vec::new();
    for w in words {
        if w.line >= lines.len() {
            lines.resize(w.line + 1, RECT { left: i32::MAX, top: i32::MAX, right: i32::MIN, bottom: i32::MIN });
        }
        let l = &mut lines[w.line];
        l.left = l.left.min(w.rect.left);
        l.top = l.top.min(w.rect.top);
        l.right = l.right.max(w.rect.right);
        l.bottom = l.bottom.max(w.rect.bottom);
    }
    lines
}

/// Copies screen pixels of `r` into a top-down BGRA buffer of `out_w` x `out_h`
/// (scaled with HALFTONE when the sizes differ). Alpha is forced to 255.
fn capture(r: RECT, out_w: i32, out_h: i32) -> Result<Vec<u8>> {
    let w = r.right - r.left;
    let h = r.bottom - r.top;
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: out_w,
                biHeight: -out_h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let dib = match CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(d) => d,
            Err(e) => {
                let _ = DeleteDC(mem);
                ReleaseDC(None, screen);
                return Err(e);
            }
        };
        let old = SelectObject(mem, dib.into());
        let ok = if out_w == w && out_h == h {
            BitBlt(mem, 0, 0, w, h, Some(screen), r.left, r.top, SRCCOPY).is_ok()
        } else {
            SetStretchBltMode(mem, HALFTONE);
            let _ = SetBrushOrgEx(mem, 0, 0, None);
            StretchBlt(mem, 0, 0, out_w, out_h, Some(screen), r.left, r.top, w, h, SRCCOPY).as_bool()
        };
        let len = (out_w * out_h * 4) as usize;
        let mut pixels = std::slice::from_raw_parts(bits as *const u8, len).to_vec();
        SelectObject(mem, old);
        let _ = DeleteObject(dib.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        if !ok {
            return Err(Error::from_thread());
        }
        for a in pixels.iter_mut().skip(3).step_by(4) {
            *a = 255;
        }
        Ok(pixels)
    }
}

fn hash_pixels(px: &[u8], w: i32, h: i32) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    hasher.write_i32(w);
    hasher.write_i32(h);
    hasher.write(px);
    hasher.finish()
}

/// Logs to stderr in debug builds, or in release builds when QC_LOG is set
/// (stderr must then be redirected, as release builds have no console).
pub fn log(msg: String) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ENABLED.get_or_init(|| cfg!(debug_assertions) || std::env::var_os("QC_LOG").is_some()) {
        eprintln!("[quickcopy] {msg}");
    }
}
