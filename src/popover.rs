//! The small menu that pops up at the cursor once text is selected: Copy and
//! Translate, each with its shortcut. A layered window that never takes
//! focus (the overlay keeps it, so the shortcuts keep working), drawn with
//! Direct2D in the app's dark theme (theme.rs) and scaled to the monitor.


use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_numerics::Vector2;

use crate::{paint, theme};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Command {
    Copy,
    Translate,
    Original,
}

pub struct Item {
    pub label: &'static str,
    pub hint: String,
    pub command: Command,
}

/// Items for a plain selection, and for one showing its translation, in
/// the app language, with the configured translate shortcut.
pub fn items(translated: bool) -> Vec<Item> {
    let t = crate::i18n::t();
    if translated {
        vec![
            Item { label: t.copy_translation, hint: "Ctrl+C".into(), command: Command::Copy },
            Item { label: t.original, hint: "Esc".into(), command: Command::Original },
        ]
    } else {
        let shortcut = crate::settings::get().translate_shortcut.label();
        vec![
            Item { label: t.copy, hint: "Ctrl+C".into(), command: Command::Copy },
            Item { label: t.translate, hint: shortcut, command: Command::Translate },
        ]
    }
}

/// Top-left of a panel of `size` near the cursor: below or above the cursor,
/// below or above the selection, or beside it, whichever fits on the screen
/// (`work`), keeps clear of every `avoid` rect and of the cursor itself, and
/// is closest to the cursor. A popover over the text just selected hides
/// what the user is about to copy or translate.
pub fn place(cursor: POINT, size: SIZE, avoid: &[RECT], work: RECT, gap: i32) -> POINT {
    let (w, h) = (size.cx, size.cy);
    let bounds = avoid.iter().fold(None::<RECT>, |acc, r| {
        Some(match acc {
            None => *r,
            Some(a) => RECT { left: a.left.min(r.left), top: a.top.min(r.top), right: a.right.max(r.right), bottom: a.bottom.max(r.bottom) },
        })
    });
    // The cursor's own glyph (an I-beam about 3 gaps tall) must stay visible.
    let cursor_box = RECT { left: cursor.x - 2, top: cursor.y - 2, right: cursor.x + 2 * gap, bottom: cursor.y + 3 * gap };
    // Left edge near the cursor, kept on screen.
    let x = (cursor.x - 2 * gap).clamp(work.left, (work.right - w).max(work.left));
    let below_cursor = cursor_box.bottom + gap / 2;
    // (tier, spot): above/below come first; beside the text only when
    // neither fits, since a menu level with a line reads as part of it.
    let mut candidates = vec![(0, POINT { x, y: below_cursor }), (0, POINT { x, y: cursor_box.top - gap - h })];
    if let Some(b) = bounds {
        candidates.push((0, POINT { x, y: b.bottom.max(cursor_box.bottom) + gap }));
        candidates.push((0, POINT { x, y: b.top.min(cursor_box.top) - gap - h }));
        let y = (cursor.y - h / 2).clamp(work.top, (work.bottom - h).max(work.top));
        candidates.push((1, POINT { x: b.right.max(cursor_box.right) + gap, y }));
        candidates.push((1, POINT { x: b.left.min(cursor_box.left) - gap - w, y }));
    }
    let fits = |p: &POINT| {
        let r = RECT { left: p.x, top: p.y, right: p.x + w, bottom: p.y + h };
        let on_screen = r.left >= work.left && r.top >= work.top && r.right <= work.right && r.bottom <= work.bottom;
        let overlaps = |a: &RECT| r.left < a.right && a.left < r.right && r.top < a.bottom && a.top < r.bottom;
        on_screen && !overlaps(&cursor_box) && !avoid.iter().any(overlaps)
    };
    let distance = |p: &POINT| {
        let dx = (p.x - cursor.x).max(cursor.x - (p.x + w)).max(0) as i64;
        let dy = (p.y - cursor.y).max(cursor.y - (p.y + h)).max(0) as i64;
        dx * dx + dy * dy
    };
    candidates
        .iter()
        .filter(|(_, p)| fits(p))
        .min_by_key(|(tier, p)| (*tier, distance(p)))
        .map(|&(_, p)| p)
        .unwrap_or_else(|| {
            // Nothing fits: below the cursor, kept on screen.
            POINT { x, y: below_cursor.min(work.bottom - h).max(work.top) }
        })
}

pub struct Popover {
    pub hwnd: HWND,
    items: Vec<Item>,
    /// Button rects in window coordinates.
    buttons: Vec<D2D_RECT_F>,
    /// Rounded panel (the whole window).
    panel: D2D_RECT_F,
    size: SIZE,
    scale: f32,
    hover: Option<usize>,
    visible: bool,
}

