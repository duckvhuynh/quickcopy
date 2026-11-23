//! QuickCopy prototype: hover an image for a moment, then double-click a word
//! or drag across text inside it to copy that text. English OCR only.
//!
//! Usage:
//!   quickcopy                 run in the tray
//!   quickcopy --probe X Y     OCR the image at screen point (X, Y) and print it

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod rectfind;
mod viet;
mod worker;

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

use worker::{OcrData, Request, WM_APP_HIDE, WM_APP_SHOW, WM_APP_UPDATE, Word, log};

const WM_APP_TRAY: u32 = WM_APP + 10;
const TICK_TIMER: usize = 1;
const FLASH_TIMER: usize = 2;
/// Cursor polling period; it bounds how late the dwell is noticed.
const TICK_MS: u32 = 20;
/// How long the cursor must rest before we look for an image under it.
/// Repeat visits are cache hits, so this is most of the wait on them.
const DWELL: Duration = Duration::from_millis(150);
/// Extra detection attempts while the cursor rests and nothing was found.
const DETECT_RETRIES: u8 = 2;
const DETECT_RETRY: Duration = Duration::from_millis(800);
/// How often we check that the covered image has not moved or changed.
const REVALIDATE: Duration = Duration::from_millis(300);
const FLASH_MS: u32 = 450;
const MENU_PAUSE: usize = 100;
const MENU_QUIT: usize = 101;
const MENU_VIETNAMESE: usize = 102;

#[derive(PartialEq)]
enum Drag {
    None,
    Pending, // button down, not moved yet
    Active,
}

struct App {
    main: HWND,
    overlay: HWND,
    tx: Sender<Request>,
    paused: bool,
    last_pos: POINT,
    still_since: Instant,
    requested: bool,
    requested_at: Instant,
    retries: u8,
    data: Option<OcrData>,
    last_revalidate: Instant,
    drag: Drag,
    down_pt: POINT,
    sel: Option<(usize, usize)>,
    /// Where the selection was made (start, end, whole line), so it can be
    /// found again when the Vietnamese reading replaces the words.
    sel_src: Option<(POINT, POINT, bool)>,
    /// The user asked to copy before the Vietnamese reading arrived.
    pending_copy: bool,
    flashing: bool,
    last_dblclk: Option<(Instant, POINT)>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs `f` with the app state. Never call this re-entrantly: keep calls that
/// pump messages (menus) outside of it.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut a| a.as_mut().map(f)))
}

fn main() -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    // Vietnamese reading is on whenever the model is installed
    // (QC_ENGLISH=1 forces Windows OCR, for comparing in --probe).
    let vietnamese = viet::model_path().is_some() && std::env::var_os("QC_ENGLISH").is_none();
    worker::VIETNAMESE.store(vietnamese, Ordering::Relaxed);

    let args: Vec<String> = std::env::args().collect();
    if args.len() == 3 && args[1] == "--ocr-file" {
        return worker::ocr_file(&args[2]);
    }
    if args.len() == 4 && args[1] == "--probe" {
        let x = args[2].parse().expect("X must be an integer");
        let y = args[3].parse().expect("Y must be an integer");
        return worker::probe(POINT { x, y });
    }

    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let main_class = w!("QuickCopyMain");
        let overlay_class = w!("QuickCopyOverlay");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(main_proc),
            hInstance: instance,
            lpszClassName: main_class,
            ..Default::default()
        });
        RegisterClassW(&WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(overlay_proc),
            hInstance: instance,
            lpszClassName: overlay_class,
            hCursor: LoadCursorW(None, IDC_IBEAM)?,
            ..Default::default()
        });

        let main = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            main_class,
            w!("QuickCopy"),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance),
            None,
        )?;
        let overlay = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            overlay_class,
            w!("QuickCopy Overlay"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance),
            None,
        )?;
        // Keep our own overlay out of screen captures so revalidation hashes stay stable.
        let _ = SetWindowDisplayAffinity(overlay, WDA_EXCLUDEFROMCAPTURE);

        let (tx, rx) = channel();
        let main_raw = main.0 as isize;
        std::thread::spawn(move || worker::run(rx, main_raw));

        APP.with(|a| {
            *a.borrow_mut() = Some(App {
                main,
                overlay,
                tx,
                paused: false,
                last_pos: POINT::default(),
                still_since: Instant::now(),
                requested: false,
                requested_at: Instant::now(),
                retries: 0,
                data: None,
                last_revalidate: Instant::now(),
                drag: Drag::None,
                down_pt: POINT::default(),
                sel: None,
                sel_src: None,
                pending_copy: false,
                flashing: false,
                last_dblclk: None,
            })
        });

        add_tray_icon(main);
        SetTimer(Some(main), TICK_TIMER, TICK_MS, None);
        // Read images in newly activated windows (an image viewer opening)
        // before the cursor gets to them.
        let _ = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(on_foreground),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        log("running; hover an image, then double-click or drag over its text".into());

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        remove_tray_icon(main);
    }
    Ok(())
}

