//! The tray icon's menu, drawn like the selection popover instead of the
//! system menu. It closes when it loses focus, on Esc, or once an entry is
//! picked; a pick is posted to the owner window as WM_COMMAND with the id.

use std::cell::RefCell;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;
use windows_numerics::Vector2;

use crate::{paint, theme};

pub struct Entry {
    pub label: String,
    /// Sent as the WM_COMMAND id.
    pub id: usize,
    pub checked: bool,
    /// Draw a separator line above this entry.
    pub separator: bool,
}

const TEXT_PX: f32 = 12.0;
const INSET: f32 = 4.0;
const ITEM_H: f32 = 28.0;
const PAD: f32 = 10.0;
/// Room for the check mark on the right.
const CHECK_W: f32 = 22.0;
const SEPARATOR_H: f32 = 9.0;
const MIN_W: f32 = 160.0;
/// From Win32::UI::Controls, a large feature to enable for one constant.
const WM_MOUSELEAVE: u32 = 0x02A3;

struct Menu {
    hwnd: HWND,
    owner: HWND,
    entries: Vec<Entry>,
    rows: Vec<D2D_RECT_F>,
    size: SIZE,
    scale: f32,
    hover: Option<usize>,
    pos: POINT,
}

thread_local! {
    static MENU: RefCell<Option<Menu>> = const { RefCell::new(None) };
}

fn with_menu<R>(f: impl FnOnce(&mut Menu) -> R) -> Option<R> {
    MENU.with(|m| m.try_borrow_mut().ok().and_then(|mut m| m.as_mut().map(f)))
}

/// Opens the menu with its corner at `at` (the cursor on the tray icon),
/// growing up and left, kept inside the monitor's work area.
pub fn show(owner: HWND, at: POINT, entries: Vec<Entry>) {
    let hwnd = match with_menu(|m| m.hwnd) {
        Some(h) => h,
        None => match create() {
            Some(h) => h,
            None => return,
        },
    };
    let monitor = unsafe { MonitorFromPoint(at, MONITOR_DEFAULTTONEAREST) };
    let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
    let _ = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
    let s = dpi_x as f32 / 96.0;

    // Layout: one row per entry, separators as thin gaps.
    let widest = entries.iter().map(|e| paint::measure(&e.label, TEXT_PX * s, 400)).fold(0.0, f32::max);
    let w = (INSET + PAD + widest + CHECK_W + PAD + INSET).max(MIN_W) * s;
    let mut y = INSET * s;
    let mut rows = Vec::new();
    for e in &entries {
        if e.separator {
            y += SEPARATOR_H * s;
        }
        rows.push(D2D_RECT_F { left: INSET * s, top: y, right: w - INSET * s, bottom: y + ITEM_H * s });
        y += ITEM_H * s;
    }
    let size = SIZE { cx: w.ceil() as i32, cy: (y + INSET * s).ceil() as i32 };

    let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    let _ = unsafe { GetMonitorInfoW(monitor, &mut info) };
    let work = info.rcWork;
    let pos = POINT {
        x: (at.x - size.cx).clamp(work.left, (work.right - size.cx).max(work.left)),
        y: (at.y - size.cy).clamp(work.top, (work.bottom - size.cy).max(work.top)),
    };
    crate::worker::log(format!("tray menu at ({}, {}) {}x{}", pos.x, pos.y, size.cx, size.cy));
    MENU.with(|m| *m.borrow_mut() = Some(Menu { hwnd, owner, entries, rows, size, scale: s, hover: None, pos }));
    with_menu(|m| render(m));
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), pos.x, pos.y, size.cx, size.cy, SWP_SHOWWINDOW);
        // Focus, so a click anywhere else closes it (WM_ACTIVATE).
        let _ = SetForegroundWindow(hwnd);
    }
}

fn create() -> Option<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        let class = w!("QuickCopyMenu");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: instance.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        });
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class,
            w!("QuickCopy Menu"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .ok()
    }
}

fn hide(m: &mut Menu) {
    unsafe {
        let _ = ShowWindow(m.hwnd, SW_HIDE);
    }
}

