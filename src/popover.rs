//! The small menu that pops up at the cursor once text is selected: Copy and
//! Translate, each with its shortcut. A layered window that never takes
//! focus (the overlay keeps it, so the shortcuts keep working), drawn with
//! Direct2D in the system's light or dark theme and scaled to the monitor.

use std::ffi::c_void;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;
use windows_numerics::Vector2;

use crate::paint;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Command {
    Copy,
    Translate,
    Original,
}

pub struct Item {
    pub label: &'static str,
    pub hint: &'static str,
    pub command: Command,
}

/// Items for a plain selection, and for one showing its translation.
pub fn items(translated: bool) -> Vec<Item> {
    if translated {
        vec![
            Item { label: "Sao chép bản dịch", hint: "Ctrl+C", command: Command::Copy },
            Item { label: "Bản gốc", hint: "Esc", command: Command::Original },
        ]
    } else {
        vec![
            Item { label: "Sao chép", hint: "Ctrl+C", command: Command::Copy },
            Item { label: "Dịch", hint: "Ctrl+T", command: Command::Translate },
        ]
    }
}

struct Theme {
    background: [u8; 3],
    border: ([u8; 3], f32),
    text: [u8; 3],
    hint: [u8; 3],
    hover: ([u8; 3], f32),
    separator: ([u8; 3], f32),
}

const DARK: Theme = Theme {
    background: [44, 44, 44],
    border: ([255, 255, 255], 0.10),
    text: [255, 255, 255],
    hint: [165, 165, 165],
    hover: ([255, 255, 255], 0.10),
    separator: ([255, 255, 255], 0.14),
};

const LIGHT: Theme = Theme {
    background: [250, 250, 250],
    border: ([0, 0, 0], 0.10),
    text: [26, 26, 26],
    hint: [112, 112, 112],
    hover: ([0, 0, 0], 0.06),
    separator: ([0, 0, 0], 0.10),
};

/// Whether apps should use the light theme (Settings > Personalization).
fn light_theme() -> bool {
    let mut value = 1u32;
    let mut size = size_of::<u32>() as u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    ok.is_err() || value != 0
}

pub struct Popover {
    pub hwnd: HWND,
    items: Vec<Item>,
    /// Button rects in window coordinates.
    buttons: Vec<D2D_RECT_F>,
    /// Rounded panel inside the window (the margin around it holds the shadow).
    panel: D2D_RECT_F,
    size: SIZE,
    scale: f32,
    hover: Option<usize>,
    visible: bool,
}

const LABEL_PX: f32 = 13.0;
const HINT_PX: f32 = 12.0;

impl Popover {
    pub fn new(hwnd: HWND) -> Self {
        Self {
            hwnd,
            items: Vec::new(),
            buttons: Vec::new(),
            panel: D2D_RECT_F::default(),
            size: SIZE::default(),
            scale: 1.0,
            hover: None,
            visible: false,
        }
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    /// Shows `items` just below `at` (screen point), flipped above it near
    /// the bottom of the screen.
    pub fn show(&mut self, at: POINT, items: Vec<Item>) {
        let monitor = unsafe { MonitorFromPoint(at, MONITOR_DEFAULTTONEAREST) };
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        let _ = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
        let s = dpi_x as f32 / 96.0;
        self.scale = s;
        self.items = items;
        self.hover = None;

        // Layout: [ label  hint ] | [ label  hint ], inside a rounded panel.
        let (margin, inset, pad, gap, height) = (10.0 * s, 4.0 * s, 12.0 * s, 10.0 * s, 32.0 * s);
        let mut x = margin + inset;
        self.buttons.clear();
        for item in &self.items {
            let width = pad + paint::measure(item.label, LABEL_PX * s, 400) + gap + paint::measure(item.hint, HINT_PX * s, 400) + pad;
            self.buttons.push(D2D_RECT_F { left: x, top: margin + inset, right: x + width, bottom: margin + inset + height });
            x += width + 1.0;
        }
        let w = x - 1.0 + inset + margin;
        let h = margin + inset + height + inset + margin;
        self.panel = D2D_RECT_F { left: margin, top: margin, right: w - margin, bottom: h - margin };
        self.size = SIZE { cx: w.ceil() as i32, cy: h.ceil() as i32 };

        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = unsafe { GetMonitorInfoW(monitor, &mut info) };
        let work = info.rcWork;
        let m = margin as i32;
        let mut pos = POINT { x: at.x - m, y: at.y + (18.0 * s) as i32 - m };
        if pos.y + self.size.cy - m > work.bottom {
            pos.y = at.y - (10.0 * s) as i32 - self.size.cy + m;
        }
        pos.x = pos.x.clamp(work.left - m, (work.right - self.size.cx + m).max(work.left - m));
        self.visible = true;
        crate::worker::log(format!("popover at ({}, {}) {}x{}", pos.x, pos.y, self.size.cx, self.size.cy));
        self.render(pos);
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), pos.x, pos.y, self.size.cx, self.size.cy, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
    }

