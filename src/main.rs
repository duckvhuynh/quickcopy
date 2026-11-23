//! QuickCopy: hover an image for a moment, then select the text inside it like
//! normal text (drag, double/triple-click, Shift+arrows) and copy it with
//! Ctrl+C or the right-click menu. English and Vietnamese.
//!
//! Usage:
//!   quickcopy                 run in the tray
//!   quickcopy --probe X Y     OCR the image at screen point (X, Y) and print it
//!   quickcopy --ocr-file F    OCR an image file and print text and word boxes
//!   quickcopy --drag F X0 Y0 X1 Y1 ...
//!                             print what dragging from (X0, Y0) to (X1, Y1)
//!                             in image file F selects; several drags allowed
//!   quickcopy --layout F      print the lines of image file F in reading
//!                             order, with the block each belongs to

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod i18n;
mod languages;
mod layout;
mod menu;
mod missed;
mod paint;
mod popover;
mod rectfind;
mod settings_ui;
mod settings;
mod style;
mod textmodel;
mod theme;
mod translate;
mod video;
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
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

use textmodel::TextModel;
use translate::{Translated, WM_APP_TRANSLATED};
use worker::{OcrData, Request, WM_APP_HIDE, WM_APP_PLAYING, WM_APP_SHOW, WM_APP_UPDATE, Word, log};

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
/// How often a playing video under the resting cursor is checked again, so
/// pausing it (a click, the space key) brings up the overlay.
const VIDEO_RECHECK: Duration = Duration::from_millis(500);
/// How often we check that the covered image has not moved or changed.
const REVALIDATE: Duration = Duration::from_millis(300);
/// Brief green flash confirming a copy.
const FLASH_MS: u32 = 250;
const MENU_PAUSE: usize = 100;
const MENU_QUIT: usize = 101;
const MENU_SETTINGS: usize = 102;
const MENU_COPY: usize = 200;
const MENU_SELECT_ALL: usize = 201;
const MENU_TRANSLATE: usize = 202;
const MENU_ORIGINAL: usize = 203;

/// A translation painted over the image, like Google Lens.
struct Shown {
    boxes: Vec<paint::DrawBox>,
    /// The whole translation, for copying.
    text: String,
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
    /// A playing video under the cursor (screen rect): checked again every
    /// `VIDEO_RECHECK` until it pauses or the cursor leaves it.
    playing: Option<RECT>,
    data: Option<OcrData>,
    model: Option<TextModel>,
    last_revalidate: Instant,
    /// Selection as caret offsets into `model`; empty when equal.
    anchor: usize,
    focus: usize,
    dragging: bool,
    /// The overlay has keyboard focus (only while something is selected).
    active: bool,
    /// Our right-click menu is open over the image.
    menu_open: bool,
    /// The worker found the image changed while a drag or our menu was
    /// going on; the overlay goes when they end.
    stale: bool,
    /// The window that had focus before the overlay took it.
    prev_foreground: HWND,
    /// The user asked to copy before the Vietnamese reading arrived.
    pending_copy: bool,
    flashing: bool,
    /// The system accent, read when the overlay is shown.
    selection_color: theme::Rgb,
    last_dblclk: Option<(Instant, POINT)>,
    /// A translation request is out; its number is `translation_request`.
    translating: bool,
    translation_request: u64,
    translation: Option<Shown>,
    /// Copy / Translate menu shown at the cursor after a selection.
    popover: popover::Popover,
}

impl App {
    fn has_selection(&self) -> bool {
        self.anchor != self.focus
    }
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs `f` with the app state. Re-entrant calls (from messages sent while
/// `f` runs, such as WM_ACTIVATE during SetForegroundWindow) get `None`, so
/// state changes those messages would make are done by the caller directly.
/// Keep calls that pump messages (menus) outside of it.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut a| a.as_mut().map(f)))
}

/// Vietnamese reading is on when the setting is, the model is installed, and
/// QC_ENGLISH (Windows OCR only, for comparing in --probe) is not set.
fn vietnamese_wanted() -> bool {
    settings::get().vietnamese_ocr && viet::model_path().is_some() && std::env::var_os("QC_ENGLISH").is_none()
}

/// Test helper: reads an image file and prints the text each drag between
/// two points (image pixels, four numbers per drag) selects.
fn drag_file(path: &str, points: &[String]) -> Result<()> {
    let (_, _, words) = worker::read_file(path)?;
    let model = TextModel::new(&words, &worker::line_rects(&words));
    for drag in points.chunks_exact(4) {
        let n: Vec<i32> = drag.iter().map(|v| v.parse().expect("coordinates must be integers")).collect();
        let (from, to) = (model.hit(POINT { x: n[0], y: n[1] }), model.hit(POINT { x: n[2], y: n[3] }));
        println!("({}, {}) -> ({}, {}): {:?}", n[0], n[1], n[2], n[3], model.text(from, to));
    }
    Ok(())
}

/// True if this is the only running QuickCopy. A second launch would put a
/// second overlay and tray icon over the first; it opens the running one's
/// settings instead (what launching the app again is for) and exits.
fn only_instance() -> bool {
    unsafe {
        // Kept for the life of the process; Windows frees it on exit.
        let _ = CreateMutexW(None, false, w!("QuickCopy.Running"));
        if GetLastError() != ERROR_ALREADY_EXISTS {
            return true;
        }
        if let Ok(running) = FindWindowExW(Some(HWND_MESSAGE), None, w!("QuickCopyMain"), None) {
            let _ = PostMessageW(Some(running), WM_COMMAND, WPARAM(MENU_SETTINGS), LPARAM(0));
        }
        false
    }
}

/// Explorer broadcasts this when the taskbar is created again (it crashed or
/// restarted); tray icons are gone then and must be added again.
fn taskbar_created() -> u32 {
    static MESSAGE: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *MESSAGE.get_or_init(|| unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) })
}