fn pick(m: &mut Menu, i: usize) {
    hide(m);
    if let Some(e) = m.entries.get(i) {
        unsafe {
            let _ = PostMessageW(Some(m.owner), WM_COMMAND, WPARAM(e.id), LPARAM(0));
        }
    }
}

fn render(m: &Menu) {
    let s = m.scale;
    let panel = D2D_RECT_F { left: 0.0, top: 0.0, right: m.size.cx as f32, bottom: m.size.cy as f32 };
    paint::update_layered(m.hwnd, m.pos, m.size.cx, m.size.cy, |target, f| unsafe {
        paint::panel(target, panel, s)?;
        let text = target.CreateSolidColorBrush(&paint::rgba(theme::TEXT, 1.0), None)?;
        let hover = target.CreateSolidColorBrush(&paint::rgba(theme::HOVER.0, theme::HOVER.1), None)?;
        let line = target.CreateSolidColorBrush(&paint::rgba(theme::SEPARATOR.0, theme::SEPARATOR.1), None)?;
        for (i, (e, r)) in m.entries.iter().zip(&m.rows).enumerate() {
            if e.separator {
                let y = (r.top - SEPARATOR_H * s / 2.0).round();
                target.FillRectangle(&D2D_RECT_F { left: r.left + 6.0 * s, top: y, right: r.right - 6.0 * s, bottom: y + 1.0 }, &line);
            }
            if m.hover == Some(i) {
                let k = theme::HOVER_RADIUS * s;
                target.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: *r, radiusX: k, radiusY: k }, &hover);
            }
            let layout = paint::text_layout(&f.dwrite, &e.label, TEXT_PX * s, 400)?;
            let mut tm = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut tm)?;
            let mid = (r.top + r.bottom) / 2.0;
            target.DrawTextLayout(Vector2 { X: r.left + PAD * s, Y: mid - tm.height / 2.0 }, &layout, &text, D2D1_DRAW_TEXT_OPTIONS_NONE);
            if e.checked {
                let check = paint::text_layout(&f.dwrite, "✓", TEXT_PX * s, 600)?;
                let mut cm = DWRITE_TEXT_METRICS::default();
                check.GetMetrics(&mut cm)?;
                let x = r.right - PAD * s - cm.widthIncludingTrailingWhitespace;
                target.DrawTextLayout(Vector2 { X: x, Y: mid - cm.height / 2.0 }, &check, &text, D2D1_DRAW_TEXT_OPTIONS_NONE);
            }
        }
        Ok(())
    });
}

fn row_at(m: &Menu, x: f32, y: f32) -> Option<usize> {
    m.rows.iter().position(|r| x >= r.left && x < r.right && y >= r.top && y < r.bottom)
}

fn set_hover(m: &mut Menu, hover: Option<usize>) {
    if hover != m.hover {
        m.hover = hover;
        render(m);
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let (x, y) = ((lp.0 & 0xFFFF) as i16 as f32, ((lp.0 >> 16) & 0xFFFF) as i16 as f32);
    match msg {
        WM_ACTIVATE if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE => {
            with_menu(hide);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let mut track = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
            unsafe {
                let _ = TrackMouseEvent(&mut track);
            }
            with_menu(|m| {
                let hover = row_at(m, x, y);
                set_hover(m, hover);
            });
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            with_menu(|m| set_hover(m, None));
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            with_menu(|m| {
                if let Some(i) = row_at(m, x, y) {
                    pick(m, i);
                }
            });
            LRESULT(0)
        }
        WM_KEYDOWN => {
            with_menu(|m| {
                let n = m.entries.len();
                match VIRTUAL_KEY(wp.0 as u16) {
                    VK_ESCAPE => hide(m),
                    VK_DOWN => set_hover(m, Some(m.hover.map_or(0, |i| (i + 1) % n))),
                    VK_UP => set_hover(m, Some(m.hover.map_or(n - 1, |i| (i + n - 1) % n))),
                    VK_RETURN => {
                        if let Some(i) = m.hover {
                            pick(m, i);
                        }
                    }
                    _ => {}
                }
            });
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}