// Sizes at 100% scaling (multiplied by the monitor's scale).
const LABEL_PX: f32 = 12.0;
const HINT_PX: f32 = 11.0;
/// Space between the panel edge and the buttons.
const INSET: f32 = 3.0;
/// Space before the label and after the hint inside a button.
const PAD: f32 = 8.0;
/// Space between a label and its shortcut hint.
const GAP: f32 = 6.0;
const BUTTON_H: f32 = 26.0;


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

    /// Shows `items` near the cursor `at` without covering `avoid` (the
    /// selected text, in screen coordinates). See `place`.
    pub fn show(&mut self, at: POINT, items: Vec<Item>, avoid: &[RECT]) {
        let monitor = unsafe { MonitorFromPoint(at, MONITOR_DEFAULTTONEAREST) };
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        let _ = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
        let s = dpi_x as f32 / 96.0;
        self.scale = s;
        self.items = items;
        self.hover = None;
        crate::theme::refresh();
        crate::theme::refresh();

        // Layout: [ label  hint ] [ label  hint ] in a rounded panel that is
        // the whole window. No drawn shadow around it: even nearly
        // transparent shadow pixels take clicks, and a drag starting next to
        // the menu landed on it.
        let (inset, pad, gap, height) = (INSET * s, PAD * s, GAP * s, BUTTON_H * s);
        let mut x = inset;
        self.buttons.clear();
        for item in &self.items {
            let width = pad + paint::measure(item.label, LABEL_PX * s, 400) + gap + paint::measure(&item.hint, HINT_PX * s, 400) + pad;
            self.buttons.push(D2D_RECT_F { left: x, top: inset, right: x + width, bottom: inset + height });
            x += width + 1.0;
        }
        let (w, h) = (x - 1.0 + inset, inset + height + inset);
        self.panel = D2D_RECT_F { left: 0.0, top: 0.0, right: w, bottom: h };
        self.size = SIZE { cx: w.ceil() as i32, cy: h.ceil() as i32 };

        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = unsafe { GetMonitorInfoW(monitor, &mut info) };
        let pos = place(at, self.size, avoid, info.rcWork, (8.0 * s) as i32);
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
        let s = self.scale;
        paint::update_layered(self.hwnd, pos, self.size.cx, self.size.cy, |target, f| unsafe {
            paint::panel(target, self.panel, s)?;
            let rounded = |r: D2D_RECT_F, k: f32| D2D1_ROUNDED_RECT { rect: r, radiusX: k, radiusY: k };
            let p = theme::get();
            let text = target.CreateSolidColorBrush(&paint::rgba(p.text, 1.0), None)?;
            let hint = target.CreateSolidColorBrush(&paint::rgba(p.subtle, 1.0), None)?;
            let hover = target.CreateSolidColorBrush(&paint::rgba(p.hover.0, p.hover.1), None)?;
            for (i, (item, b)) in self.items.iter().zip(&self.buttons).enumerate() {
                if self.hover == Some(i) {
                    target.FillRoundedRectangle(&rounded(*b, theme::HOVER_RADIUS * s), &hover);
                }
                let label = paint::text_layout(&f.dwrite, item.label, LABEL_PX * s, 400)?;
                let hint_layout = paint::text_layout(&f.dwrite, &item.hint, HINT_PX * s, 400)?;
                let mut lm = DWRITE_TEXT_METRICS::default();
                label.GetMetrics(&mut lm)?;
                let mut hm = DWRITE_TEXT_METRICS::default();
                hint_layout.GetMetrics(&mut hm)?;
                let mid = (b.top + b.bottom) / 2.0;
                let x = b.left + PAD * s;
                target.DrawTextLayout(Vector2 { X: x, Y: mid - lm.height / 2.0 }, &label, &text, D2D1_DRAW_TEXT_OPTIONS_NONE);
                let hx = x + lm.widthIncludingTrailingWhitespace + GAP * s;
                target.DrawTextLayout(Vector2 { X: hx, Y: mid - hm.height / 2.0 }, &hint_layout, &hint, D2D1_DRAW_TEXT_OPTIONS_NONE);
            }
            Ok(())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: RECT = RECT { left: 0, top: 0, right: 1920, bottom: 1080 };
    const SIZE_: SIZE = SIZE { cx: 240, cy: 40 };

    fn rect(l: i32, t: i32, r: i32, b: i32) -> RECT {
        RECT { left: l, top: t, right: r, bottom: b }
    }

    fn overlaps(p: POINT, a: &RECT) -> bool {
        p.x < a.right && a.left < p.x + SIZE_.cx && p.y < a.bottom && a.top < p.y + SIZE_.cy
    }

    /// Three selected lines, 24 px tall, 8 px apart.
    fn lines() -> Vec<RECT> {
        vec![rect(300, 400, 900, 424), rect(300, 432, 900, 456), rect(300, 464, 700, 488)]
    }

    #[test]
    fn drag_down_goes_just_below_the_cursor() {
        let p = place(POINT { x: 700, y: 476 }, SIZE_, &lines(), WORK, 8);
        assert!(lines().iter().all(|r| !overlaps(p, r)), "{p:?}");
        assert!(p.y >= 488 && p.y <= 510, "{p:?}");
    }

    #[test]
    fn drag_up_goes_above_the_selection_not_over_it() {
        // Cursor at the start of the first line: below it is selected text.
        let p = place(POINT { x: 300, y: 410 }, SIZE_, &lines(), WORK, 8);
        assert!(lines().iter().all(|r| !overlaps(p, r)), "{p:?}");
        assert!(p.y + SIZE_.cy <= 400, "{p:?}");
    }

    #[test]
    fn near_the_screen_bottom_goes_above() {
        let sel = vec![rect(300, 1020, 900, 1044)];
        let p = place(POINT { x: 900, y: 1032 }, SIZE_, &sel, WORK, 8);
        assert!(p.y + SIZE_.cy <= 1020 && p.y >= 0, "{p:?}");
    }

    #[test]
    fn tall_selection_goes_beside_it() {
        // Selected text from the top of the screen to the bottom.
        let sel: Vec<RECT> = (0..33).map(|i| rect(100, i * 32, 1300, i * 32 + 24)).collect();
        let p = place(POINT { x: 1300, y: 500 }, SIZE_, &sel, WORK, 8);
        assert!(sel.iter().all(|r| !overlaps(p, r)), "{p:?}");
        assert!(p.x >= 1300, "{p:?}");
    }

    #[test]
    fn never_under_the_cursor() {
        let p = place(POINT { x: 500, y: 500 }, SIZE_, &[], WORK, 8);
        let cursor = rect(498, 498, 518, 524);
        assert!(!overlaps(p, &cursor), "{p:?}");
    }
}