// ---------------------------------------------------------------- main window

unsafe extern "system" fn main_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER if wp.0 == TICK_TIMER => {
            with_app(tick);
            LRESULT(0)
        }
        WM_APP_SHOW => {
            let data = unsafe { *Box::from_raw(lp.0 as *mut OcrData) };
            with_app(|app| show(app, data));
            LRESULT(0)
        }
        WM_APP_UPDATE => {
            let data = unsafe { *Box::from_raw(lp.0 as *mut OcrData) };
            with_app(|app| apply_update(app, data));
            LRESULT(0)
        }
        WM_APP_HIDE => {
            with_app(|app| {
                if app.drag == Drag::None {
                    hide(app);
                    app.still_since = Instant::now();
                    app.requested = false;
                }
            });
            LRESULT(0)
        }
        WM_APP_TRAY => {
            let event = (lp.0 & 0xFFFF) as u32;
            if event == WM_RBUTTONUP || event == WM_CONTEXTMENU {
                unsafe { tray_menu(hwnd) };
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// A window came to the front: prefetch the image at its center, if any, so
/// it is already read when the cursor reaches it.
unsafe extern "system" fn on_foreground(_: HWINEVENTHOOK, _: u32, hwnd: HWND, _: i32, _: i32, _: u32, _: u32) {
    let mut r = RECT::default();
    if hwnd.is_invalid() || unsafe { GetWindowRect(hwnd, &mut r) }.is_err() {
        return;
    }
    let center = POINT { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 };
    // Only if that point really shows this window (not covered, not offscreen).
    let at = unsafe { GetAncestor(WindowFromPoint(center), GA_ROOT) };
    if at != unsafe { GetAncestor(hwnd, GA_ROOT) } {
        return;
    }
    with_app(|app| {
        if !app.paused {
            let _ = app.tx.send(Request::Prefetch(center));
        }
    });
}

fn tick(app: &mut App) {
    let mut pos = POINT::default();
    if unsafe { GetCursorPos(&mut pos) }.is_err() {
        return;
    }
    let now = Instant::now();

    if let Some(data) = &app.data {
        // Keep the overlay while a selection is in progress or a copy waits
        // for the Vietnamese reading, even if the cursor wanders off.
        if app.drag == Drag::None && !app.flashing && !app.pending_copy {
            if !contains(data.rect, pos) {
                log("hide: cursor left the image".into());
                hide(app);
                let _ = app.tx.send(Request::Forget);
            } else if now - app.last_revalidate >= REVALIDATE {
                app.last_revalidate = now;
                let _ = app.tx.send(Request::Revalidate);
            }
        }
    }

    if pos != app.last_pos {
        app.last_pos = pos;
        app.still_since = now;
        app.requested = false;
        app.retries = 0;
        return;
    }
    let button_down = unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } < 0;
    if app.paused || app.data.is_some() || button_down || now - app.still_since < DWELL {
        return;
    }
    // Ask once when the cursor settles, then a couple more times while it
    // stays put: a page or viewer may still be loading, and a fresh Chrome
    // window answers the first accessibility query before its tree exists.
    let retry_due = app.requested && app.retries < DETECT_RETRIES && now - app.requested_at >= DETECT_RETRY;
    if !app.requested || retry_due {
        if app.requested {
            app.retries += 1;
        }
        app.requested = true;
        app.requested_at = now;
        let _ = app.tx.send(Request::Detect(pos));
    }
}

fn show(app: &mut App, data: OcrData) {
    let mut pos = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pos);
    }
    // The cursor moved off the image while OCR was running.
    if app.paused || !contains(data.rect, pos) || app.data.is_some() {
        return;
    }
    let r = data.rect;
    log(format!("overlay {}x{} at ({}, {}), {} words", r.right - r.left, r.bottom - r.top, r.left, r.top, data.words.len()));
    app.data = Some(data);
    app.sel = None;
    app.drag = Drag::None;
    app.last_revalidate = Instant::now();
    render(app);
    unsafe {
        let _ = SetWindowPos(
            app.overlay,
            Some(HWND_TOPMOST),
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}

fn hide(app: &mut App) {
    if app.data.take().is_some() {
        unsafe {
            let _ = ShowWindow(app.overlay, SW_HIDE);
            let _ = KillTimer(Some(app.overlay), FLASH_TIMER);
        }
    }
    app.sel = None;
    app.sel_src = None;
    app.pending_copy = false;
    app.drag = Drag::None;
    app.flashing = false;
}

/// The Vietnamese reading arrived for the image on screen: swap the words in,
/// find the selection again by where it was made, and finish a waiting copy.
fn apply_update(app: &mut App, data: OcrData) {
    if app.data.as_ref().is_none_or(|d| d.id != data.id) {
        return;
    }
    app.data = Some(data);
    if let Some((a, b, line)) = app.sel_src {
        select_points(app, a, b, line);
    }
    if std::mem::take(&mut app.pending_copy) {
        copy_selection(app);
    } else {
        render(app);
    }
}

/// Selects the words from `a` to `b` (overlay coordinates), or the whole line
/// at `a`, and remembers the points so the selection survives a word update.
fn select_points(app: &mut App, a: POINT, b: POINT, line: bool) {
    let Some(data) = &app.data else { return };
    let (Some(ia), Some(ib)) = (nearest_word(data, a), nearest_word(data, b)) else { return };
    app.sel = Some(if line {
        let l = data.words[ia].line;
        let first = data.words.iter().position(|w| w.line == l).unwrap_or(ia);
        let last = data.words.iter().rposition(|w| w.line == l).unwrap_or(ia);
        (first, last)
    } else {
        (ia.min(ib), ia.max(ib))
    });
    app.sel_src = Some((a, b, line));
}

// ------------------------------------------------------------- overlay window

unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let pt = POINT {
        x: (lp.0 & 0xFFFF) as i16 as i32,
        y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32,
    };
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_LBUTTONDOWN => {
            with_app(|app| on_button_down(app, pt));
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            with_app(|app| on_mouse_move(app, pt));
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            with_app(on_button_up);
            LRESULT(0)
        }
        WM_LBUTTONDBLCLK => {
            with_app(|app| on_double_click(app, pt));
            LRESULT(0)
        }
        WM_TIMER if wp.0 == FLASH_TIMER => {
            with_app(|app| {
                unsafe {
                    let _ = KillTimer(Some(app.overlay), FLASH_TIMER);
                }
                app.flashing = false;
                app.sel = None;
                app.sel_src = None;
                render(app);
            });
            LRESULT(0)
        }
        // Hand wheel and right-click back to the app underneath.
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let flags = if msg == WM_MOUSEWHEEL { MOUSEEVENTF_WHEEL } else { MOUSEEVENTF_HWHEEL };
            let delta = ((wp.0 >> 16) & 0xFFFF) as i16 as i32;
            with_app(|app| {
                hide(app);
                let _ = app.tx.send(Request::Forget);
                app.still_since = Instant::now();
                app.requested = false;
            });
            send_mouse(flags, delta);
            LRESULT(0)
        }
        WM_RBUTTONDOWN => {
            with_app(|app| {
                hide(app);
                let _ = app.tx.send(Request::Forget);
            });
            send_mouse(MOUSEEVENTF_RIGHTDOWN, 0);
            send_mouse(MOUSEEVENTF_RIGHTUP, 0);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

fn on_button_down(app: &mut App, pt: POINT) {
    if app.data.is_none() {
        return;
    }

    // Third click right after a double-click selects the whole line.
    if let Some((t, p)) = app.last_dblclk.take() {
        let dbl = Duration::from_millis(unsafe { GetDoubleClickTime() } as u64);
        if t.elapsed() <= dbl && (p.x - pt.x).abs() <= 4 && (p.y - pt.y).abs() <= 4 {
            select_points(app, pt, pt, true);
            copy_selection(app);
            return;
        }
    }

    app.down_pt = pt;
    app.drag = Drag::Pending;
    unsafe {
        SetCapture(app.overlay);
    }
}

fn on_mouse_move(app: &mut App, pt: POINT) {
    if app.drag == Drag::None {
        return;
    }
    if app.drag == Drag::Pending {
        if (pt.x - app.down_pt.x).abs() <= 3 && (pt.y - app.down_pt.y).abs() <= 3 {
            return;
        }
        app.drag = Drag::Active;
        app.flashing = false;
    }
    let before = app.sel;
    select_points(app, app.down_pt, pt, false);
    if app.sel != before {
        render(app);
    }
}

fn on_button_up(app: &mut App) {
    if app.drag == Drag::None {
        return;
    }
    let dragged = app.drag == Drag::Active;
    app.drag = Drag::None;
    unsafe {
        let _ = ReleaseCapture();
    }
    if dragged && app.sel.is_some() {
        copy_selection(app);
    }
}

fn on_double_click(app: &mut App, pt: POINT) {
    select_points(app, pt, pt, false);
    app.last_dblclk = Some((Instant::now(), pt));
    copy_selection(app);
}

fn copy_selection(app: &mut App) {
    let (Some(data), Some((a, b))) = (&app.data, app.sel) else { return };
    if data.pending {
        // Copy once the Vietnamese reading arrives (apply_update).
        log("copy waits for the Vietnamese reading".into());
        app.pending_copy = true;
        render(app);
        return;
    }
    let indices: Vec<usize> = (a..=b).collect();
    let text = selection_text(&data.words, &indices);
    match set_clipboard(app.main, &text) {
        Ok(()) => log(format!("copied: {text:?}")),
        Err(e) => log(format!("clipboard error: {e}")),
    }
    app.flashing = true;
    render(app);
    unsafe {
        SetTimer(Some(app.overlay), FLASH_TIMER, FLASH_MS, None);
    }
}

/// Joins words with spaces, and lines with newlines.
pub fn selection_text(words: &[Word], indices: &[usize]) -> String {
    let mut out = String::new();
    let mut prev_line = None;
    for &i in indices {
        let w = &words[i];
        match prev_line {
            Some(l) if l != w.line => out.push('\n'),
            Some(_) => out.push(' '),
            None => {}
        }
        out.push_str(&w.text);
        prev_line = Some(w.line);
    }
    out
}

/// The word under `pt`, or else the closest word on the closest line.
fn nearest_word(data: &OcrData, pt: POINT) -> Option<usize> {
    if let Some(i) = data.words.iter().position(|w| contains(inflate(w.rect, 2, 3), pt)) {
        return Some(i);
    }
    let line = (0..data.lines.len()).min_by_key(|&l| axis_dist(data.lines[l].top, data.lines[l].bottom, pt.y))?;
    data.words
        .iter()
        .enumerate()
        .filter(|(_, w)| w.line == line)
        .min_by_key(|(_, w)| axis_dist(w.rect.left, w.rect.right, pt.x))
        .map(|(i, _)| i)
}

fn axis_dist(lo: i32, hi: i32, v: i32) -> i32 {
    if v < lo {
        lo - v
    } else if v > hi {
        v - hi
    } else {
        0
    }
}

// ------------------------------------------------------------------ drawing

/// Premultiplied BGRA pixel.
fn pixel(r: u32, g: u32, b: u32, a: u32) -> u32 {
    (a << 24) | ((r * a / 255) << 16) | ((g * a / 255) << 8) | (b * a / 255)
}

/// Redraws the overlay. Text lines get alpha 1 (invisible but clickable);
/// everything else has alpha 0, so clicks there fall through to the app below.
fn render(app: &App) {
    let Some(data) = &app.data else { return };
    let w = data.rect.right - data.rect.left;
    let h = data.rect.bottom - data.rect.top;
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let Ok(dib) = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
            return;
        };
        let px = std::slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize);
        px.fill(0);

        let hit = pixel(0, 0, 0, 1);
        for line in &data.lines {
            fill(px, w, h, hit_area(*line), hit);
        }
        if let Some((a, b)) = app.sel {
            let color = if app.flashing { pixel(0x22, 0xC5, 0x5E, 0x70) } else { pixel(0x26, 0x84, 0xFF, 0x60) };
            // One highlight band per line, spanning its selected words.
            let mut band: Option<(usize, RECT)> = None;
            for word in &data.words[a..=b] {
                band = match band {
                    Some((line, r)) if line == word.line => Some((line, union(r, word.rect))),
                    Some((_, r)) => {
                        fill(px, w, h, inflate(r, 1, 2), color);
                        Some((word.line, word.rect))
                    }
                    None => Some((word.line, word.rect)),
                };
            }
            if let Some((_, r)) = band {
                fill(px, w, h, inflate(r, 1, 2), color);
            }
        }

        let old = SelectObject(mem, dib.into());
        let dst = POINT { x: data.rect.left, y: data.rect.top };
        let size = SIZE { cx: w, cy: h };
        let src = POINT::default();
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let _ = UpdateLayeredWindow(
            app.overlay,
            Some(screen),
            Some(&dst),
            Some(&size),
            Some(mem),
            Some(&src),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
        SelectObject(mem, old);
        let _ = DeleteObject(dib.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
    }
}