fn main() -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    i18n::set(settings::get().ui_language);
    worker::VIETNAMESE.store(vietnamese_wanted(), Ordering::Relaxed);

    let args: Vec<String> = std::env::args().collect();
    if args.len() == 3 && args[1] == "--ocr-file" {
        return worker::ocr_file(&args[2]);
    }
    if args.len() == 3 && args[1] == "--layout" {
        let (_, _, words) = worker::read_file(&args[2])?;
        let model = TextModel::new(&words, &worker::line_rects(&words));
        for (line, (&(s, e), r)) in model.lines.iter().zip(&model.line_rects).enumerate() {
            println!("block {} line {line} x {}..{} y {}..{} {}", model.block_of(line), r.left, r.right, r.top, r.bottom, model.text(s, e));
        }
        return Ok(());
    }
    if args.len() >= 7 && args[1] == "--drag" {
        return drag_file(&args[2], &args[3..]);
    }
    if args.len() == 4 && (args[1] == "--probe" || args[1] == "--uia") {
        let x = args[2].parse().expect("X must be an integer");
        let y = args[3].parse().expect("Y must be an integer");
        if args[1] == "--uia" {
            return worker::print_uia(POINT { x, y });
        }
        return worker::probe(POINT { x, y });
    }
    if !only_instance() {
        return Ok(());
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

        let popover_class = w!("QuickCopyPopover");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(popover_proc),
            hInstance: instance,
            lpszClassName: popover_class,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
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
        // NOACTIVATE: hovering and clicking never take focus from the app;
        // the overlay activates itself only once text is selected.
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
        // Keep our own overlay out of screen captures: revalidation hashes
        // and the translation's color/size measurements must see the image,
        // not our highlight. (QC_CAPTURABLE: see `render`.)
        let _ = SetWindowDisplayAffinity(overlay, WDA_EXCLUDEFROMCAPTURE);
        // Owned by the overlay so it stays above it; never takes focus.
        let popover_hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            popover_class,
            w!("QuickCopy Popover"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            Some(overlay),
            None,
            Some(instance),
            None,
        )?;
        if std::env::var_os("QC_CAPTURABLE").is_none() {
            let _ = SetWindowDisplayAffinity(popover_hwnd, WDA_EXCLUDEFROMCAPTURE);
        }

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
                playing: None,
                data: None,
                model: None,
                last_revalidate: Instant::now(),
                anchor: 0,
                focus: 0,
                dragging: false,
                active: false,
                menu_open: false,
                stale: false,
                prev_foreground: HWND::default(),
                pending_copy: false,
                flashing: false,
                selection_color: theme::get().selection,
                last_dblclk: None,
                translating: false,
                translation_request: 0,
                translation: None,
                popover: popover::Popover::new(popover_hwnd),
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
        log("running; hover an image, then select its text".into());
        if args.iter().any(|a| a == "--settings") {
            settings_ui::open(main);
        }

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
        WM_APP_TRANSLATED => {
            let translated = unsafe { *Box::from_raw(lp.0 as *mut Translated) };
            with_app(|app| apply_translation(app, translated));
            LRESULT(0)
        }
        WM_APP_PLAYING => {
            let rect = unsafe { *Box::from_raw(lp.0 as *mut RECT) };
            with_app(|app| app.playing = Some(rect));
            LRESULT(0)
        }
        WM_APP_HIDE => {
            // The image moved or changed under the overlay. The worker has
            // let it go, so a hide put off now must still happen.
            with_app(|app| {
                if app.dragging || app.menu_open {
                    app.stale = true;
                } else {
                    hide_stale(app);
                }
            });
            LRESULT(0)
        }
        settings_ui::WM_APP_SETTINGS_CHANGED => {
            // Apply what the settings window changed.
            refresh_tray_icon(hwnd);
            worker::VIETNAMESE.store(vietnamese_wanted(), Ordering::Relaxed);
            with_app(|app| {
                hide(app);
                let _ = app.tx.send(Request::Forget);
            });
            LRESULT(0)
        }
        WM_COMMAND => {
            tray_command(hwnd, wp.0 & 0xFFFF);
            LRESULT(0)
        }
        WM_APP_TRAY => {
            let event = (lp.0 & 0xFFFF) as u32;
            if event == WM_RBUTTONUP || event == WM_CONTEXTMENU {
                tray_menu(hwnd);
            } else if event == WM_LBUTTONDBLCLK {
                settings_ui::open(hwnd);
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

/// QC_CAPTURABLE=1: screenshots can see the overlay while it has focus.
fn capturable() -> bool {
    static CAPTURABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CAPTURABLE.get_or_init(|| std::env::var_os("QC_CAPTURABLE").is_some())
}

fn tick(app: &mut App) {
    let mut pos = POINT::default();
    if unsafe { GetCursorPos(&mut pos) }.is_err() {
        return;
    }
    let now = Instant::now();

    if let Some(data) = &app.data {
        // Also while something is selected: the wheel over the image but
        // off its text lines goes straight to the page, which scrolls the
        // image away from under the selection. Not while our own menu is
        // open over the image (it would look like the image changing), nor
        // when the overlay is made visible to screenshots for debugging.
        if !app.dragging && !app.menu_open && !(app.active && capturable()) && now - app.last_revalidate >= REVALIDATE {
            app.last_revalidate = now;
            let _ = app.tx.send(Request::Revalidate);
        }
        // A selection stays until the user clicks elsewhere (like native
        // text), so only an idle overlay goes away when the cursor leaves.
        let busy = app.dragging || app.active || app.flashing || app.pending_copy;
        if !busy && !contains(data.rect, pos) {
            log("hide: cursor left the image".into());
            hide(app);
            let _ = app.tx.send(Request::Forget);
        }
    }

    if app.playing.is_some_and(|r| !contains(r, pos)) {
        app.playing = None;
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
    let video_due = app.playing.is_some() && now - app.requested_at >= VIDEO_RECHECK;
    if !app.requested || retry_due || video_due {
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
    theme::refresh();
    app.selection_color = theme::get().selection;
    let r = data.rect;
    log(format!("overlay {}x{} at ({}, {}), {} words", r.right - r.left, r.bottom - r.top, r.left, r.top, data.words.len()));
    app.model = Some(TextModel::new(&data.words, &data.lines));
    app.data = Some(data);
    app.playing = None;
    app.anchor = 0;
    app.focus = 0;
    app.dragging = false;
    app.translating = false;
    app.translation = None;
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
    app.popover.hide();
    if app.active {
        deactivate(app);
    }
    if app.data.take().is_some() {
        unsafe {
            let _ = ShowWindow(app.overlay, SW_HIDE);
            let _ = KillTimer(Some(app.overlay), FLASH_TIMER);
        }
    }
    app.model = None;
    app.anchor = 0;
    app.focus = 0;
    app.pending_copy = false;
    app.dragging = false;
    app.flashing = false;
    app.translating = false;
    app.translation = None;
    app.stale = false;
}

/// The Vietnamese reading arrived for the image on screen: swap the words in,
/// find the selection again by position, and finish a waiting copy.
fn apply_update(app: &mut App, data: OcrData) {
    if app.data.as_ref().is_none_or(|d| d.id != data.id) {
        return;
    }
    let model = TextModel::new(&data.words, &data.lines);
    if let Some(old) = &app.model {
        let remap = |o: usize| model.hit(old.point_of(o));
        app.anchor = remap(app.anchor);
        app.focus = remap(app.focus);
    }
    app.model = Some(model);
    app.data = Some(data);
    if std::mem::take(&mut app.pending_copy) {
        copy_selection(app);
    } else {
        render(app);
    }
}

// ------------------------------------------------------------ keyboard focus

/// Takes keyboard focus so Ctrl+C and Shift+arrows reach the selection, like
/// clicking into text does. Called on a click in the overlay, which is what
/// allows it. It keeps focus until the user clicks elsewhere, presses Esc,
/// or types a key meant for the app (handed back, see `on_key`).
fn activate(app: &mut App) {
    if app.active {
        return;
    }
    unsafe {
        app.prev_foreground = GetForegroundWindow();
        if !SetForegroundWindow(app.overlay).as_bool() {
            log("could not take keyboard focus".into());
            return;
        }
    }
    app.active = true;
}

/// Gives keyboard focus back to the window that had it.
fn deactivate(app: &mut App) {
    app.active = false;
    unsafe {
        if GetForegroundWindow() == app.overlay && !app.prev_foreground.is_invalid() {
            let _ = SetForegroundWindow(app.prev_foreground);
        }
    }
}

// ------------------------------------------------------------- overlay window

unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let pt = POINT {
        x: (lp.0 & 0xFFFF) as i16 as i32,
        y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32,
    };
    match msg {
        // Broadcast to top-level windows only (not the message-only main
        // window): light/dark or the accent changed.
        WM_SETTINGCHANGE => {
            theme::refresh();
            if let Some(main) = with_app(|app| app.main) {
                refresh_tray_icon(main);
            }
            LRESULT(0)
        }
        _ if msg == taskbar_created() => {
            if let Some(main) = with_app(|app| app.main) {
                log("taskbar created again: adding the tray icon".into());
                add_tray_icon(main);
            }
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_ACTIVATE => {
            // Lost focus (click elsewhere, Alt+Tab): the selection goes, as
            // with native text. Gaining focus is recorded by `activate`.
            if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE {
                with_app(|app| {
                    app.popover.hide();
                    app.active = false;
                    app.anchor = app.focus;
                    app.dragging = false;
                    render(app);
                });
            }
            LRESULT(0)
        }
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
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = wp.0 as u16;
            if with_app(|app| on_key(app, vk)).unwrap_or(false) {
                LRESULT(0)
            } else {
                unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
            }
        }
        WM_TIMER if wp.0 == FLASH_TIMER => {
            with_app(|app| {
                unsafe {
                    let _ = KillTimer(Some(app.overlay), FLASH_TIMER);
                }
                app.flashing = false;
                render(app);
            });
            LRESULT(0)
        }
        // Hand the wheel back to the app underneath.
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
        // With a selection, right-click opens our Copy menu (on button up,
        // like native). Without one, it goes to the app (its image menu).
        WM_RBUTTONDOWN => {
            if !with_app(|app| app.has_selection()).unwrap_or(false) {
                with_app(|app| {
                    hide(app);
                    let _ = app.tx.send(Request::Forget);
                });
                send_mouse(MOUSEEVENTF_RIGHTDOWN, 0);
                send_mouse(MOUSEEVENTF_RIGHTUP, 0);
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            if with_app(|app| app.has_selection()).unwrap_or(false) {
                with_app(|app| app.menu_open = true);
                unsafe { selection_menu(hwnd) };
                with_app(|app| {
                    app.menu_open = false;
                    if app.stale {
                        hide_stale(app);
                    }
                });
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// From Win32::UI::Controls, a large feature to enable for one constant.
const WM_MOUSELEAVE: u32 = 0x02A3;

unsafe extern "system" fn popover_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let pt = POINT {
        x: (lp.0 & 0xFFFF) as i16 as i32,
        y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32,
    };
    match msg {
        // Clicking it must not take focus from the overlay.
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            let mut track = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
            unsafe {
                let _ = TrackMouseEvent(&mut track);
            }
            with_app(|app| app.popover.set_hover(Some(pt)));
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            with_app(|app| app.popover.set_hover(None));
            LRESULT(0)
        }
        WM_LBUTTONDOWN => LRESULT(0),
        WM_LBUTTONUP => {
            with_app(|app| {
                if let Some(command) = app.popover.command_at(pt) {
                    run_command(app, command);
                }
            });
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// Opens the Copy / Translate menu at the cursor for the current selection.
fn offer_actions(app: &mut App) {
    if !app.has_selection() {
        return;
    }
    let mut at = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut at);
    }
    // Keep clear of what the user is reading: the translation if shown,
    // otherwise the selection (overlay coordinates -> screen).
    let (Some(data), Some(model)) = (&app.data, &app.model) else { return };
    let origin = (data.rect.left, data.rect.top);
    let boxes: Vec<RECT> = match &app.translation {
        Some(shown) => shown.boxes.iter().map(|b| b.fill).collect(),
        None => model.highlight(app.anchor, app.focus).into_iter().map(|r| inflate(r, 1, 2)).collect(),
    };
    let avoid: Vec<RECT> = boxes
        .into_iter()
        .map(|r| RECT { left: r.left + origin.0, top: r.top + origin.1, right: r.right + origin.0, bottom: r.bottom + origin.1 })
        .collect();
    log(format!("popover avoids {} box(es), bounds {:?}", avoid.len(), avoid.iter().fold(None::<(i32, i32, i32, i32)>, |a, r| Some(match a {
        None => (r.left, r.top, r.right, r.bottom),
        Some((l, t, rr, b)) => (l.min(r.left), t.min(r.top), rr.max(r.right), b.max(r.bottom)),
    }))));
    app.popover.show(at, popover::items(app.translation.is_some()), &avoid);
}

fn run_command(app: &mut App, command: popover::Command) {
    app.popover.hide();
    match command {
        popover::Command::Copy => copy_selection(app),
        popover::Command::Translate => start_translation(app),
        popover::Command::Original => {
            app.translation = None;
            render(app);
        }
    }
}

fn key_down(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(vk.0 as i32) < 0 }
}

fn on_button_down(app: &mut App, pt: POINT) {
    if app.model.is_none() {
        return;
    }
    activate(app);
    app.popover.hide();
    // A new selection starts from the original text, and no longer waits
    // for a translation of the previous one.
    app.translation = None;
    app.translating = false;
    let Some(model) = &app.model else { return };
    let at = model.hit(pt);

    // Third click right after a double-click selects the whole line.
    if let Some((t, p)) = app.last_dblclk.take() {
        let dbl = Duration::from_millis(unsafe { GetDoubleClickTime() } as u64);
        if t.elapsed() <= dbl && (p.x - pt.x).abs() <= 4 && (p.y - pt.y).abs() <= 4 {
            let (s, e) = model.line_at(at);
            (app.anchor, app.focus) = (s, e);
            app.flashing = false;
            render(app);
            offer_actions(app);
            return;
        }
    }

    // Shift+click extends the selection; a plain click starts a new one.
    if !key_down(VK_SHIFT) {
        app.anchor = at;
    }
    app.focus = at;
    app.dragging = true;
    app.flashing = false;
    render(app);
    unsafe {
        SetCapture(app.overlay);
    }
}

fn on_mouse_move(app: &mut App, pt: POINT) {
    if !app.dragging {
        return;
    }
    let Some(model) = &app.model else { return };
    let at = model.hit(pt);
    if at != app.focus {
        app.focus = at;
        render(app);
    }
}

fn on_button_up(app: &mut App) {
    if !app.dragging {
        return;
    }
    app.dragging = false;
    unsafe {
        let _ = ReleaseCapture();
    }
    if app.stale {
        hide_stale(app);
        return;
    }
    offer_actions(app);
}

/// Hides an overlay whose image moved or changed, and looks again where
/// the cursor rests.
fn hide_stale(app: &mut App) {
    hide(app);
    app.still_since = Instant::now();
    app.requested = false;
}

fn on_double_click(app: &mut App, pt: POINT) {
    let Some(model) = &app.model else { return };
    let (s, e) = model.word_at(model.hit(pt));
    (app.anchor, app.focus) = (s, e);
    app.last_dblclk = Some((Instant::now(), pt));
    app.flashing = false;
    render(app);
    activate(app);
    offer_actions(app);
}

/// Native text keys while the overlay has focus. Returns false for keys left
/// to the default handler.
fn on_key(app: &mut App, vk: u16) -> bool {
    let Some(model) = &app.model else { return false };
    let vk = VIRTUAL_KEY(vk);
    let (shift, ctrl) = (key_down(VK_SHIFT), key_down(VK_CONTROL));
    if settings::get().translate_shortcut.matches(vk.0, ctrl, shift, key_down(VK_MENU)) {
        run_command(app, popover::Command::Translate);
        return true;
    }
    let n = model.len();
    let (lo, hi) = (app.anchor.min(app.focus), app.anchor.max(app.focus));
    let collapsed = lo == hi;

    let target = match vk {
        VK_LEFT if !shift && !collapsed && !ctrl => Some(lo),
        VK_RIGHT if !shift && !collapsed && !ctrl => Some(hi),
        VK_LEFT if ctrl => Some(model.prev_word(app.focus)),
        VK_RIGHT if ctrl => Some(model.next_word(app.focus)),
        VK_LEFT => Some(app.focus.saturating_sub(1)),
        VK_RIGHT => Some((app.focus + 1).min(n)),
        VK_UP => Some(model.vertical(app.focus, false)),
        VK_DOWN => Some(model.vertical(app.focus, true)),
        VK_HOME => Some(if ctrl { 0 } else { model.line_at(app.focus).0 }),
        VK_END => Some(if ctrl { n } else { model.line_at(app.focus).1 }),
        _ => None,
    };
    if let Some(t) = target {
        app.popover.hide();
        app.focus = t;
        if !shift {
            app.anchor = t;
        }
        app.flashing = false;
        render(app);
        return true;
    }

    match vk {
        VK_A if ctrl => {
            (app.anchor, app.focus) = (0, n);
            render(app);
            true
        }
        VK_C | VK_INSERT if ctrl => {
            run_command(app, popover::Command::Copy);
            true
        }
        // Esc closes the menu first, then puts the original back, then dismisses.
        VK_ESCAPE if app.popover.visible() => {
            app.popover.hide();
            true
        }
        // Esc first puts the original text back, then dismisses.
        VK_ESCAPE if app.translation.is_some() => {
            app.translation = None;
            render(app);
            true
        }
        VK_ESCAPE => {
            hide(app);
            let _ = app.tx.send(Request::Forget);
            true
        }
        // Modifiers alone change nothing.
        VK_SHIFT | VK_CONTROL | VK_MENU | VK_LSHIFT | VK_RSHIFT | VK_LCONTROL | VK_RCONTROL | VK_LWIN | VK_RWIN => false,
        // Anything else was meant for the app: give focus back and replay it.
        _ => {
            app.anchor = app.focus;
            render(app);
            deactivate(app);
            send_key(vk);
            true
        }
    }
}

fn select_all(app: &mut App) {
    if let Some(model) = &app.model {
        (app.anchor, app.focus) = (0, model.len());
        render(app);
    }
}

fn copy_selection(app: &mut App) {
    if let Some(shown) = &app.translation {
        let text = shown.text.clone();
        match set_clipboard(app.main, &text) {
            Ok(()) => log(format!("copied translation: {text:?}")),
            Err(e) => log(format!("clipboard error: {e}")),
        }
        return;
    }
    let (Some(data), Some(model)) = (&app.data, &app.model) else { return };
    if !app.has_selection() {
        return;
    }
    if data.pending {
        // Copy once the Vietnamese reading arrives (apply_update).
        log("copy waits for the Vietnamese reading".into());
        app.pending_copy = true;
        return;
    }
    let text = model.text(app.anchor, app.focus);
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

/// Right-click menu for a selection. Runs a modal loop, so it must not hold
/// the app borrow.
unsafe fn selection_menu(overlay: HWND) {
    let translated = with_app(|app| {
        app.popover.hide();
        app.translation.is_some()
    })
    .unwrap_or(false);
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let t = i18n::t();
        let item = |id: usize, label: String| {
            let _ = AppendMenuW(menu, MF_STRING, id, &HSTRING::from(label));
        };
        if translated {
            item(MENU_COPY, format!("{}\tCtrl+C", t.copy_translation));
            item(MENU_ORIGINAL, format!("{}\tEsc", t.show_original));
        } else {
            let prefs = settings::get();
            let target = t.translate_to.replace("{}", i18n::language_name(&prefs.target_language));
            item(MENU_COPY, format!("{}\tCtrl+C", t.copy));
            item(MENU_TRANSLATE, format!("{target}\t{}", prefs.translate_shortcut.label()));
            item(MENU_SELECT_ALL, format!("{}\tCtrl+A", t.select_all));
        }
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, None, overlay, None);
        let _ = DestroyMenu(menu);
        match cmd.0 as usize {
            MENU_COPY => {
                with_app(copy_selection);
            }
            MENU_SELECT_ALL => {
                with_app(select_all);
            }
            MENU_TRANSLATE => {
                with_app(start_translation);
            }
            MENU_ORIGINAL => {
                with_app(|app| {
                    app.translation = None;
                    render(app);
                });
            }
            _ => {}
        }
    }
}

/// Sends the selected lines off for translation. The translation thread
/// measures each line's look and groups them into headings and paragraphs
/// (`translate::group`), so each is translated and laid out on its own.
fn start_translation(app: &mut App) {
    if !app.has_selection() || app.translating || app.translation.is_some() {
        return;
    }
    let (Some(data), Some(model)) = (&app.data, &app.model) else { return };
    let lines: Vec<translate::Line> = model
        .selected_lines(app.anchor, app.focus)
        .into_iter()
        .filter(|(_, text)| !text.trim().is_empty())
        .map(|(rect, text)| translate::Line { rect, text })
        .collect();
    if lines.is_empty() {
        return;
    }
    log(format!("translating {} line(s)", lines.len()));
    app.translating = true;
    app.translation_request += 1;
    let (request, clip) = (app.translation_request, data.rect);
    render(app);
    translate::spawn(app.main, request, clip, settings::get().target_language, lines);
}

/// A translation came back: draw it in the original's size and weight, on
/// the original baselines, wrapped into the selected line boxes.
fn apply_translation(app: &mut App, translated: Translated) {
    // Only the request that is still waiting (not one from a selection or
    // image since dismissed).
    if !app.translating || translated.request != app.translation_request {
        return;
    }
    app.translating = false;
    let Some(image_w) = app.data.as_ref().map(|d| d.rect.right - d.rect.left) else {
        return;
    };
    let units = match translated.result {
        Ok(units) => units,
        Err(e) => {
            log(format!("translation failed: {e}"));
            render(app);
            return;
        }
    };
    let median = |mut v: Vec<f32>| {
        v.sort_by(f32::total_cmp);
        v.get(v.len() / 2).copied()
    };
    // Pass 1: each unit's look (matched size and weight) and its fit.
    struct Plan {
        size: f32,
        weight: i32,
        widths: Vec<f32>,
        fit: layout::Fit,
    }
    let mut plans = Vec::new();
    for unit in &units {
        let inks: Vec<style::Ink> = unit.slices.iter().filter_map(|s| s.ink).collect();
        let line_h = median(unit.slices.iter().map(|s| (s.rect.bottom - s.rect.top) as f32).collect()).unwrap_or(16.0);
        // The original's look, from the median of its lines.
        let original = (!inks.is_empty()).then(|| style::Ink {
            body: median(inks.iter().map(|i| i.body).collect()).unwrap_or(1.0),
            stroke: median(inks.iter().map(|i| i.stroke).collect()).unwrap_or(1.0),
            baseline: 0.0,
        });
        let sample: String = unit.slices.iter().map(|s| s.original.as_str()).collect::<Vec<_>>().join(" ").chars().take(60).collect();
        let matched = original.and_then(|ink| paint::match_style(&sample, ink));
        let (size, weight) = matched.as_ref().map_or((line_h * 0.9, 400), |m| (m.size, m.weight));
        log(format!("matched style: size {size:.1}px weight {weight} (body {:?})", original.map(|o| (o.body, o.stroke))));

        // Keep the original size: lines may run to the paragraph's right edge
        // and the last one to the image's, before the font shrinks.
        let right = unit.slices.iter().map(|s| s.rect.right).max().unwrap_or(0) as f32 + size * 0.5;
        let last = unit.slices.len().saturating_sub(1);
        let widths: Vec<f32> = unit
            .slices
            .iter()
            .enumerate()
            .map(|(i, s)| if i == last { image_w as f32 - 4.0 } else { right } - s.rect.left as f32)
            .collect();
        let fit = layout::fit(&unit.translation, &widths, size, &mut |t, s| paint::measure(t, s, weight));
        plans.push(Plan { size, weight, widths, fit });
    }

    // Pass 2: text that looked the same in the original looks the same
    // translated. Units of one style share the smallest size any of them
    // had to shrink to; otherwise a paragraph that ran long would sit in a
    // smaller font than the sentence above it.
    let shared: Vec<f32> = plans
        .iter()
        .map(|p| {
            plans
                .iter()
                .filter(|q| q.weight == p.weight && (q.size / p.size - 1.0).abs() < 0.1)
                .map(|q| q.fit.size)
                .fold(p.fit.size, f32::min)
        })
        .collect();
    for ((plan, unit), size) in plans.iter_mut().zip(&units).zip(shared) {
        if size < plan.fit.size {
            let weight = plan.weight;
            plan.fit = layout::fit(&unit.translation, &plan.widths, size, &mut |t, s| paint::measure(t, s, weight));
        }
    }

    // Pass 3: boxes, on the original baselines.
    let mut boxes = Vec::new();
    let mut text = Vec::new();
    for (unit, plan) in units.iter().zip(plans) {
        text.push(unit.translation.clone());
        let weight = plan.weight;
        // Baseline of lines the measurement missed: same offset from the box
        // top as the measured lines.
        let offset = median(unit.slices.iter().filter_map(|s| s.ink.map(|i| i.baseline - s.rect.top as f32)).collect());
        for (slice, line) in unit.slices.iter().zip(plan.fit.lines) {
            let h = slice.rect.bottom - slice.rect.top;
            // Cover the original glyphs, tone marks included.
            let mut fill = translate::inflate(slice.rect, 2, 0);
            fill.top -= (h * 2 / 5).max(2);
            fill.bottom += (h / 3).max(2);
            let needed = paint::measure(&line, plan.fit.size, weight).ceil() as i32 + 6;
            fill.right = fill.right.max(fill.left + needed);
            let baseline = slice.ink.map(|i| i.baseline).or(offset.map(|o| slice.rect.top as f32 + o));
            boxes.push(paint::DrawBox {
                fill,
                background: slice.background,
                foreground: slice.foreground,
                text: line,
                size: plan.fit.size,
                weight,
                baseline,
            });
        }
    }
    log(format!("showing translation: {:?}", text.join(" / ")));
    app.translation = Some(Shown { boxes, text: text.join("\n") });
    render(app);
    if app.active {
        offer_actions(app);
    }
}

/// Joins words with spaces, and lines with newlines (for printing results).
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

// ------------------------------------------------------------------ drawing

/// Premultiplied BGRA pixel.
fn pixel(r: u32, g: u32, b: u32, a: u32) -> u32 {
    (a << 24) | ((r * a / 255) << 16) | ((g * a / 255) << 8) | (b * a / 255)
}

/// Redraws the overlay. Text paragraphs and a wide margin around them get
/// alpha 1 (invisible but clickable, see `textmodel::grab_areas`); the rest
/// has alpha 0, so clicks there fall through to the app below.
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
        let old = SelectObject(mem, dib.into());
        let px = std::slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize);
        px.fill(0);

        let hit = pixel(0, 0, 0, 1);
        for area in textmodel::grab_areas(&data.lines, data.video) {
            fill(px, w, h, area, hit);
        }
        if let (Some(model), true, None) = (&app.model, app.has_selection(), &app.translation) {
            let color = if app.flashing {
                pixel(0x22, 0xC5, 0x5E, 0x70) // copied
            } else if app.translating {
                pixel(0xF5, 0x9E, 0x0B, 0x60) // waiting for the translation
            } else {
                let [r, g, b] = app.selection_color;
                pixel(r as u32, g as u32, b as u32, 0x60)
            };
            for r in model.highlight(app.anchor, app.focus) {
                fill_rounded(px, w, h, inflate(r, 1, 2), color);
            }
        }
        if let Some(shown) = &app.translation {
            let _ = GdiFlush();
            if let Err(e) = paint::draw(mem, w, h, &shown.boxes) {
                log(format!("drawing translation failed: {e}"));
            }
        }
        // QC_CAPTURABLE=1: screenshots can see the overlay while it has focus
        // (selection, translation), for checking how it is drawn. Never while
        // a translation measures the image, nor while revalidation runs.
        if capturable() {
            let affinity = if app.active && !app.translating { WDA_NONE } else { WDA_EXCLUDEFROMCAPTURE };
            let _ = SetWindowDisplayAffinity(app.overlay, affinity);
        }

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

/// Fills `r` with slightly rounded, antialiased corners, blending `color`
/// (premultiplied) over what is there.
fn fill_rounded(px: &mut [u32], w: i32, h: i32, r: RECT, color: u32) {
    let radius = ((r.bottom - r.top) as f32 / 5.0).clamp(2.0, 6.0);
    let blend = |dst: u32, k: f32| -> u32 {
        let mut out = 0u32;
        for shift in [0, 8, 16, 24] {
            let s = ((color >> shift) & 0xFF) as f32;
            let d = ((dst >> shift) & 0xFF) as f32;
            out |= ((s * k + d * (1.0 - k)).round() as u32).min(255) << shift;
        }
        out
    };
    for y in r.top.max(0)..r.bottom.min(h) {
        for x in r.left.max(0)..r.right.min(w) {
            // Distance into the corner square, measured from the pixel center.
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let cx = if fx < r.left as f32 + radius { r.left as f32 + radius } else if fx > r.right as f32 - radius { r.right as f32 - radius } else { fx };
            let cy = if fy < r.top as f32 + radius { r.top as f32 + radius } else if fy > r.bottom as f32 - radius { r.bottom as f32 - radius } else { fy };
            let dist = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
            let k = (radius - dist + 0.5).clamp(0.0, 1.0);
            if k > 0.0 {
                let i = (y * w + x) as usize;
                px[i] = blend(px[i], k);
            }
        }
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

fn inflate(r: RECT, dx: i32, dy: i32) -> RECT {
    RECT { left: r.left - dx, top: r.top - dy, right: r.right + dx, bottom: r.bottom + dy }
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

/// Replays a key press for the window that now has focus (its release
/// arrives there by itself).
fn send_key(vk: VIRTUAL_KEY) {
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, ..Default::default() } },
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
        hIcon: tray_icon(),
        ..Default::default()
    };
    let tip: Vec<u16> = i18n::t().tooltip.encode_utf16().take(nid.szTip.len() - 1).collect();
    nid.szTip[..tip.len()].copy_from_slice(&tip);
    nid
}

/// Icon resource ids (assets/quickcopy.rc).
pub const ICON_APP: u16 = 1;
/// White glyph, for a dark taskbar.
const ICON_TRAY_WHITE: u16 = 2;
/// Dark glyph, for a light taskbar.
const ICON_TRAY_BLACK: u16 = 3;

pub fn load_icon(id: u16, size: i32) -> HICON {
    unsafe {
        let Ok(instance) = GetModuleHandleW(None) else { return HICON::default() };
        LoadImageW(Some(instance.into()), PCWSTR(id as usize as *const u16), IMAGE_ICON, size, size, LR_SHARED)
            .map(|h| HICON(h.0))
            .unwrap_or_default()
    }
}

/// The glyph that reads on the taskbar's own light or dark color.
fn tray_icon() -> HICON {
    let id = if theme::taskbar_light() { ICON_TRAY_BLACK } else { ICON_TRAY_WHITE };
    load_icon(id, unsafe { GetSystemMetrics(SM_CXSMICON) })
}

fn add_tray_icon(main: HWND) {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &tray_data(main));
    }
}

/// Shows the tooltip in the current app language and the icon for the
/// current taskbar color.
pub fn refresh_tray_icon(main: HWND) {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &tray_data(main));
    }
}

fn remove_tray_icon(main: HWND) {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &tray_data(main));
    }
}

/// Shows the tray menu (menu.rs); picks come back as WM_COMMAND.
fn tray_menu(main: HWND) {
    let paused = with_app(|app| app.paused).unwrap_or(false);
    let t = i18n::t();
    let mut at = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut at);
    }
    menu::show(
        main,
        at,
        vec![
            menu::Entry { label: t.settings.into(), id: MENU_SETTINGS, checked: false },
            menu::Entry { label: t.pause.into(), id: MENU_PAUSE, checked: paused },
            menu::Entry { label: t.quit.into(), id: MENU_QUIT, checked: false },
        ],
    );
}

/// A tray menu pick.
fn tray_command(main: HWND, id: usize) {
    match id {
        MENU_SETTINGS => settings_ui::open(main),
        MENU_PAUSE => {
            with_app(|app| {
                app.paused = !app.paused;
                // Paused, nothing watches the cursor.
                unsafe {
                    if app.paused {
                        hide(app);
                        let _ = KillTimer(Some(main), TICK_TIMER);
                    } else {
                        SetTimer(Some(main), TICK_TIMER, TICK_MS, None);
                    }
                }
            });
        }
        MENU_QUIT => unsafe { PostQuitMessage(0) },
        _ => {}
    }
}
