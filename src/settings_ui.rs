//! The settings window: dark rows on the panel color, drawn with Direct2D in
//! the same style as the selection popover. The title bar takes the same
//! color and keeps only a close button, which hides the window to the tray.
//! Choosing the translate-into language swaps the rows for a searchable list.

use std::cell::RefCell;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;
use windows_numerics::Vector2;

use crate::i18n::{self, UiLang};
use crate::settings::{self, Shortcut};
use crate::{paint, theme};

/// Posted to the main window after a setting changed, so it can apply it.
pub const WM_APP_SETTINGS_CHANGED: u32 = WM_APP + 20;

const W: f32 = 420.0;
/// Space between the window edges and the content.
const PAD: f32 = 16.0;
/// Gap under the title bar.
const TOP: f32 = 4.0;
const ROW_H: f32 = 44.0;
const FOOTER_H: f32 = 34.0;
const ROWS: usize = 5;
const CONTROL_W: f32 = 200.0;
const CONTROL_H: f32 = 28.0;
const TEXT_PX: f32 = 13.0;
const SMALL_PX: f32 = 12.0;
const LIST_ROW_H: f32 = 30.0;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    None,
    StartToggle,
    Language,
    Shortcut,
    UiLang(UiLang),
    OcrToggle,
    Search,
    Pick(usize),
}

enum Mode {
    Rows,
    Picker { query: String, scroll: f32, highlight: usize },
}

struct Ui {
    hwnd: HWND,
    main: HWND,
    target: Option<ID2D1HwndRenderTarget>,
    hover: Hit,
    mode: Mode,
    capturing: bool,
    /// Shown under the shortcut row after a rejected key.
    error: Option<&'static str>,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

fn with_ui<R>(f: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    UI.with(|u| u.try_borrow_mut().ok().and_then(|mut u| u.as_mut().map(f)))
}

fn scale(hwnd: HWND) -> f32 {
    unsafe { GetDpiForWindow(hwnd) as f32 / 96.0 }.max(1.0)
}

fn client_h() -> f32 {
    TOP + ROWS as f32 * ROW_H + FOOTER_H
}

/// Opens the settings window, or brings it to the front.
pub fn open(main: HWND) {
    if let Some(hwnd) = with_ui(|ui| {
        ui.mode = Mode::Rows;
        ui.capturing = false;
        ui.error = None;
        let _ = unsafe { SetWindowTextW(ui.hwnd, &HSTRING::from(title())) };
        invalidate(ui);
        ui.hwnd
    }) {
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
        return;
    }
    unsafe {
        let Ok(instance) = GetModuleHandleW(None) else { return };
        let class = w!("QuickCopySettings");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: instance.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: CreateSolidBrush(COLORREF(rgb(theme::PANEL))),
            ..Default::default()
        });
        // Caption with only the close button (no minimize or maximize).
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            &HSTRING::from(title()),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            100,
            100,
            None,
            None,
            Some(instance.into()),
            None,
        ) else {
            return;
        };
        // Dark title bar in the panel color, so the window's own border is
        // the only one.
        let on: BOOL = TRUE;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, (&on as *const BOOL).cast(), size_of::<BOOL>() as u32);
        let caption = COLORREF(rgb(theme::PANEL));
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_CAPTION_COLOR, (&caption as *const COLORREF).cast(), size_of::<COLORREF>() as u32);
        let border = COLORREF(rgb(theme::WINDOW_BORDER));
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, (&border as *const COLORREF).cast(), size_of::<COLORREF>() as u32);

        // Size the client area for this monitor's scale, centered on it.
        let s = scale(hwnd);
        let mut r = RECT { left: 0, top: 0, right: (W * s) as i32, bottom: (client_h() * s) as i32 };
        let _ = AdjustWindowRectExForDpi(&mut r, style, false, WINDOW_EX_STYLE::default(), GetDpiForWindow(hwnd));
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let work = info.rcWork;
        let (x, y) = ((work.left + work.right - w) / 2, (work.top + work.bottom - h) / 2);
        let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER);
        let mut client = POINT::default();
        let _ = ClientToScreen(hwnd, &mut client);
        crate::worker::log(format!("settings window client at ({}, {}) scale {s}", client.x, client.y));

        UI.with(|u| {
            *u.borrow_mut() =
                Some(Ui { hwnd, main, target: None, hover: Hit::None, mode: Mode::Rows, capturing: false, error: None })
        });
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
}