fn fill(px: &mut [u32], w: i32, h: i32, r: RECT, color: u32) {
    let (l, t) = (r.left.max(0), r.top.max(0));
    let (rr, b) = (r.right.min(w), r.bottom.min(h));
    for y in t..b {
        let row = (y * w) as usize;
        px[row + l as usize..row + rr.max(l) as usize].fill(color);
    }
}

// ------------------------------------------------------------------ helpers

fn contains(r: RECT, p: POINT) -> bool {
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}

/// Where a line accepts the mouse. People start a drag a little before the
/// first word or past the last one, so the line reaches well beyond its text
/// sideways (twice its height, at least 24 px) and a little above and below.
/// Points there select from the nearest word (see `nearest_word`).
fn hit_area(line: RECT) -> RECT {
    let h = line.bottom - line.top;
    inflate(line, (2 * h).max(24), (h / 4).max(3))
}

fn inflate(r: RECT, dx: i32, dy: i32) -> RECT {
    RECT { left: r.left - dx, top: r.top - dy, right: r.right + dx, bottom: r.bottom + dy }
}

fn union(a: RECT, b: RECT) -> RECT {
    RECT {
        left: a.left.min(b.left),
        top: a.top.min(b.top),
        right: a.right.max(b.right),
        bottom: a.bottom.max(b.bottom),
    }
}

