//! QuickCopy: hover an image for a moment, then select the text inside it like
//! normal text (drag, double/triple-click, Shift+arrows) and copy it with
//! Ctrl+C or the right-click menu. English and Vietnamese.
//!
//! Usage:
//!   quickcopy                 run in the tray
//!   quickcopy --probe X Y     OCR the image at screen point (X, Y) and print it
//!   quickcopy --ocr-file F    OCR an image file and print text and word boxes

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod layout;
mod paint;
mod rectfind;
mod textmodel;
mod translate;
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

use textmodel::TextModel;
use translate::{Translated, Unit, WM_APP_TRANSLATED};
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
/// Brief green flash confirming a copy.
const FLASH_MS: u32 = 250;
const MENU_PAUSE: usize = 100;
const MENU_QUIT: usize = 101;
const MENU_VIETNAMESE: usize = 102;
const MENU_COPY: usize = 200;
const MENU_SELECT_ALL: usize = 201;
const MENU_TRANSLATE: usize = 202;
const MENU_ORIGINAL: usize = 203;
/// Language translations are made into (English when the text already is).
const TARGET_LANGUAGE: &str = "vi";

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
    data: Option<OcrData>,
    model: Option<TextModel>,
    last_revalidate: Instant,
    /// Selection as caret offsets into `model`; empty when equal.
    anchor: usize,
    focus: usize,
    dragging: bool,
    /// The overlay has keyboard focus (only while something is selected).
    active: bool,
    /// The window that had focus before the overlay took it.
    prev_foreground: HWND,
    /// The user asked to copy before the Vietnamese reading arrived.
    pending_copy: bool,
    flashing: bool,
    last_dblclk: Option<(Instant, POINT)>,
    /// A translation request is out.
    translating: bool,
    translation: Option<Shown>,
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
        // Keep our own overlay out of screen captures so revalidation hashes
        // stay stable. QC_CAPTURABLE=1 leaves it visible to screenshots, for
        // checking what the overlay draws.
        if std::env::var_os("QC_CAPTURABLE").is_none() {
            let _ = SetWindowDisplayAffinity(overlay, WDA_EXCLUDEFROMCAPTURE);
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
                data: None,
                model: None,
                last_revalidate: Instant::now(),
                anchor: 0,
                focus: 0,
                dragging: false,
                active: false,
                prev_foreground: HWND::default(),
                pending_copy: false,
                flashing: false,
                last_dblclk: None,
                translating: false,
                translation: None,
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
        WM_APP_HIDE => {
            // The image moved or changed under the overlay.
            with_app(|app| {
                if !app.dragging {
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
        // Not while the overlay has focus: our own menu over the image would
        // look like the image changing, and the page cannot scroll then
        // anyway (wheel and other keys hand focus back first).
        if !app.dragging && !app.active && now - app.last_revalidate >= REVALIDATE {
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
    app.model = Some(TextModel::new(&data.words, &data.lines));
    app.data = Some(data);
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
}

/// The Vietnamese reading arrived for the image on screen: swap the words in,
/// find the selection again by position, and finish a waiting copy.
fn apply_update(app: &mut App, data: OcrData) {
    if app.data.as_ref().is_none_or(|d| d.id != data.id) {
        return;
    }
    let model = TextModel::new(&data.words, &data.lines);
    if let Some(old) = &app.model {
        let remap = |o: usize| {
            let (line, x) = old.pos_of(o);
            model.offset_at(line.min(model.lines.len().saturating_sub(1)), x)
        };
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
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_ACTIVATE => {
            // Lost focus (click elsewhere, Alt+Tab): the selection goes, as
            // with native text. Gaining focus is recorded by `activate`.
            if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE {
                with_app(|app| {
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
                unsafe { selection_menu(hwnd) };
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
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
    // A new selection starts from the original text.
    app.translation = None;
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
}

fn on_double_click(app: &mut App, pt: POINT) {
    let Some(model) = &app.model else { return };
    let (s, e) = model.word_at(model.hit(pt));
    (app.anchor, app.focus) = (s, e);
    app.last_dblclk = Some((Instant::now(), pt));
    app.flashing = false;
    render(app);
    activate(app);
}

/// Native text keys while the overlay has focus. Returns false for keys left
/// to the default handler.
fn on_key(app: &mut App, vk: u16) -> bool {
    let Some(model) = &app.model else { return false };
    let vk = VIRTUAL_KEY(vk);
    let (shift, ctrl) = (key_down(VK_SHIFT), key_down(VK_CONTROL));
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
            copy_selection(app);
            true
        }
        VK_T if ctrl => {
            start_translation(app);
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
    let translated = with_app(|app| app.translation.is_some()).unwrap_or(false);
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        if translated {
            let _ = AppendMenuW(menu, MF_STRING, MENU_COPY, w!("Sao chép bản dịch\tCtrl+C"));
            let _ = AppendMenuW(menu, MF_STRING, MENU_ORIGINAL, w!("Hiện bản gốc\tEsc"));
        } else {
            let _ = AppendMenuW(menu, MF_STRING, MENU_COPY, w!("Sao chép\tCtrl+C"));
            let _ = AppendMenuW(menu, MF_STRING, MENU_TRANSLATE, w!("Dịch sang Tiếng Việt\tCtrl+T"));
            let _ = AppendMenuW(menu, MF_STRING, MENU_SELECT_ALL, w!("Chọn tất cả\tCtrl+A"));
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

/// Sends the selection off for translation, one request per paragraph so
/// sentences that wrap across lines are translated whole.
fn start_translation(app: &mut App) {
    if !app.has_selection() || app.translating || app.translation.is_some() {
        return;
    }
    let (Some(data), Some(model)) = (&app.data, &app.model) else { return };
    let (lo, hi) = (app.anchor.min(app.focus), app.anchor.max(app.focus));
    let mut units = Vec::new();
    for p in textmodel::paragraphs(&model.line_rects) {
        let (Some(&first), Some(&last)) = (p.lines.iter().min(), p.lines.iter().max()) else { continue };
        let (a, b) = (lo.max(model.lines[first].0), hi.min(model.lines[last].1));
        if a >= b {
            continue;
        }
        let text = model.text(a, b).replace('\n', " ");
        if text.trim().is_empty() {
            continue;
        }
        units.push(Unit { text: text.trim().to_string(), slices: model.highlight(a, b) });
    }
    if units.is_empty() {
        return;
    }
    units.sort_by_key(|u| u.slices.first().map_or(0, |r| r.top));
    // Widely spaced lines can land in separate paragraphs; a sentence that
    // runs on (no closing punctuation) into the text right below is one unit.
    let mut merged: Vec<Unit> = Vec::new();
    for unit in units {
        if let (Some(prev), Some(first)) = (merged.last_mut(), unit.slices.first()) {
            let last = prev.slices.last().copied().unwrap_or_default();
            let h = last.bottom - last.top;
            let ends = prev.text.trim_end().ends_with(['.', '!', '?', ':', ';', '…']);
            if !ends && first.top - last.bottom <= 2 * h {
                prev.text = format!("{} {}", prev.text, unit.text);
                prev.slices.extend(unit.slices);
                continue;
            }
        }
        merged.push(unit);
    }
    let units = merged;
    log(format!("translating {} paragraph(s)", units.len()));
    app.translating = true;
    render(app);
    translate::spawn(app.main, data.id, data.rect, TARGET_LANGUAGE, units);
}

/// A translation came back: wrap it into the selected line boxes, shrinking
/// the font where it runs longer than the original, and paint it.
fn apply_translation(app: &mut App, translated: Translated) {
    app.translating = false;
    if app.data.as_ref().is_none_or(|d| d.id != translated.id) {
        return;
    }
    let units = match translated.result {
        Ok(units) => units,
        Err(e) => {
            log(format!("translation failed: {e}"));
            render(app);
            return;
        }
    };
    let mut boxes = Vec::new();
    let mut text = Vec::new();
    for unit in &units {
        text.push(unit.translation.clone());
        let mut heights: Vec<i32> = unit.slices.iter().map(|s| s.rect.bottom - s.rect.top).collect();
        heights.sort_unstable();
        let line_h = heights.get(heights.len() / 2).copied().unwrap_or(16) as f32;
        let widths: Vec<f32> = unit.slices.iter().map(|s| (s.rect.right - s.rect.left) as f32 + line_h * 0.3).collect();
        let fit = layout::fit(&unit.translation, &widths, line_h * 0.9, &mut paint::measure);
        for (slice, line) in unit.slices.iter().zip(fit.lines) {
            let h = slice.rect.bottom - slice.rect.top;
            // Cover the original glyphs, tone marks included.
            let mut fill = translate::inflate(slice.rect, 2, (h / 5).max(2));
            let needed = paint::measure(&line, fit.size).ceil() as i32 + 4;
            fill.right = fill.right.max(fill.left + needed);
            boxes.push(paint::DrawBox { fill, background: slice.background, foreground: slice.foreground, text: line, size: fit.size });
        }
    }
    log(format!("showing translation: {:?}", text.join(" / ")));
    app.translation = Some(Shown { boxes, text: text.join("\n") });
    render(app);
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
        for area in textmodel::grab_areas(&data.lines) {
            fill(px, w, h, area, hit);
        }
        if let (Some(model), true, None) = (&app.model, app.has_selection(), &app.translation) {
            let color = if app.flashing {
                pixel(0x22, 0xC5, 0x5E, 0x70) // copied
            } else if app.translating {
                pixel(0xF5, 0x9E, 0x0B, 0x60) // waiting for the translation
            } else {
                pixel(0x26, 0x84, 0xFF, 0x60)
            };
            for r in model.highlight(app.anchor, app.focus) {
                fill(px, w, h, inflate(r, 0, 2), color);
            }
        }
        if let Some(shown) = &app.translation {
            let _ = GdiFlush();
            if let Err(e) = paint::draw(mem, w, h, &shown.boxes) {
                log(format!("drawing translation failed: {e}"));
            }
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