fn title() -> String {
    format!("QuickCopy - {}", i18n::t().settings.trim_end_matches('…'))
}

fn rgb(c: [u8; 3]) -> u32 {
    c[0] as u32 | (c[1] as u32) << 8 | (c[2] as u32) << 16
}

/// Rects of everything clickable, in client pixels.
struct Layout {
    s: f32,
    /// The rows' area, full width (also where the language list goes).
    body: D2D_RECT_F,
    rows: [D2D_RECT_F; ROWS],
    start_toggle: D2D_RECT_F,
    language: D2D_RECT_F,
    shortcut: D2D_RECT_F,
    ui_en: D2D_RECT_F,
    ui_vi: D2D_RECT_F,
    ocr_toggle: D2D_RECT_F,
    search: D2D_RECT_F,
    list: D2D_RECT_F,
}

fn layout(hwnd: HWND) -> Layout {
    let s = scale(hwnd);
    let body = D2D_RECT_F { left: 0.0, top: TOP * s, right: W * s, bottom: (TOP + ROWS as f32 * ROW_H) * s };
    let rows: [D2D_RECT_F; ROWS] = std::array::from_fn(|i| D2D_RECT_F {
        left: body.left,
        top: body.top + i as f32 * ROW_H * s,
        right: body.right,
        bottom: body.top + (i + 1) as f32 * ROW_H * s,
    });
    // A control at the right of a row, vertically centered.
    let control = |row: &D2D_RECT_F, w: f32| D2D_RECT_F {
        left: row.right - (PAD + w) * s,
        top: (row.top + row.bottom) / 2.0 - CONTROL_H * s / 2.0,
        right: row.right - PAD * s,
        bottom: (row.top + row.bottom) / 2.0 + CONTROL_H * s / 2.0,
    };
    let toggle = |row: &D2D_RECT_F| {
        let c = control(row, 36.0);
        D2D_RECT_F { top: c.top + 4.0 * s, bottom: c.bottom - 4.0 * s, ..c }
    };
    let ui = control(&rows[3], CONTROL_W);
    let mid = (ui.left + ui.right) / 2.0;
    let search = D2D_RECT_F { left: PAD * s, top: body.top + 6.0 * s, right: body.right - PAD * s, bottom: body.top + (6.0 + 30.0) * s };
    Layout {
        s,
        body,
        start_toggle: toggle(&rows[0]),
        language: control(&rows[1], CONTROL_W),
        shortcut: control(&rows[2], CONTROL_W),
        ui_en: D2D_RECT_F { right: mid, ..ui },
        ui_vi: D2D_RECT_F { left: mid, ..ui },
        ocr_toggle: toggle(&rows[4]),
        search,
        list: D2D_RECT_F { left: (PAD - 6.0) * s, top: search.bottom + 6.0 * s, right: body.right - (PAD - 6.0) * s, bottom: body.bottom },
        rows,
    }
}