fn send_mouse(flags: MOUSE_EVENT_FLAGS, data: i32) {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dwFlags: flags, mouseData: data as u32, ..Default::default() },
        },
    };
    unsafe {
        SendInput(&[input], size_of::<INPUT>() as i32);
    }
}

fn set_clipboard(owner: HWND, text: &str) -> Result<()> {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let mut opened = false;
        for _ in 0..10 {
            if OpenClipboard(Some(owner)).is_ok() {
                opened = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if !opened {
            return Err(Error::from_thread());
        }
        let result = (|| {
            EmptyClipboard()?;
            let mem = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2)?;
            let dst = GlobalLock(mem) as *mut u16;
            if dst.is_null() {
                let _ = GlobalFree(Some(mem));
                return Err(Error::from_thread());
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
            let _ = GlobalUnlock(mem);
            if let Err(e) = SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(mem.0))) {
                let _ = GlobalFree(Some(mem));
                return Err(e);
            }
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}

// --------------------------------------------------------------------- tray

fn tray_data(main: HWND) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: main,
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_APP_TRAY,
        hIcon: unsafe { LoadIconW(None, IDI_APPLICATION) }.unwrap_or_default(),
        ..Default::default()
    };
    let tip: Vec<u16> = "QuickCopy - copy text from images".encode_utf16().collect();
    nid.szTip[..tip.len()].copy_from_slice(&tip);
    nid
}