    pub fn hide(&mut self) {
        if self.visible {
            crate::worker::log("popover hidden".into());
            self.visible = false;
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }

    fn hit(&self, pt: POINT) -> Option<usize> {
        let (x, y) = (pt.x as f32, pt.y as f32);
        self.buttons.iter().position(|b| x >= b.left && x < b.right && y >= b.top && y < b.bottom)
    }

    /// Hover feedback; `pt` in window coordinates, `None` when the mouse left.
    pub fn set_hover(&mut self, pt: Option<POINT>) {
        let hover = pt.and_then(|p| self.hit(p));
        if hover != self.hover && self.visible {
            self.hover = hover;
            let mut r = RECT::default();
            let _ = unsafe { GetWindowRect(self.hwnd, &mut r) };
            self.render(POINT { x: r.left, y: r.top });
        }
    }

    pub fn command_at(&self, pt: POINT) -> Option<Command> {
        self.hit(pt).map(|i| self.items[i].command)
    }

    fn render(&self, pos: POINT) {
        let (w, h) = (self.size.cx, self.size.cy);
        let theme = if light_theme() { &LIGHT } else { &DARK };
        let s = self.scale;
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
            if let Ok(dib) = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
                let old = SelectObject(mem, dib.into());
                std::slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize).fill(0);
                let drawn = paint::with_factories(|f| -> Result<()> {
                    let target = paint::dc_target(&f.d2d, mem, w, h)?;
                    target.BeginDraw();
                    target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
                    let radius = 8.0 * s;
                    let rounded = |r: D2D_RECT_F, k: f32| D2D1_ROUNDED_RECT { rect: r, radiusX: k, radiusY: k };
                    // Soft shadow: a few widening, fading layers.
                    for i in 1..=4 {
                        let spread = i as f32 * 2.0 * s;
                        let r = D2D_RECT_F {
                            left: self.panel.left - spread * 0.5,
                            top: self.panel.top - spread * 0.25,
                            right: self.panel.right + spread * 0.5,
                            bottom: self.panel.bottom + spread,
                        };
                        let shadow = target.CreateSolidColorBrush(&paint::rgba([0, 0, 0], 0.05), None)?;
                        target.FillRoundedRectangle(&rounded(r, radius + spread), &shadow);
                    }
                    let background = target.CreateSolidColorBrush(&paint::rgba(theme.background, 0.98), None)?;
                    target.FillRoundedRectangle(&rounded(self.panel, radius), &background);
                    let border = target.CreateSolidColorBrush(&paint::rgba(theme.border.0, theme.border.1), None)?;
                    let edge = D2D_RECT_F {
                        left: self.panel.left + 0.5,
                        top: self.panel.top + 0.5,
                        right: self.panel.right - 0.5,
                        bottom: self.panel.bottom - 0.5,
                    };
                    target.DrawRoundedRectangle(&rounded(edge, radius), &border, 1.0, None);

                    let text = target.CreateSolidColorBrush(&paint::rgba(theme.text, 1.0), None)?;
                    let hint = target.CreateSolidColorBrush(&paint::rgba(theme.hint, 1.0), None)?;
                    let hover = target.CreateSolidColorBrush(&paint::rgba(theme.hover.0, theme.hover.1), None)?;
                    let separator = target.CreateSolidColorBrush(&paint::rgba(theme.separator.0, theme.separator.1), None)?;
                    for (i, (item, b)) in self.items.iter().zip(&self.buttons).enumerate() {
                        if self.hover == Some(i) {
                            target.FillRoundedRectangle(&rounded(*b, 5.0 * s), &hover);
                        }
                        if i > 0 {
                            let x = b.left - 1.0;
                            target.FillRectangle(&D2D_RECT_F { left: x, top: b.top + 7.0 * s, right: x + 1.0, bottom: b.bottom - 7.0 * s }, &separator);
                        }
                        let label = paint::text_layout(&f.dwrite, item.label, LABEL_PX * s, 400)?;
                        let hint_layout = paint::text_layout(&f.dwrite, item.hint, HINT_PX * s, 400)?;
                        let mut lm = DWRITE_TEXT_METRICS::default();
                        label.GetMetrics(&mut lm)?;
                        let mut hm = DWRITE_TEXT_METRICS::default();
                        hint_layout.GetMetrics(&mut hm)?;
                        let mid = (b.top + b.bottom) / 2.0;
                        let x = b.left + 12.0 * s;
                        target.DrawTextLayout(Vector2 { X: x, Y: mid - lm.height / 2.0 }, &label, &text, D2D1_DRAW_TEXT_OPTIONS_NONE);
                        let hx = x + lm.widthIncludingTrailingWhitespace + 10.0 * s;
                        target.DrawTextLayout(Vector2 { X: hx, Y: mid - hm.height / 2.0 }, &hint_layout, &hint, D2D1_DRAW_TEXT_OPTIONS_NONE);
                    }
                    target.EndDraw(None, None)
                });
                if let Some(Err(e)) = drawn {
                    crate::worker::log(format!("drawing popover failed: {e}"));
                }
                let size = SIZE { cx: w, cy: h };
                let src = POINT::default();
                let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
                let _ = UpdateLayeredWindow(self.hwnd, Some(screen), Some(&pos), Some(&size), Some(mem), Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA);
                SelectObject(mem, old);
                let _ = DeleteObject(dib.into());
            }
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
        }
    }
}