fn inside(r: &D2D_RECT_F, x: f32, y: f32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

/// Languages matching the picker's query, in the app language's order.
fn matches(query: &str) -> Vec<(&'static str, &'static str)> {
    let q = fold(query.trim());
    i18n::lang()
        .languages()
        .iter()
        .copied()
        .filter(|(code, name)| q.is_empty() || fold(name).contains(&q) || code.to_lowercase().starts_with(&q))
        .collect()
}

/// Lowercase without diacritics, so "viet" finds "Việt".
fn fold(s: &str) -> String {
    const FROM: &str = "àáảãạăằắẳẵặâầấẩẫậèéẻẽẹêềếểễệìíỉĩịòóỏõọôồốổỗộơờớởỡợùúủũụưừứửữựỳýỷỹỵđäëïöüçñåæœøß";
    const TO: &str = "aaaaaaaaaaaaaaaaaeeeeeeeeeeeiiiiiooooooooooooooooouuuuuuuuuuuyyyyydaeioucnaaoos";
    s.to_lowercase()
        .chars()
        .map(|c| FROM.chars().position(|f| f == c).and_then(|i| TO.chars().nth(i)).unwrap_or(c))
        .collect()
}

fn hit_test(ui: &Ui, x: f32, y: f32) -> Hit {
    let l = layout(ui.hwnd);
    match &ui.mode {
        Mode::Rows => {
            let model = crate::viet::model_path().is_some();
            [
                (l.start_toggle, Hit::StartToggle),
                (l.language, Hit::Language),
                (l.shortcut, Hit::Shortcut),
                (l.ui_en, Hit::UiLang(UiLang::En)),
                (l.ui_vi, Hit::UiLang(UiLang::Vi)),
                (l.ocr_toggle, if model { Hit::OcrToggle } else { Hit::None }),
            ]
            .into_iter()
            .find(|(r, _)| inside(r, x, y))
            .map_or(Hit::None, |(_, h)| h)
        }
        Mode::Picker { query, scroll, .. } => {
            if inside(&l.search, x, y) {
                return Hit::Search;
            }
            if !inside(&l.list, x, y) {
                return Hit::None;
            }
            let i = ((y - l.list.top + scroll) / (LIST_ROW_H * l.s)) as usize;
            if i < matches(query).len() { Hit::Pick(i) } else { Hit::None }
        }
    }
}

/// Tells the main window something changed and redraws.
fn changed(ui: &mut Ui) {
    unsafe {
        let _ = PostMessageW(Some(ui.main), WM_APP_SETTINGS_CHANGED, WPARAM(0), LPARAM(0));
        let _ = SetWindowTextW(ui.hwnd, &HSTRING::from(title()));
    }
    invalidate(ui);
}

fn invalidate(ui: &Ui) {
    unsafe {
        let _ = InvalidateRect(Some(ui.hwnd), None, false);
    }
}

fn click(ui: &mut Ui, hit: Hit) {
    ui.error = None;
    if hit != Hit::Shortcut {
        ui.capturing = false;
    }
    match hit {
        Hit::StartToggle => {
            let on = settings::start_with_windows();
            if let Err(e) = settings::set_start_with_windows(!on) {
                crate::worker::log(format!("start with Windows: {e}"));
            }
            changed(ui);
        }
        Hit::Language => {
            let current = settings::get().target_language;
            let highlight = matches("").iter().position(|(c, _)| *c == current).unwrap_or(0);
            let l = layout(ui.hwnd);
            let visible = (l.list.bottom - l.list.top) / (LIST_ROW_H * l.s);
            let scroll = ((highlight as f32 - visible / 2.0).max(0.0)) * LIST_ROW_H * l.s;
            ui.mode = Mode::Picker { query: String::new(), scroll, highlight };
            invalidate(ui);
        }
        Hit::Shortcut => {
            ui.capturing = !ui.capturing;
            invalidate(ui);
        }
        Hit::UiLang(lang) => {
            settings::update(|s| s.ui_language = lang);
            i18n::set(lang);
            changed(ui);
        }
        Hit::OcrToggle => {
            settings::update(|s| s.vietnamese_ocr = !s.vietnamese_ocr);
            changed(ui);
        }
        Hit::Pick(i) => {
            if let Mode::Picker { query, .. } = &ui.mode {
                if let Some((code, _)) = matches(query).get(i) {
                    settings::update(|s| s.target_language = code.to_string());
                }
            }
            ui.mode = Mode::Rows;
            changed(ui);
        }
        Hit::Search | Hit::None => invalidate(ui),
    }
}

/// Keyboard: shortcut capture, and typing/navigation in the language picker.
fn key(ui: &mut Ui, vk: VIRTUAL_KEY) -> bool {
    let held = |k: VIRTUAL_KEY| unsafe { GetKeyState(k.0 as i32) < 0 };
    if ui.capturing {
        let modifiers = [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LSHIFT, VK_RSHIFT, VK_LCONTROL, VK_RCONTROL, VK_LMENU, VK_RMENU, VK_LWIN, VK_RWIN];
        if modifiers.contains(&vk) {
            return true; // wait for the main key
        }
        let (ctrl, shift, alt) = (held(VK_CONTROL), held(VK_SHIFT), held(VK_MENU));
        if !ctrl && !shift && !alt && vk == VK_ESCAPE {
            ui.capturing = false;
        } else if !ctrl && !shift && !alt && (vk == VK_BACK || vk == VK_DELETE) {
            settings::update(|s| s.translate_shortcut = Shortcut::default());
            ui.capturing = false;
            changed(ui);
        } else {
            let shortcut = Shortcut { ctrl, shift, alt, key: vk.0 };
            if shortcut.conflicts() {
                ui.error = Some(i18n::t().shortcut_taken);
            } else {
                settings::update(|s| s.translate_shortcut = shortcut);
                ui.capturing = false;
                ui.error = None;
                changed(ui);
            }
        }
        invalidate(ui);
        return true;
    }
    let Mode::Picker { query, scroll, highlight } = &mut ui.mode else {
        return false;
    };
    let count = matches(query).len();
    let l = layout(ui.hwnd);
    let row = LIST_ROW_H * l.s;
    let view = l.list.bottom - l.list.top;
    match vk {
        VK_ESCAPE => ui.mode = Mode::Rows,
        VK_RETURN => {
            let pick = *highlight;
            click(ui, Hit::Pick(pick));
            return true;
        }
        VK_DOWN | VK_UP => {
            if count > 0 {
                *highlight = if vk == VK_DOWN { (*highlight + 1).min(count - 1) } else { highlight.saturating_sub(1) };
                // Keep the highlighted row in view.
                let top = *highlight as f32 * row;
                if top < *scroll {
                    *scroll = top;
                } else if top + row > *scroll + view {
                    *scroll = top + row - view;
                }
            }
        }
        VK_BACK => {
            query.pop();
            *scroll = 0.0;
            *highlight = 0;
        }
        _ => return false,
    }
    invalidate(ui);
    true
}

fn type_char(ui: &mut Ui, c: char) {
    if let Mode::Picker { query, scroll, highlight } = &mut ui.mode {
        if !c.is_control() {
            query.push(c);
            *scroll = 0.0;
            *highlight = 0;
            invalidate(ui);
        }
    }
}

fn wheel(ui: &mut Ui, delta: i32) {
    if let Mode::Picker { query, scroll, .. } = &mut ui.mode {
        let l = layout(ui.hwnd);
        let content = matches(query).len() as f32 * LIST_ROW_H * l.s;
        let view = l.list.bottom - l.list.top;
        *scroll = (*scroll - delta as f32 / 120.0 * 3.0 * LIST_ROW_H * l.s).clamp(0.0, (content - view).max(0.0));
        invalidate(ui);
    }
}

// ------------------------------------------------------------------ drawing

struct Pen<'a> {
    target: &'a ID2D1HwndRenderTarget,
    dwrite: &'a IDWriteFactory,
    s: f32,
}