fn add_tray_icon(main: HWND) {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &tray_data(main));
    }
}

fn remove_tray_icon(main: HWND) {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &tray_data(main));
    }
}

/// Shows the tray menu. Runs a modal loop, so it must not hold the app borrow.
unsafe fn tray_menu(main: HWND) {
    let paused = with_app(|app| app.paused).unwrap_or(false);
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let pause_flags = if paused { MF_STRING | MF_CHECKED } else { MF_STRING };
        let _ = AppendMenuW(menu, pause_flags, MENU_PAUSE, w!("Pause"));
        let has_model = viet::model_path().is_some();
        let mut vi_flags = if worker::VIETNAMESE.load(Ordering::Relaxed) { MF_STRING | MF_CHECKED } else { MF_STRING };
        if !has_model {
            vi_flags |= MF_GRAYED;
        }
        let _ = AppendMenuW(menu, vi_flags, MENU_VIETNAMESE, w!("Tiếng Việt (VieSVTR)"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Quit QuickCopy"));
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(main);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, None, main, None);
        let _ = DestroyMenu(menu);
        match cmd.0 as usize {
            MENU_PAUSE => {
                with_app(|app| {
                    app.paused = !app.paused;
                    if app.paused {
                        hide(app);
                    }
                });
            }
            MENU_VIETNAMESE => {
                worker::VIETNAMESE.fetch_xor(true, Ordering::Relaxed);
                with_app(hide);
            }
            MENU_QUIT => PostQuitMessage(0),
            _ => {}
        }
    }
}
