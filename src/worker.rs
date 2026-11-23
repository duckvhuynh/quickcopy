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
pub const WM_APP_UPDATE: u32 = WM_APP + 3; // lparam = Box<OcrData>
/// How long an image may keep changing (opening animation) before we give up.
const SETTLE_TIMEOUT: Duration = Duration::from_millis(1200);
const SETTLE_STEP: Duration = Duration::from_millis(40);
/// Time a Chromium window gets to build its accessibility tree on first contact.
/// Edge needed ~380 ms; Electron apps with accessibility off never get one.
const TREE_WAIT: Duration = Duration::from_millis(1000);
/// Prefetch retries while a newly opened window loads its image.
const PREFETCH_TRIES: usize = 6;
const PREFETCH_RETRY: Duration = Duration::from_millis(250);

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
    /// Identifies the image, so a later `WM_APP_UPDATE` can find its overlay.
    pub id: u64,
    /// Words are Windows OCR's; the Vietnamese reading is still coming.
    pub pending: bool,
}

impl OcrData {
    fn new(rect: RECT, words: Vec<Word>, id: u64, pending: bool) -> Self {
        Self { rect, lines: line_rects(&words), words, id, pending }
    }
}

/// Which result `detect` is reporting.
pub enum Stage {
    /// First words for an image (posted as `WM_APP_SHOW`).
    Show,
    /// Final words for an image already shown (posted as `WM_APP_UPDATE`).
    Update,
}