impl Pen<'_> {
    fn brush(&self, c: [u8; 3], a: f32) -> Result<ID2D1SolidColorBrush> {
        unsafe { self.target.CreateSolidColorBrush(&paint::rgba(c, a), None) }
    }

    fn rounded(&self, r: D2D_RECT_F, radius: f32, fill: Option<([u8; 3], f32)>, border: Option<([u8; 3], f32)>) -> Result<()> {
        let rr = D2D1_ROUNDED_RECT { rect: r, radiusX: radius * self.s, radiusY: radius * self.s };
        unsafe {
            if let Some((c, a)) = fill {
                self.target.FillRoundedRectangle(&rr, &self.brush(c, a)?);
            }
            if let Some((c, a)) = border {
                let inner = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F { left: r.left + 0.5, top: r.top + 0.5, right: r.right - 0.5, bottom: r.bottom - 0.5 },
                    ..rr
                };
                self.target.DrawRoundedRectangle(&inner, &self.brush(c, a)?, 1.0, None);
            }
        }
        Ok(())
    }

    /// Draws `text` vertically centered in `r`: left-aligned from `r.left`,
    /// or right-aligned to `r.right`. Returns its width.
    fn text(&self, text: &str, r: D2D_RECT_F, px: f32, weight: i32, color: [u8; 3], right: bool) -> Result<f32> {
        let layout = paint::text_layout(self.dwrite, text, px * self.s, weight)?;
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe { layout.GetMetrics(&mut m)? };
        let x = if right { r.right - m.widthIncludingTrailingWhitespace } else { r.left };
        let y = (r.top + r.bottom) / 2.0 - m.height / 2.0;
        unsafe { self.target.DrawTextLayout(Vector2 { X: x, Y: y }, &layout, &self.brush(color, 1.0)?, D2D1_DRAW_TEXT_OPTIONS_CLIP) };
        Ok(m.widthIncludingTrailingWhitespace)
    }

    fn toggle(&self, r: D2D_RECT_F, on: bool, enabled: bool) -> Result<()> {
        let (track, knob) = if on { (theme::TEXT, theme::WINDOW) } else { ([70, 70, 70], [170, 170, 170]) };
        let a = if enabled { 1.0 } else { 0.4 };
        let h = r.bottom - r.top;
        self.rounded(r, h / 2.0 / self.s, Some((track, a)), None)?;
        let d = h - 6.0 * self.s;
        let x = if on { r.right - 3.0 * self.s - d } else { r.left + 3.0 * self.s };
        let knob_r = D2D_RECT_F { left: x, top: r.top + 3.0 * self.s, right: x + d, bottom: r.top + 3.0 * self.s + d };
        self.rounded(knob_r, d / 2.0 / self.s, Some((knob, a)), None)
    }
}

