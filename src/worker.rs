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
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

use crate::viet;

pub const WM_APP_SHOW: u32 = WM_APP + 1; // lparam = Box<OcrData>
pub const WM_APP_HIDE: u32 = WM_APP + 2;
pub const WM_APP_UPDATE: u32 = WM_APP + 3; // lparam = Box<OcrData>
/// A video under the cursor is playing (lparam = Box<RECT>, its screen rect).
pub const WM_APP_PLAYING: u32 = WM_APP + 5;
/// Two captures this far apart tell a playing video from a paused one (a
/// frame lasts 17-42 ms at 24-60 fps).
const VIDEO_FRAME: Duration = Duration::from_millis(150);
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
/// Unload the Vietnamese model after this long without any request. Loaded
/// it holds 110-175 MB; loading takes about 160 ms and only delays the
/// Vietnamese reading of the next image (its Windows OCR words show first).
const UNLOAD_AFTER: Duration = Duration::from_secs(60);

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
    /// A paused video's frame, not an image (see `textmodel::grab_areas`).
    pub video: bool,
}

impl OcrData {
    fn new(rect: RECT, words: Vec<Word>, id: u64, pending: bool, video: bool) -> Self {
        Self { rect, lines: line_rects(&words), words, id, pending, video }
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
    /// Coarse look of the image when it was read (see `grid`).
    grid: Vec<[u8; 3]>,
    /// For a paused video: the window it plays in, to notice it playing
    /// again when its picture does not move (`video::Sessions`).
    video_window: Option<HWND>,
}

/// What `detect` found under a point.
enum Found {
    /// No image, or one without text.
    Nothing,
    /// An image or a paused video's frame, read and reported; kept to
    /// notice it moving or changing.
    Image(Current),
    /// A video that is playing (its screen rect): read once it pauses.
    PlayingVideo(RECT),
}

/// The Vietnamese model being loaded on another thread.
type Loading = std::thread::JoinHandle<std::result::Result<viet::Recognizer, String>>;

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
    sessions: crate::video::Sessions,
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
            sessions: Default::default(),
        })
    }

    pub fn engine_language(&self) -> String {
        self.engine
            .RecognizerLanguage()
            .and_then(|l| l.LanguageTag())
            .map(|t| t.to_string())
            .unwrap_or_default()
    }

    /// Finds the image (or paused video) under `pt` and OCRs it. `report`
    /// gets the result as soon as Windows OCR has the words (`Stage::Show`,
    /// marked pending when Vietnamese is still being read), then the final
    /// words (`Stage::Update`). `from_cursor` is false for prefetching, where
    /// the cursor is elsewhere.
    fn detect(&mut self, pt: POINT, from_cursor: bool, report: &mut dyn FnMut(Stage, OcrData)) -> Result<Found> {
        let t0 = Instant::now();
        let mut element = unsafe { self.uia.ElementFromPoint(pt)? };
        if unsafe { element.CurrentProcessId()? } == self.own_pid {
            return Ok(Found::Nothing);
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
        // A <video> is a group like any <div>; the tag says which.
        let video = if control_type == UIA_GroupControlTypeId { crate::video::video_at(pt) } else { None };
        let is_video = video.is_some();
        let (clip, tracked) = if control_type == UIA_ImageControlTypeId || is_video {
            let element_rect = unsafe { element.CurrentBoundingRectangle()? };
            // The element UI Automation found may be a box around the video:
            // the video's own rect is what gets read; the element is kept to
            // notice the video moving only when it is the video.
            let (element_rect, tracked) = match video {
                Some(rect) if rect != element_rect => (rect, None),
                _ => (element_rect, Some(element.clone())),
            };
            let Some(clip) = visible_rect(pt, element_rect) else {
                return Ok(Found::Nothing);
            };
            (clip, tracked.map(|e| (e, element_rect)))
        } else if control_type == UIA_PaneControlTypeId && self.is_chromium_without_tree(pt, &element) {
            // Electron app with accessibility turned off (e.g. Zalo): find the image from pixels.
            if from_cursor && !self.ignore_cursor && cursor_is_ibeam() {
                log("pixel mode: cursor is over real text".into());
                return Ok(Found::Nothing);
            }
            let Some(clip) = image_from_pixels(pt)? else {
                return Ok(Found::Nothing);
            };
            (clip, None)
        } else {
            if cfg!(debug_assertions) {
                let class = unsafe { element.CurrentClassName() }.map(|s| s.to_string()).unwrap_or_default();
                log(format!("not an image: control type {} class {class:?}", control_type.0));
            }
            return Ok(Found::Nothing);
        };

        let (w, h) = (clip.right - clip.left, clip.bottom - clip.top);
        // The same pixels read differently in Vietnamese mode.
        let mode = if VIETNAMESE.load(Ordering::Relaxed) { 0x9E37_79B9_7F4A_7C15 } else { 0 };
        let mut pixels = capture(clip, w, h)?;
        let video_window = is_video.then(|| unsafe { GetAncestor(WindowFromPoint(pt), GA_ROOT) });
        if let Some(window) = video_window {
            match self.paused_frame(clip, window, &pixels)? {
                Some(frame) => pixels = frame,
                None => return Ok(Found::PlayingVideo(clip)),
            }
        }
        let mut hash = hash_pixels(&pixels, w, h);
        if !self.cache.contains_key(&(hash ^ mode)) {
            // Not seen before. An image still animating in (viewer zoom, fade)
            // would be read half-drawn, so wait until two captures agree.
            // Cache hits skip this: they matched an image that had settled.
            let Some(settled) = stable_capture(clip, hash)? else {
                log("image never settled".into());
                return Ok(Found::Nothing);
            };
            (hash, pixels) = settled;
        }
        let key = hash ^ mode;
        let current = Current { element: tracked, clip, grid: grid(&pixels, w, h), video_window };
        let t_found = t0.elapsed();

        if let Some(words) = self.cache.get(&key) {
            log(format!("cache hit {w}x{h} in {:?}", t0.elapsed()));
            if !words.is_empty() {
                report(Stage::Show, OcrData::new(clip, words.clone(), key, false, is_video));
            }
            return Ok(Found::Image(current));
        }

        let loading = self.start_loading();
        let (pixels, sw, sh, scale) = self.scaled_for_ocr(clip, pixels)?;
        let t_scaled = t0.elapsed();
        let mut lines = self.windows_lines(&pixels, sw, sh)?;
        if is_video {
            let rows = self.player_rows(&element, clip);
            lines = in_picture(lines, clip, scale, &rows);
        }
        let first = to_words(&lines, scale);
        log(format!(
            "windows ocr {w}x{h}: {} words in {:?} (found {t_found:?}, scaled {:?}, read {:?})",
            first.len(),
            t0.elapsed(),
            t_scaled - t_found,
            t0.elapsed() - t_scaled
        ));
        if first.is_empty() {
            self.finish_loading(loading);
            self.remember(key, Vec::new());
            return Ok(Found::Nothing);
        }
        // The Windows OCR words go out without waiting for the model.
        let vietnamese = VIETNAMESE.load(Ordering::Relaxed) && !self.viet_failed;
        report(Stage::Show, OcrData::new(clip, first.clone(), key, vietnamese, is_video));
        self.finish_loading(loading);

        let words = if vietnamese {
            // Without the model (it failed to load) Windows OCR's words stand;
            // the update still goes out, to end the overlay's pending state.
            let words = match self.viet {
                Some(_) => to_words(&self.read_vietnamese(&pixels, sw, sh, lines), scale),
                None => first,
            };
            log(format!("vietnamese ready in {:?}", t0.elapsed()));
            report(Stage::Update, OcrData::new(clip, words.clone(), key, false, is_video));
            words
        } else {
            first
        };
        self.remember(key, words);
        Ok(Found::Image(current))
    }

    /// The frame of a video that is paused, or `None` while it plays: its
    /// window's media session says so, or its picture changes between two
    /// captures. `first` is a capture of `clip` just taken.
    fn paused_frame(&self, clip: RECT, window: HWND, first: &[u8]) -> Result<Option<Vec<u8>>> {
        if self.sessions.playing_in(&crate::video::window_title(window)) {
            log("video playing (media session)".into());
            return Ok(None);
        }
        std::thread::sleep(VIDEO_FRAME);
        let (w, h) = (clip.right - clip.left, clip.bottom - clip.top);
        let second = capture(clip, w, h)?;
        if second != first {
            log("video playing (picture moves)".into());
            return Ok(None);
        }
        Ok(Some(second))
    }

    /// The rows of the screen (top, bottom) where a video's player shows its
    /// own controls and text over the picture: buttons, the seek bar and the
    /// time beside them, a title, captions. They are the named elements
    /// inside the player (the highest element around `element` that is the
    /// video's size), except panels as tall as a third of the video.
    fn player_rows(&self, element: &IUIAutomationElement, video: RECT) -> Vec<(i32, i32)> {
        let close = |r: RECT| (r.left - video.left).abs() <= 2 && (r.top - video.top).abs() <= 2 && (r.right - video.right).abs() <= 2 && (r.bottom - video.bottom).abs() <= 2;
        let rows = || -> Result<Vec<(i32, i32)>> {
            unsafe {
                let walker = self.uia.RawViewWalker()?;
                let mut player = element.clone();
                for _ in 0..10 {
                    match walker.GetParentElement(&player) {
                        Ok(parent) if close(parent.CurrentBoundingRectangle()?) || !close(player.CurrentBoundingRectangle()?) => player = parent,
                        _ => break,
                    }
                }
                let unnamed = self.uia.CreatePropertyCondition(UIA_NamePropertyId, &VARIANT::from(BSTR::new()))?;
                let named = self.uia.CreateNotCondition(&unnamed)?;
                let found = player.FindAll(TreeScope_Descendants, &named)?;
                let mut rows = Vec::new();
                for i in 0..found.Length()? {
                    let e = found.GetElement(i)?;
                    let r = e.CurrentBoundingRectangle()?;
                    let visible = r.right > r.left && r.bottom > r.top && !e.CurrentIsOffscreen()?.as_bool();
                    let inside = r.left < video.right && r.right > video.left && r.top < video.bottom && r.bottom > video.top;
                    if visible && inside && (r.bottom - r.top) * 3 < video.bottom - video.top {
                        rows.push((r.top, r.bottom));
                    }
                }
                Ok(rows)
            }
        };
        rows().unwrap_or_else(|e| {
            log(format!("video: reading the player's controls failed: {e}"));
            Vec::new()
        })
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

    /// Starts loading the Vietnamese model on its own thread if it is wanted
    /// and not loaded. `detect` does this while Windows OCR reads the image,
    /// so the load (about 160 ms) costs no time; pass the result to
    /// `finish_loading`.
    fn start_loading(&self) -> Option<Loading> {
        if !VIETNAMESE.load(Ordering::Relaxed) || self.viet.is_some() || self.viet_failed {
            return None;
        }
        Some(std::thread::spawn(|| {
            let t0 = Instant::now();
            let model = viet::model_path().ok_or_else(|| "model file not found".to_string()).and_then(|p| viet::Recognizer::load(&p))?;
            log(format!("Vietnamese model loaded in {:?}", t0.elapsed()));
            Ok(model)
        }))
    }

    fn finish_loading(&mut self, loading: Option<Loading>) {
        match loading.map(|l| l.join()) {
            Some(Ok(Ok(model))) => self.viet = Some(model),
            Some(Ok(Err(e))) => {
                log(format!("Vietnamese OCR unavailable: {e}"));
                self.viet_failed = true;
            }
            Some(Err(_)) | None => {}
        }
    }

    /// The Vietnamese recognizer, loaded on first use.
    fn recognizer(&mut self) -> Option<&mut viet::Recognizer> {
        let loading = self.start_loading();
        self.finish_loading(loading);
        self.viet.as_mut()
    }

    /// The image at the size OCR reads best: `pixels` (a capture of `clip`
    /// at its own size) when no scaling is needed, else a scaled capture.
    /// Returns pixels, size and the scale.
    fn scaled_for_ocr(&self, clip: RECT, pixels: Vec<u8>) -> Result<(Vec<u8>, i32, i32, f32)> {
        let w = clip.right - clip.left;
        let h = clip.bottom - clip.top;
        let longest = w.max(h) as f32;
        // Windows OCR is much more accurate on small screen text when upscaled.
        let mut scale: f32 = if longest <= 1600.0 { 2.0 } else { 1.0 };
        scale = scale.min(self.max_dim as f32 / longest);
        let sw = ((w as f32 * scale).round() as i32).max(1);
        let sh = ((h as f32 * scale).round() as i32).max(1);
        if (sw, sh) == (w, h) {
            return Ok((pixels, w, h, scale));
        }
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
        Ok(join_rows(out))
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

    /// True if the image the overlay covers is still where it was and unchanged
    /// (and, for a video, still paused).
    fn still_valid(&self, cur: &Current) -> bool {
        if let Some(window) = cur.video_window {
            if self.sessions.playing_in(&crate::video::window_title(window)) {
                log("revalidate: video playing".into());
                return false;
            }
        }
        if let Some((element, element_rect)) = &cur.element {
            let Ok(rect) = (unsafe { element.CurrentBoundingRectangle() }) else {
                return false;
            };
            if rect != *element_rect {
                log("revalidate: image moved".into());
                return false;
            }
        }
        // Pixel-detected images have no element: scrolling shows up as changed
        // pixels. Only a broad change counts: pages and browsers draw hover
        // buttons on images (Edge's visual search), which must not hide us.
        let w = cur.clip.right - cur.clip.left;
        let h = cur.clip.bottom - cur.clip.top;
        let Ok(px) = capture(cur.clip, w, h) else { return false };
        let now = grid(&px, w, h);
        let changed = now.iter().zip(&cur.grid).filter(|(a, b)| (0..3).map(|i| (a[i] as i32 - b[i] as i32).abs()).sum::<i32>() > 24).count();
        let same = changed * 100 <= cur.grid.len() * 15;
        if !same {
            log(format!("revalidate: image changed ({changed}/{} blocks)", cur.grid.len()));
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
                // Idle: give back what the Vietnamese model holds.
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
                Ok(Found::Image(cur)) => current = Some(cur),
                Ok(Found::PlayingVideo(rect)) => {
                    let ptr = Box::into_raw(Box::new(rect));
                    unsafe {
                        if PostMessageW(Some(hwnd), WM_APP_PLAYING, WPARAM(0), LPARAM(ptr as isize)).is_err() {
                            drop(Box::from_raw(ptr));
                        }
                    }
                }
                Ok(Found::Nothing) => {}
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
                    Ok(Found::Image(cur)) => {
                        let c = cur.clip;
                        log(format!("prefetch at ({}, {}): cached, clip ({}, {}) - ({}, {})", pt.x, pt.y, c.left, c.top, c.right, c.bottom));
                        break;
                    }
                    Ok(_) => log(format!("prefetch at ({}, {}): nothing yet", pt.x, pt.y)),
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

/// Prints what UI Automation and IAccessible2 report for the element at a
/// screen point and its ancestors (a debugging aid for apps whose images or
/// videos are not found).
pub fn print_uia(pt: POINT) -> Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
        let walker = uia.RawViewWalker()?;
        println!("IAccessible2 attributes: {:?}", crate::video::attributes_at(pt));
        let window = GetAncestor(WindowFromPoint(pt), GA_ROOT);
        let title = crate::video::window_title(window);
        println!("window title {title:?}, its media playing: {}", crate::video::Sessions::default().playing_in(&title));
        let mut element = Some(uia.ElementFromPoint(pt)?);
        for depth in 0..6 {
            let Some(e) = element else { break };
            println!(
                "{depth}: type {} '{}' aria-role '{}' aria '{}' name '{}' id '{}' class '{}' rect {:?}",
                e.CurrentControlType()?.0,
                e.CurrentLocalizedControlType().unwrap_or_default(),
                e.CurrentAriaRole().unwrap_or_default(),
                e.CurrentAriaProperties().unwrap_or_default(),
                e.CurrentName().unwrap_or_default(),
                e.CurrentAutomationId().unwrap_or_default(),
                e.CurrentClassName().unwrap_or_default(),
                e.CurrentBoundingRectangle()?,
            );
            if let Ok(c) = walker.GetFirstChildElement(&e) {
                println!("   first child: type {} aria-role '{}' name '{}'", c.CurrentControlType()?.0, c.CurrentAriaRole().unwrap_or_default(), c.CurrentName().unwrap_or_default());
            }
            element = walker.GetParentElement(&e).ok();
        }
    }
    Ok(())
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
        (Found::Image(_), Some(data)) => {
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
        (Found::PlayingVideo(_), _) => println!("a video is playing at ({}, {})", pt.x, pt.y),
        _ => println!("no image with text at ({}, {})", pt.x, pt.y),
    }
    Ok(())
}

/// OCRs an image file and prints the text and word boxes. Used for testing
/// without driving the screen.
pub fn ocr_file(path: &str) -> Result<()> {
    let t0 = Instant::now();
    let (w, h, words) = read_file(path)?;
    println!("{}x{}, {} words in {:?}", w, h, words.len(), t0.elapsed());
    let all: Vec<usize> = (0..words.len()).collect();
    println!("--- text ---\n{}\n--- word boxes ---", crate::selection_text(&words, &all));
    for word in &words {
        let b = word.rect;
        println!("line {} x {}..{} y {}..{} {}", word.line, b.left, b.right, b.top, b.bottom, word.text);
    }
    Ok(())
}

/// The words in an image file (upscaled 2x like screen captures), in the
/// image's own pixels, with its size.
pub fn read_file(path: &str) -> Result<(u32, u32, Vec<Word>)> {
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
    let words = detector.ocr_pixels(&pixels, (w * 2) as i32, (h * 2) as i32, 2.0)?;
    Ok((w, h, words))
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

/// Puts back together what Windows OCR reports as separate lines but is one
/// line on screen: pieces on one row a few characters apart (the text around
/// an inline code chip, a line of code cut at a space), or overlapping (a
/// long line it read in two halves, with the word at the cut in both).
/// Pieces further apart stay lines of their own: table cells, columns, a
/// label and its value.
fn join_rows(lines: Vec<Vec<(Box4, String)>>) -> Vec<Vec<(Box4, String)>> {
    let bounds = line_bounds;
    let mut out: Vec<Vec<(Box4, String)>> = Vec::new();
    for line in lines.into_iter().filter(|l| !l.is_empty()) {
        let b = bounds(&line);
        let same_line = out.iter_mut().find(|other| {
            let o = bounds(other);
            let (h, oh) = (b.3 - b.1, o.3 - o.1);
            let shared = b.3.min(o.3) - b.1.max(o.1);
            // Negative when the two overlap sideways.
            let gap = (b.0 - o.2).max(o.0 - b.2);
            shared > 0.6 * h.min(oh) && gap < 1.5 * h.max(oh)
        });
        match same_line {
            Some(other) => {
                other.extend(line);
                other.sort_by(|p, q| p.0.0.total_cmp(&q.0.0));
                // A word read twice at the cut: keep the wider reading.
                let mut i = 1;
                while i < other.len() {
                    let (p, q) = (other[i - 1].0, other[i].0);
                    let (pw, qw) = (p.2 - p.0, q.2 - q.0);
                    if p.2.min(q.2) - p.0.max(q.0) > 0.5 * pw.min(qw) {
                        other.remove(if pw < qw { i - 1 } else { i });
                    } else {
                        i += 1;
                    }
                }
            }
            None => out.push(line),
        }
    }
    out
}

/// The lines that are part of a video's picture. Players draw their own
/// controls, time, title and captions over the video as page text; lines on
/// the rows they occupy (`player_rows`) are dropped, so the overlay never
/// takes the player's clicks and the page's own text stays the page's.
fn in_picture(lines: Vec<Vec<(Box4, String)>>, clip: RECT, scale: f32, rows: &[(i32, i32)]) -> Vec<Vec<(Box4, String)>> {
    let before = lines.len();
    let kept: Vec<_> = lines
        .into_iter()
        .filter(|line| {
            let (_, t, _, b) = line_bounds(line);
            let middle = clip.top + ((t + b) / 2.0 / scale) as i32;
            !rows.iter().any(|&(top, bottom)| (top..=bottom).contains(&middle))
        })
        .collect();
    if kept.len() < before {
        log(format!("video: {} of {before} lines are the player's own text", before - kept.len()));
    }
    kept
}

/// The box around a line's words.
fn line_bounds(line: &[(Box4, String)]) -> Box4 {
    line.iter().fold((f32::MAX, f32::MAX, f32::MIN, f32::MIN), |a, (b, _)| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
}

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
    // CTC spans end at a character's peak, short of its ink: words boxed by
    // them were highlighted a few pixels short, and a drag starting on a
    // word's first letter missed it. Windows OCR's boxes are the outlines.
    // With the same number of words on both sides they are used as they are.
    // Otherwise each gap between two words here is moved onto the gap
    // Windows OCR measured there (the two engines rarely disagree on more
    // than a split or two per line), or failing that the words meet halfway.
    // The outer words reach the line's ends as Windows OCR saw them, unless
    // it read something there that this reading lacks (a bullet, a checkbox).
    if out.len() == boxes.len() {
        for (o, (b, _)) in out.iter_mut().zip(boxes) {
            o.0 = *b;
        }
    } else if let (Some(first), Some(last)) = (out.first(), out.last()) {
        // The spans fall at most about half a line height short.
        let reach = 0.6 * (b - t).max(1.0);
        let (line_left, line_right) = boxes.iter().fold((f32::MAX, f32::MIN), |a, (bx, _)| (a.0.min(bx.0), a.1.max(bx.2)));
        let left = if first.0.0 - line_left < reach { line_left } else { first.0.0 };
        let right = if line_right - last.0.2 < reach { line_right } else { last.0.2 };
        // (right edge of the word before, left edge of the word after)
        let measured: Vec<(f32, f32)> = boxes.windows(2).map(|pair| (pair[0].0.2, pair[1].0.0)).filter(|(r, l)| l >= r).collect();
        let mut edges = vec![(left, left)];
        edges.extend(out.windows(2).map(|pair| {
            let middle = (pair[0].0.2 + pair[1].0.0) / 2.0;
            let off = |gap: &(f32, f32)| (gap.0 - middle).max(middle - gap.1).max(0.0);
            measured.iter().copied().min_by(|p, q| off(p).total_cmp(&off(q))).filter(|gap| off(gap) < reach).unwrap_or((middle, middle))
        }));
        edges.push((right, right));
        for (o, edge) in out.iter_mut().zip(edges.windows(2)) {
            (o.0.0, o.0.2) = (edge[0].1, edge[1].0.max(edge[0].1 + 1.0));
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
///
/// What it costs is reading the screen, by source area: about 6 ms for
/// 640x400 and 18 ms for 1920x1200, the same whether the output is full
/// size or a thumbnail (`bench_capture_paths`). Scaling up is extra: 36 ms
/// for 1280x800 at 2x.
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

/// The capture of `clip`, with its hash, once two captures in a row agree
/// (starting from the hash of a capture just taken), or `None` if it is
/// still changing after `SETTLE_TIMEOUT`.
fn stable_capture(clip: RECT, first: u64) -> Result<Option<(u64, Vec<u8>)>> {
    let (w, h) = (clip.right - clip.left, clip.bottom - clip.top);
    let start = Instant::now();
    let mut prev = first;
    loop {
        std::thread::sleep(SETTLE_STEP);
        let pixels = capture(clip, w, h)?;
        let next = hash_pixels(&pixels, w, h);
        if next == prev {
            return Ok(Some((next, pixels)));
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

/// Mean color of each cell of a 16x16 grid over a BGRA capture.
fn grid(px: &[u8], w: i32, h: i32) -> Vec<[u8; 3]> {
    const N: i32 = 16;
    let mut out = Vec::with_capacity((N * N) as usize);
    for gy in 0..N {
        for gx in 0..N {
            let (x0, x1) = (gx * w / N, ((gx + 1) * w / N).max(gx * w / N + 1).min(w));
            let (y0, y1) = (gy * h / N, ((gy + 1) * h / N).max(gy * h / N + 1).min(h));
            let mut sum = [0u64; 3];
            let mut n = 0u64;
            for y in (y0..y1).step_by(2) {
                for x in (x0..x1).step_by(2) {
                    let i = ((y * w + x) * 4) as usize;
                    sum[0] += px[i + 2] as u64;
                    sum[1] += px[i + 1] as u64;
                    sum[2] += px[i] as u64;
                    n += 1;
                }
            }
            let n = n.max(1);
            out.push([(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8]);
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(l: f32, t: f32, r: f32, b: f32, text: &str) -> (Box4, String) {
        ((l, t, r, b), text.to_string())
    }

    fn texts(lines: &[Vec<(Box4, String)>]) -> Vec<String> {
        lines.iter().map(|l| l.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join(" ")).collect()
    }

    #[test]
    fn pieces_of_one_line_are_joined_but_columns_are_not() {
        let lines = vec![
            // A line cut at a space, its second piece reported later.
            vec![piece(135.0, 130.0, 434.0, 144.0, "order.id,")],
            vec![piece(105.0, 174.0, 326.0, 188.0, "total")],
            vec![piece(443.0, 130.0, 542.0, 144.0, "order.price);")],
            // Two labels on one row, three line heights apart.
            vec![piece(65.0, 480.0, 250.0, 497.0, "created")],
            vec![piece(296.0, 480.0, 467.0, 497.0, "assigned")],
        ];
        assert_eq!(texts(&join_rows(lines)), ["order.id, order.price);", "total", "created", "assigned"]);
    }

    #[test]
    fn a_word_read_twice_where_a_long_line_was_cut_is_kept_once() {
        let lines = vec![
            vec![piece(366.0, 482.0, 700.0, 496.0, "pwsh"), piece(760.0, 482.0, 782.0, 496.0, "-Fi")],
            vec![piece(763.0, 482.0, 800.0, 496.0, "-File"), piece(810.0, 482.0, 900.0, 496.0, "run.ps1")],
        ];
        assert_eq!(texts(&join_rows(lines)), ["pwsh -File run.ps1"]);
    }

    #[test]
    fn vietnamese_words_cover_the_line_when_the_engines_split_it_differently() {
        // Windows OCR: three words from 100 to 400. The model: two words,
        // its spans a few pixels short at every end.
        let crop = Crop { px: Vec::new(), w: 320, h: 20, x0: 90, top: 10.0, bottom: 26.0 };
        let chars: Vec<viet::RecChar> = [('a', 0.04, 0.28), (' ', 0.3, 0.3), ('b', 0.33, 0.94)].iter().map(|&(ch, x0, x1)| viet::RecChar { ch, x0, x1 }).collect();
        let windows = [piece(100.0, 10.0, 180.0, 26.0, "x"), piece(190.0, 10.0, 220.0, 26.0, "y"), piece(230.0, 10.0, 400.0, 26.0, "z")];
        let words = words_from_chars(&crop, &chars, &windows);
        assert_eq!(words.len(), 2);
        // The ends reach the line's ends, and the gap between the two words
        // is the one Windows OCR measured nearest to it.
        assert_eq!((words[0].0.0, words[0].0.2), (100.0, 180.0));
        assert_eq!((words[1].0.0, words[1].0.2), (190.0, 400.0));
    }
}

/// Timings and memory on the real screen and services, to rerun before
/// changing how the pipeline captures, renders or loads:
///   cargo test --release bench_ -- --ignored --nocapture
/// The Vietnamese stages need the test exe copied somewhere under the
/// project folder (it looks for `models/` in its parent folders).
#[cfg(test)]
mod bench {
    use super::*;

    fn median_ms(mut f: impl FnMut()) -> (f64, f64, f64) {
        let mut t: Vec<f64> = (0..15)
            .map(|_| {
                let t0 = Instant::now();
                f();
                t0.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        t.sort_by(f64::total_cmp);
        (t[t.len() / 2], t[0], t[t.len() - 1])
    }

    /// Private bytes and working set, in MB.
    fn memory() -> (f64, f64) {
        use windows::Win32::System::ProcessStatus::*;
        use windows::Win32::System::Threading::GetCurrentProcess;
        let mut c = PROCESS_MEMORY_COUNTERS_EX::default();
        unsafe {
            let _ = GetProcessMemoryInfo(GetCurrentProcess(), (&mut c as *mut PROCESS_MEMORY_COUNTERS_EX).cast(), size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32);
        }
        (c.PrivateUsage as f64 / 1048576.0, c.WorkingSetSize as f64 / 1048576.0)
    }

    /// What Direct2D and DirectWrite hold on to (see `paint::RENDER_TARGET`).
    #[test]
    #[ignore = "memory"]
    fn bench_d2d_memory() {
        let stage = |name: &str| {
            std::thread::sleep(Duration::from_millis(300));
            let (private, ws) = memory();
            println!("{name:<44} private {private:7.1} MB   working set {ws:7.1} MB");
        };
        stage("start");
        std::hint::black_box(crate::paint::measure("Sao chép Ctrl+C", 12.0, 400));
        stage("after DirectWrite measure");
        let t0 = Instant::now();
        let ink = crate::style::Ink { body: 8.0, baseline: 0.0, stroke: 1.4 };
        std::hint::black_box(crate::paint::match_style("Sao chép", ink).is_some());
        println!("first Direct2D draw (10 renders): {:?}", t0.elapsed());
        stage("after Direct2D DC render target");
        let t0 = Instant::now();
        std::hint::black_box(crate::paint::match_style("Sao chép", ink).is_some());
        println!("10 more renders: {:?}", t0.elapsed());
        stage("after more Direct2D");
    }
    /// What each part of the pipeline holds on to, stage by stage.
    #[test]
    #[ignore = "memory on the real screen"]
    fn bench_memory() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let stage = |name: &str| {
            std::thread::sleep(Duration::from_millis(300));
            let (private, ws) = memory();
            println!("{name:<44} private {private:7.1} MB   working set {ws:7.1} MB");
        };
        stage("start");
        let mut d = Detector::new().unwrap();
        stage("detector created (UIA + OCR engine)");
        let (w, h) = (1280, 800);
        let clip = RECT { left: 0, top: 0, right: w, bottom: h };
        let px = capture(clip, w * 2, h * 2).unwrap();
        let t0 = Instant::now();
        let lines = d.windows_lines(&px, w * 2, h * 2).unwrap();
        println!("first OCR: {} lines in {:?}", lines.len(), t0.elapsed());
        stage("after first Windows OCR");
        let t0 = Instant::now();
        for _ in 0..3 {
            d.windows_lines(&px, w * 2, h * 2).unwrap();
        }
        println!("3 more OCRs: {:?} each", t0.elapsed() / 3);
        stage("after 4 Windows OCRs");
        let t0 = Instant::now();
        d.engine = make_engine().unwrap();
        println!("engine recreated in {:?}", t0.elapsed());
        stage("engine dropped and recreated");
        let t0 = Instant::now();
        d.windows_lines(&px, w * 2, h * 2).unwrap();
        println!("first OCR on the new engine: {:?}", t0.elapsed());
        stage("after OCR on the new engine");
        let _ = unsafe { d.uia.ElementFromPoint(POINT { x: 600, y: 400 }) };
        stage("after a UIA query");

        let t0 = Instant::now();
        let loaded = d.recognizer().is_some();
        println!("Vietnamese model loaded: {loaded} in {:?}", t0.elapsed());
        stage("Vietnamese model loaded");
        let t0 = Instant::now();
        let read = d.read_vietnamese(&px, (w * 2) as i32, (h * 2) as i32, lines.clone());
        println!("Vietnamese read {} lines in {:?}", read.len(), t0.elapsed());
        stage("after one Vietnamese read");
        for i in 2..=5 {
            let t0 = Instant::now();
            d.read_vietnamese(&px, (w * 2) as i32, (h * 2) as i32, lines.clone());
            println!("Vietnamese read {i}: {:?}", t0.elapsed());
            stage("after another Vietnamese read");
        }
        d.viet = None;
        stage("Vietnamese model dropped");
        drop(px);
        drop(d);
        stage("detector dropped");
    }
    #[test]
    #[ignore = "timing on the real screen"]
    fn bench_capture_paths() {
        for (w, h) in [(640, 400), (1280, 800), (1920, 1200)] {
            let clip = RECT { left: 0, top: 0, right: w, bottom: h };
            let px = capture(clip, w, h).unwrap();
            let row = |name: &str, (m, lo, hi): (f64, f64, f64)| println!("{w}x{h} {name:<28} {m:7.3} ms  ({lo:.3} .. {hi:.3})");
            row("capture full", median_ms(|| drop(capture(clip, w, h).unwrap())));
            row("capture 2x halftone", median_ms(|| drop(capture(clip, w * 2, h * 2).unwrap())));
            row("capture 1/4", median_ms(|| drop(capture(clip, w / 4, h / 4).unwrap())));
            row("capture 64x64", median_ms(|| drop(capture(clip, 64, 64).unwrap())));
            row("hash (sip)", median_ms(|| { std::hint::black_box(hash_pixels(&px, w, h)); }));
            row("grid", median_ms(|| drop(std::hint::black_box(grid(&px, w, h)))));
        }
    }
}