pub enum Request {
    Detect(POINT),
    /// OCR the image at this point ahead of time (a window just opened).
    Prefetch(POINT),
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
    /// Chromium top-level windows already given time to build a tree (see detect).
    waited: std::collections::HashSet<isize>,
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
            waited: Default::default(),
        })
    }

    pub fn engine_language(&self) -> String {
        self.engine
            .RecognizerLanguage()
            .and_then(|l| l.LanguageTag())
            .map(|t| t.to_string())
            .unwrap_or_default()
    }

    /// Finds the image under `pt` and OCRs it. `report` gets the result as
    /// soon as Windows OCR has the words (`Stage::Show`, marked pending when
    /// Vietnamese is still being read), then the final words (`Stage::Update`).
    /// `from_cursor` is false for prefetching, where the cursor is elsewhere.
    fn detect(&mut self, pt: POINT, from_cursor: bool, report: &mut dyn FnMut(Stage, OcrData)) -> Result<Option<Current>> {
        let t0 = Instant::now();
        let mut element = unsafe { self.uia.ElementFromPoint(pt)? };
        if unsafe { element.CurrentProcessId()? } == self.own_pid {
            return Ok(None);
        }
        let mut control_type = unsafe { element.CurrentControlType()? };
        // Chrome/Edge build their accessibility tree only after the first
        // query, so a fresh window looks empty like Zalo. Ask once more after
        // a moment; windows still empty then are remembered as pixel-only.
        // Before that they answer with a bare pane (the content area, or the
        // frame's "View"), which is how this check recognizes them.
        if control_type == UIA_PaneControlTypeId {
            if let Some(root) = chromium_root(pt) {
                if self.waited.insert(root) {
                    let start = Instant::now();
                    while start.elapsed() < TREE_WAIT {
                        std::thread::sleep(Duration::from_millis(100));
                        element = unsafe { self.uia.ElementFromPoint(pt)? };
                        control_type = unsafe { element.CurrentControlType()? };
                        if control_type != UIA_PaneControlTypeId {
                            log(format!("accessibility tree appeared after {:?}", start.elapsed()));
                            break;
                        }
                    }
                }
            }
        }
        let (clip, tracked) = if control_type == UIA_ImageControlTypeId {
            let element_rect = unsafe { element.CurrentBoundingRectangle()? };
            let Some(clip) = visible_rect(pt, element_rect) else {
                return Ok(None);
            };
            (clip, Some((element, element_rect)))
        } else if control_type == UIA_PaneControlTypeId && self.is_chromium_without_tree(pt, &element) {
            // Electron app with accessibility turned off (e.g. Zalo): find the image from pixels.
            if from_cursor && !self.ignore_cursor && cursor_is_ibeam() {
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

        let (w, h) = (clip.right - clip.left, clip.bottom - clip.top);
        // The same pixels read differently in Vietnamese mode.
        let mode = if VIETNAMESE.load(Ordering::Relaxed) { 0x9E37_79B9_7F4A_7C15 } else { 0 };
        let mut hash = hash_pixels(&capture(clip, w, h)?, w, h);
        if !self.cache.contains_key(&(hash ^ mode)) {
            // Not seen before. An image still animating in (viewer zoom, fade)
            // would be read half-drawn, so wait until two captures agree.
            // Cache hits skip this: they matched an image that had settled.
            let Some(settled) = stable_hash(clip, hash)? else {
                log("image never settled".into());
                return Ok(None);
            };
            hash = settled;
        }
        let key = hash ^ mode;
        let current = Current { element: tracked, clip, hash };

        if let Some(words) = self.cache.get(&key) {
            log(format!("cache hit {w}x{h} in {:?}", t0.elapsed()));
            if !words.is_empty() {
                report(Stage::Show, OcrData::new(clip, words.clone(), key, false));
            }
            return Ok(Some(current));
        }

        let (pixels, sw, sh, scale) = self.capture_scaled(clip)?;
        let lines = self.windows_lines(&pixels, sw, sh)?;
        let vietnamese = VIETNAMESE.load(Ordering::Relaxed) && self.recognizer().is_some();
        let first = to_words(&lines, scale);
        log(format!("windows ocr {w}x{h}: {} words in {:?}", first.len(), t0.elapsed()));
        if first.is_empty() {
            self.remember(key, Vec::new());
            return Ok(None);
        }
        report(Stage::Show, OcrData::new(clip, first.clone(), key, vietnamese));

        let words = if vietnamese {
            let lines = self.read_vietnamese(&pixels, sw, sh, lines);
            let words = to_words(&lines, scale);
            log(format!("vietnamese ready in {:?}", t0.elapsed()));
            report(Stage::Update, OcrData::new(clip, words.clone(), key, false));
            words
        } else {
            first
        };
        self.remember(key, words);
        Ok(Some(current))
    }

    fn remember(&mut self, key: u64, words: Vec<Word>) {
        if self.cache.len() >= CACHE_LIMIT {
            self.cache.clear();
        }
        self.cache.insert(key, words);
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

    /// Captures `clip` upscaled for OCR. Returns pixels, size and the scale.
    fn capture_scaled(&self, clip: RECT) -> Result<(Vec<u8>, i32, i32, f32)> {
        let w = clip.right - clip.left;
        let h = clip.bottom - clip.top;
        let longest = w.max(h) as f32;
        // Windows OCR is much more accurate on small screen text when upscaled.
        let mut scale: f32 = if longest <= 1600.0 { 2.0 } else { 1.0 };
        scale = scale.min(self.max_dim as f32 / longest);
        let sw = ((w as f32 * scale).round() as i32).max(1);
        let sh = ((h as f32 * scale).round() as i32).max(1);
        Ok((capture(clip, sw, sh)?, sw, sh, scale))
    }

    /// OCRs a BGRA buffer that is `scale` times the size of the image it shows,
    /// all stages at once; word rects are in the unscaled image's coordinates.
    fn ocr_pixels(&mut self, pixels: &[u8], sw: i32, sh: i32, scale: f32) -> Result<Vec<Word>> {
        let mut lines = self.windows_lines(pixels, sw, sh)?;
        if VIETNAMESE.load(Ordering::Relaxed) && self.recognizer().is_some() {
            lines = self.read_vietnamese(pixels, sw, sh, lines);
        }
        Ok(to_words(&lines, scale))
    }

    /// Windows OCR: per line, its words' boxes (capture pixels) and text.
    fn windows_lines(&self, pixels: &[u8], sw: i32, sh: i32) -> Result<Vec<Vec<(Box4, String)>>> {
        let writer = DataWriter::new()?;
        writer.WriteBytes(pixels)?;
        let buffer = writer.DetachBuffer()?;
        let bitmap = SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, sw, sh)?;
        let result = self.engine.RecognizeAsync(&bitmap)?.join()?;
        let lines = result.Lines()?;
        let mut out = Vec::new();
        for li in 0..lines.Size()? {
            let line_words = lines.GetAt(li)?.Words()?;
            let mut boxes = Vec::new();
            for wi in 0..line_words.Size()? {
                let word = line_words.GetAt(wi)?;
                let r = word.BoundingRect()?;
                boxes.push(((r.X, r.Y, r.X + r.Width, r.Y + r.Height), word.Text()?.to_string()));
            }
            out.push(boxes);
        }
        Ok(out)
    }

    /// Re-reads every line with VieSVTR, which knows the tone marks Windows
    /// OCR lacks. Lines it cannot read keep Windows OCR's words.
    fn read_vietnamese(&mut self, pixels: &[u8], sw: i32, sh: i32, mut lines: Vec<Vec<(Box4, String)>>) -> Vec<Vec<(Box4, String)>> {
        let t0 = Instant::now();
        let crops: Vec<Option<Crop>> = lines.iter().map(|b| crop_line(pixels, sw as usize, sh as usize, b)).collect();
        let present: Vec<(usize, &Crop)> = crops.iter().enumerate().filter_map(|(i, c)| Some((i, c.as_ref()?))).collect();
        let inputs: Vec<viet::Line> = present.iter().map(|(_, c)| viet::Line { px: &c.px, w: c.w, h: c.h }).collect();
        match self.viet.as_mut().map(|r| r.recognize_batch(&inputs)) {
            Some(Ok(read)) => {
                for ((li, crop), chars) in present.iter().zip(read) {
                    let vi = words_from_chars(crop, &chars, &lines[*li]);
                    if !vi.is_empty() {
                        lines[*li] = vi;
                    }
                }
            }
            Some(Err(e)) => log(format!("Vietnamese read failed: {e}")),
            None => {}
        }
        log(format!("Vietnamese read {} lines in {:?}", present.len(), t0.elapsed()));
        lines
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
    // A request that arrived while prefetching; handled next.
    let mut backlog: Option<Request> = None;

    loop {
        let next = match backlog.take() {
            Some(r) => Ok(r),
            None => rx.recv_timeout(UNLOAD_AFTER),
        };
        let first = match next {
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
        // Coalesce queued requests: the newest Detect wins, then prefetches;
        // Forget clears state.
        let mut detect = None;
        let mut prefetch = None;
        let mut revalidate = false;
        let mut handle = |r: Request, current: &mut Option<Current>| match r {
            Request::Detect(pt) => detect = Some(pt),
            Request::Prefetch(pt) => prefetch = Some(pt),
            Request::Revalidate => revalidate = true,
            Request::Forget => *current = None,
        };
        handle(first, &mut current);
        while let Ok(r) = rx.try_recv() {
            handle(r, &mut current);
        }

        if let Some(pt) = detect {
            let mut report = |stage: Stage, data: OcrData| {
                let msg = match stage {
                    Stage::Show => WM_APP_SHOW,
                    Stage::Update => WM_APP_UPDATE,
                };
                let ptr = Box::into_raw(Box::new(data));
                unsafe {
                    if PostMessageW(Some(hwnd), msg, WPARAM(0), LPARAM(ptr as isize)).is_err() {
                        drop(Box::from_raw(ptr));
                    }
                }
            };
            match detector.detect(pt, true, &mut report) {
                Ok(Some(cur)) => current = Some(cur),
                Ok(None) => {}
                Err(e) => log(format!("detect error: {e}")),
            }
        } else if let Some(pt) = prefetch {
            // Fill the cache only; the overlay appears when the cursor arrives.
            // A window that just opened may not show its image yet, so retry
            // for a while, giving way as soon as the cursor needs the worker.
            for attempt in 0..PREFETCH_TRIES {
                if attempt > 0 {
                    std::thread::sleep(PREFETCH_RETRY);
                    if let Ok(r) = rx.try_recv() {
                        backlog = Some(r);
                        break;
                    }
                }
                match detector.detect(pt, false, &mut |_, _| {}) {
                    Ok(Some(cur)) => {
                        let c = cur.clip;
                        log(format!("prefetch at ({}, {}): cached, clip ({}, {}) - ({}, {})", pt.x, pt.y, c.left, c.top, c.right, c.bottom));
                        break;
                    }
                    Ok(None) => log(format!("prefetch at ({}, {}): nothing yet", pt.x, pt.y)),
                    Err(e) => {
                        log(format!("prefetch error: {e}"));
                        break;
                    }
                }
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
    let mut last = None;
    let found = detector.detect(pt, true, &mut |_, data| last = Some(data))?;
    match (found, last) {
        (Some(_), Some(data)) => {
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
        _ => println!("no image with text at ({}, {})", pt.x, pt.y),
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

/// The top-level window under `pt` if it is a Chromium/Electron window.
fn chromium_root(pt: POINT) -> Option<isize> {
    let root = unsafe { GetAncestor(WindowFromPoint(pt), GA_ROOT) };
    let mut class = [0u16; 64];
    let len = unsafe { GetClassNameW(root, &mut class) } as usize;
    (String::from_utf16_lossy(&class[..len]) == "Chrome_WidgetWin_1").then_some(root.0 as isize)
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

/// A text line cut out of the capture for VieSVTR.
struct Crop {
    px: Vec<u8>,
    w: usize,
    h: usize,
    /// Left edge of the crop in the capture.
    x0: usize,
    /// The line's top and bottom (without the tone-mark padding).
    top: f32,
    bottom: f32,
}

/// Cuts out the line covered by Windows OCR's word `boxes`, padded so tone
/// marks above and dots below the reported box are included.
fn crop_line(px: &[u8], w: usize, h: usize, boxes: &[(Box4, String)]) -> Option<Crop> {
    if boxes.is_empty() {
        return None;
    }
    let (l, t, r, b) = boxes.iter().fold((f32::MAX, f32::MAX, f32::MIN, f32::MIN), |a, (bx, _)| {
        (a.0.min(bx.0), a.1.min(bx.1), a.2.max(bx.2), a.3.max(bx.3))
    });
    let lh = (b - t).max(1.0);
    let cx0 = (l - 0.2 * lh).max(0.0) as usize;
    let cy0 = (t - 0.35 * lh).max(0.0) as usize;
    let cx1 = ((r + 0.2 * lh).ceil() as usize).min(w);
    let cy1 = ((b + 0.2 * lh).ceil() as usize).min(h);
    if cx1 <= cx0 + 1 || cy1 <= cy0 + 1 {
        return None;
    }
    let mut crop = Vec::with_capacity((cx1 - cx0) * (cy1 - cy0) * 4);
    for y in cy0..cy1 {
        crop.extend_from_slice(&px[(y * w + cx0) * 4..(y * w + cx1) * 4]);
    }
    Some(Crop { px: crop, w: cx1 - cx0, h: cy1 - cy0, x0: cx0, top: t, bottom: b })
}

/// Turns VieSVTR's characters for one line into words. Each word's box comes
/// from where in the line its characters were read.
fn words_from_chars(crop: &Crop, chars: &[viet::RecChar], boxes: &[(Box4, String)]) -> Vec<(Box4, String)> {
    let (t, b, cw, cx0) = (crop.top, crop.bottom, crop.w, crop.x0);
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
    for c in chars {
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
    out
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
pub fn capture(r: RECT, out_w: i32, out_h: i32) -> Result<Vec<u8>> {
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

/// Hash of `clip` once two captures in a row agree (starting from the hash of
/// a capture just taken), or `None` if it is still changing after
/// `SETTLE_TIMEOUT`.
fn stable_hash(clip: RECT, first: u64) -> Result<Option<u64>> {
    let (w, h) = (clip.right - clip.left, clip.bottom - clip.top);
    let start = Instant::now();
    let mut prev = first;
    loop {
        std::thread::sleep(SETTLE_STEP);
        let next = hash_pixels(&capture(clip, w, h)?, w, h);
        if next == prev {
            return Ok(Some(next));
        }
        if start.elapsed() >= SETTLE_TIMEOUT {
            return Ok(None);
        }
        prev = next;
    }
}

/// Converts per-line boxes in capture pixels to words in image coordinates.
fn to_words(lines: &[Vec<(Box4, String)>], scale: f32) -> Vec<Word> {
    let mut words = Vec::new();
    for (li, boxes) in lines.iter().enumerate() {
        for (b, text) in boxes {
            let rect = RECT {
                left: (b.0 / scale).floor() as i32,
                top: (b.1 / scale).floor() as i32,
                right: (b.2 / scale).ceil() as i32,
                bottom: (b.3 / scale).ceil() as i32,
            };
            words.push(Word { rect, text: text.clone(), line: li });
        }
    }
    words
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