fn paint(ui: &mut Ui) -> Result<()> {
    let hwnd = ui.hwnd;
    let mut rc = RECT::default();
    unsafe { GetClientRect(hwnd, &mut rc)? };
    let size = D2D_SIZE_U { width: (rc.right - rc.left) as u32, height: (rc.bottom - rc.top) as u32 };
    let drawn = paint::with_factories(|f| -> Result<()> {
        if ui.target.is_none() {
            let props = D2D1_RENDER_TARGET_PROPERTIES { dpiX: 96.0, dpiY: 96.0, ..Default::default() };
            let hprops = D2D1_HWND_RENDER_TARGET_PROPERTIES { hwnd, pixelSize: size, presentOptions: D2D1_PRESENT_OPTIONS_NONE };
            ui.target = Some(unsafe { f.d2d.CreateHwndRenderTarget(&props, &hprops)? });
        }
        let target = ui.target.as_ref().expect("created above");
        let pen = Pen { target, dwrite: &f.dwrite, s: scale(hwnd) };
        let l = layout(hwnd);
        let t = i18n::t();
        let prefs = settings::get();
        unsafe {
            target.BeginDraw();
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            target.Clear(Some(&paint::rgba(theme::PANEL, 1.0)));
        }
        let hover = |h: Hit| ui.hover == h;
        let row_text = |i: usize| D2D_RECT_F { left: l.rows[i].left + PAD * l.s, ..l.rows[i] };

        match &ui.mode {
            Mode::Rows => {
                for row in &l.rows[1..] {
                    let line = D2D_RECT_F { left: row.left + PAD * l.s, right: row.right - PAD * l.s, top: row.top, bottom: row.top + 1.0 };
                    unsafe { target.FillRectangle(&line, &pen.brush(theme::SEPARATOR.0, theme::SEPARATOR.1)?) };
                }
                // Start with Windows
                pen.text(t.start_with_windows, row_text(0), TEXT_PX, 400, theme::TEXT, false)?;
                pen.toggle(l.start_toggle, settings::start_with_windows(), true)?;

                // Translate into
                pen.text(t.translate_into, row_text(1), TEXT_PX, 400, theme::TEXT, false)?;
                let fill = if hover(Hit::Language) { Some((theme::HOVER.0, theme::HOVER.1)) } else { None };
                pen.rounded(l.language, theme::CONTROL_RADIUS, fill, Some((theme::CONTROL_BORDER.0, theme::CONTROL_BORDER.1)))?;
                let inner = D2D_RECT_F { left: l.language.left + 10.0 * l.s, right: l.language.right - 10.0 * l.s, ..l.language };
                pen.text(i18n::language_name(&prefs.target_language), inner, TEXT_PX, 400, theme::TEXT, false)?;
                pen.text("›", inner, TEXT_PX + 2.0, 400, theme::SUBTLE, true)?;

                // Translate shortcut
                let mut label_row = row_text(2);
                if let Some(error) = ui.error {
                    label_row.bottom = (label_row.top + label_row.bottom) / 2.0 + 2.0 * l.s;
                    let below = D2D_RECT_F { top: label_row.bottom - 2.0 * l.s, bottom: l.rows[2].bottom - 4.0 * l.s, ..row_text(2) };
                    pen.text(error, below, SMALL_PX - 1.0, 400, [235, 120, 110], false)?;
                }
                pen.text(t.translate_shortcut, label_row, TEXT_PX, 400, theme::TEXT, false)?;
                let border = if ui.capturing { (theme::TEXT, 0.9) } else { theme::CONTROL_BORDER };
                let fill = if hover(Hit::Shortcut) && !ui.capturing { Some((theme::HOVER.0, theme::HOVER.1)) } else { None };
                pen.rounded(l.shortcut, theme::CONTROL_RADIUS, fill, Some(border))?;
                let inner = D2D_RECT_F { left: l.shortcut.left + 10.0 * l.s, right: l.shortcut.right - 10.0 * l.s, ..l.shortcut };
                if ui.capturing {
                    pen.text(t.press_shortcut, inner, TEXT_PX, 400, theme::SUBTLE, false)?;
                } else {
                    pen.text(&prefs.translate_shortcut.label(), inner, TEXT_PX, 400, theme::TEXT, false)?;
                }

                // App language: segmented English | Tiếng Việt
                pen.text(t.app_language, row_text(3), TEXT_PX, 400, theme::TEXT, false)?;
                let ui_box = D2D_RECT_F { right: l.ui_vi.right, ..l.ui_en };
                pen.rounded(ui_box, theme::CONTROL_RADIUS, None, Some((theme::CONTROL_BORDER.0, theme::CONTROL_BORDER.1)))?;
                for (r, lang, label) in [(l.ui_en, UiLang::En, "English"), (l.ui_vi, UiLang::Vi, "Tiếng Việt")] {
                    let selected = prefs.ui_language == lang;
                    let seg = D2D_RECT_F { left: r.left + 2.0 * l.s, top: r.top + 2.0 * l.s, right: r.right - 2.0 * l.s, bottom: r.bottom - 2.0 * l.s };
                    if selected {
                        pen.rounded(seg, theme::CONTROL_RADIUS - 1.0, Some((theme::TEXT, 1.0)), None)?;
                    } else if hover(Hit::UiLang(lang)) {
                        pen.rounded(seg, theme::CONTROL_RADIUS - 1.0, Some((theme::HOVER.0, theme::HOVER.1)), None)?;
                    }
                    let lw = paint::measure(label, TEXT_PX * l.s, 400);
                    let centered = D2D_RECT_F { left: (r.left + r.right - lw) / 2.0, ..r };
                    pen.text(label, centered, TEXT_PX, 400, if selected { theme::WINDOW } else { theme::TEXT }, false)?;
                }

                // Vietnamese text recognition (needs the model)
                let model = crate::viet::model_path().is_some();
                let mut label_row = row_text(4);
                if !model {
                    label_row.bottom = (label_row.top + label_row.bottom) / 2.0 + 2.0 * l.s;
                    let below = D2D_RECT_F { top: label_row.bottom - 2.0 * l.s, bottom: l.rows[4].bottom - 4.0 * l.s, ..row_text(4) };
                    pen.text(t.vietnamese_ocr_missing, below, SMALL_PX - 1.0, 400, theme::SUBTLE, false)?;
                }
                pen.text(t.vietnamese_ocr, label_row, TEXT_PX, 400, if model { theme::TEXT } else { theme::SUBTLE }, false)?;
                pen.toggle(l.ocr_toggle, model && prefs.vietnamese_ocr, model)?;
            }
            Mode::Picker { query, scroll, highlight } => {
                // Search box
                pen.rounded(l.search, theme::CONTROL_RADIUS, None, Some((theme::TEXT, 0.5)))?;
                let inner = D2D_RECT_F { left: l.search.left + 10.0 * l.s, right: l.search.right - 10.0 * l.s, ..l.search };
                if query.is_empty() {
                    pen.text(t.search_languages, inner, TEXT_PX, 400, theme::SUBTLE, false)?;
                } else {
                    let w = pen.text(query, inner, TEXT_PX, 400, theme::TEXT, false)?;
                    let caret = D2D_RECT_F { left: inner.left + w + 1.0, right: inner.left + w + 2.0, top: inner.top + 8.0 * l.s, bottom: inner.bottom - 8.0 * l.s };
                    unsafe { target.FillRectangle(&caret, &pen.brush(theme::TEXT, 1.0)?) };
                }
                // List, clipped to its area
                let found = matches(query);
                unsafe { target.PushAxisAlignedClip(&l.list, D2D1_ANTIALIAS_MODE_ALIASED) };
                if found.is_empty() {
                    pen.text(t.no_match, D2D_RECT_F { left: l.list.left + 10.0 * l.s, bottom: l.list.top + LIST_ROW_H * l.s, ..l.list }, TEXT_PX, 400, theme::SUBTLE, false)?;
                }
                let row = LIST_ROW_H * l.s;
                let first = (*scroll / row) as usize;
                let visible = ((l.list.bottom - l.list.top) / row) as usize + 2;
                for (i, (code, name)) in found.iter().enumerate().skip(first).take(visible) {
                    let top = l.list.top + i as f32 * row - scroll;
                    let r = D2D_RECT_F { left: l.list.left, top, right: l.list.right, bottom: top + row };
                    if hover(Hit::Pick(i)) || i == *highlight {
                        pen.rounded(r, theme::HOVER_RADIUS, Some((theme::HOVER.0, theme::HOVER.1)), None)?;
                    }
                    let inner = D2D_RECT_F { left: r.left + 10.0 * l.s, right: r.right - 10.0 * l.s, ..r };
                    pen.text(name, inner, TEXT_PX, 400, theme::TEXT, false)?;
                    if *code == prefs.target_language {
                        pen.text("✓", inner, TEXT_PX, 600, theme::TEXT, true)?;
                    }
                }
                unsafe { target.PopAxisAlignedClip() };
            }
        }
        // Footer
        let footer = D2D_RECT_F { left: PAD * l.s, top: l.body.bottom, right: l.body.right - PAD * l.s, bottom: l.body.bottom + FOOTER_H * l.s };
        let hint = match ui.mode {
            Mode::Rows => format!("QuickCopy {}", env!("CARGO_PKG_VERSION")),
            Mode::Picker { .. } => "↑ ↓ Enter · Esc".to_string(),
        };
        pen.text(&hint, footer, SMALL_PX - 1.0, 400, theme::SUBTLE, false)?;
        unsafe { target.EndDraw(None, None) }
    });
    match drawn {
        Some(Err(e)) => {
            // Lost device: recreate the target next time.
            ui.target = None;
            Err(e)
        }
        _ => Ok(()),
    }
}

// ------------------------------------------------------------ window proc

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let (x, y) = ((lp.0 & 0xFFFF) as i16 as f32, ((lp.0 >> 16) & 0xFFFF) as i16 as f32);
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            unsafe { BeginPaint(hwnd, &mut ps) };
            with_ui(|ui| {
                if let Err(e) = paint(ui) {
                    crate::worker::log(format!("settings paint: {e}"));
                }
            });
            unsafe {
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEMOVE => {
            with_ui(|ui| {
                let hit = hit_test(ui, x, y);
                if hit != ui.hover {
                    ui.hover = hit;
                    invalidate(ui);
                }
                let cursor = if hit == Hit::None { IDC_ARROW } else { IDC_HAND };
                unsafe {
                    SetCursor(LoadCursorW(None, cursor).ok());
                }
            });
            LRESULT(0)
        }
        WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT => LRESULT(1),
        WM_LBUTTONDOWN => {
            with_ui(|ui| {
                let hit = hit_test(ui, x, y);
                click(ui, hit);
            });
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp.0 >> 16) & 0xFFFF) as i16 as i32;
            with_ui(|ui| wheel(ui, delta));
            LRESULT(0)
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let handled = with_ui(|ui| key(ui, VIRTUAL_KEY(wp.0 as u16))).unwrap_or(false);
            if handled { LRESULT(0) } else { unsafe { DefWindowProcW(hwnd, msg, wp, lp) } }
        }
        WM_CHAR => {
            if let Some(c) = char::from_u32(wp.0 as u32) {
                with_ui(|ui| type_char(ui, c));
            }
            LRESULT(0)
        }
        WM_DPICHANGED => {
            // Follow the suggested rect; the layout scales with the DPI.
            let r = unsafe { &*(lp.0 as *const RECT) };
            unsafe {
                let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            }
            with_ui(|ui| {
                ui.target = None;
                invalidate(ui);
            });
            LRESULT(0)
        }
        WM_SIZE => {
            with_ui(|ui| ui.target = None);
            LRESULT(0)
        }
        // The close button hides the window to the tray; Quit is in the tray menu.
        WM_CLOSE => {
            unsafe {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            UI.with(|u| u.borrow_mut().take());
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}
