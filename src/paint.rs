//! Draws translated text over the overlay's bitmap with Direct2D/DirectWrite.
//! GDI text would wipe the per-pixel alpha the layered overlay needs;
//! Direct2D writes premultiplied alpha and shapes Vietnamese properly.
//!
//! It also matches the original text's look: `match_style` finds the font
//! weight and size whose rendering measures like the text in the image.

use std::cell::OnceCell;
use std::ffi::c_void;

use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::core::*;
use windows_numerics::Vector2;

use crate::style;
use crate::translate::Rgb;

const FONT: PCWSTR = w!("Segoe UI");
/// Weights tried when matching the original (Segoe UI has all of them).
const WEIGHTS: [i32; 5] = [300, 400, 600, 700, 900];

/// One box of translated text: its background is painted over `fill`
/// (hiding the original), then `text` is drawn from the fill's left edge
/// with its baseline on `baseline` (or vertically centered without one).
pub struct DrawBox {
    pub fill: RECT,
    pub background: Rgb,
    pub foreground: Rgb,
    pub text: String,
    pub size: f32,
    pub weight: i32,
    pub baseline: Option<f32>,
}

pub(crate) struct Factories {
    pub d2d: ID2D1Factory,
    pub dwrite: IDWriteFactory,
}

thread_local! {
    static FACTORIES: OnceCell<Option<Factories>> = const { OnceCell::new() };
}

pub(crate) fn with_factories<R>(f: impl FnOnce(&Factories) -> R) -> Option<R> {
    FACTORIES.with(|cell| {
        cell.get_or_init(|| unsafe {
            let d2d = D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
            let dwrite = DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED).ok()?;
            Some(Factories { d2d, dwrite })
        })
        .as_ref()
        .map(f)
    })
}

fn text_format(dwrite: &IDWriteFactory, size: f32, weight: i32) -> Result<IDWriteTextFormat> {
    unsafe {
        let format = dwrite.CreateTextFormat(
            FONT,
            None,
            DWRITE_FONT_WEIGHT(weight),
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size.max(1.0),
            w!("vi-vn"),
        )?;
        format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        Ok(format)
    }
}

pub(crate) fn text_layout(dwrite: &IDWriteFactory, text: &str, size: f32, weight: i32) -> Result<IDWriteTextLayout> {
    let format = text_format(dwrite, size, weight)?;
    let wide: Vec<u16> = text.encode_utf16().collect();
    unsafe { dwrite.CreateTextLayout(&wide, &format, 100_000.0, 10_000.0) }
}

/// Width in pixels of `text` at font size `size` (pixels) and `weight`.
pub fn measure(text: &str, size: f32, weight: i32) -> f32 {
    with_factories(|f| unsafe {
        let layout = text_layout(&f.dwrite, text, size, weight).ok()?;
        let mut metrics = DWRITE_TEXT_METRICS::default();
        layout.GetMetrics(&mut metrics).ok()?;
        Some(metrics.widthIncludingTrailingWhitespace)
    })
    .flatten()
    .unwrap_or(text.chars().count() as f32 * size * 0.5)
}

/// Distance from a layout's top to its first baseline.
fn layout_baseline(layout: &IDWriteTextLayout) -> f32 {
    unsafe {
        let mut lines = [DWRITE_LINE_METRICS::default(); 1];
        let mut count = 0u32;
        let _ = layout.GetLineMetrics(Some(&mut lines), &mut count);
        lines[0].baseline
    }
}

/// Font size and weight that make our font look like the measured text.
pub struct Matched {
    pub size: f32,
    pub weight: i32,
}

/// Renders `sample` (the original words, so the letters match) at each
/// weight, sized so its body height equals `body`, measures it the same way
/// the image was measured, and keeps the weight whose stroke-to-body ratio
/// is closest to the original's.
pub fn match_style(sample: &str, original: style::Ink) -> Option<Matched> {
    let want = original.stroke / original.body;
    let mut best: Option<(f32, Matched)> = None;
    for weight in WEIGHTS {
        // Size from a first render at a reference size, then refined once.
        let reference = 40.0;
        let ink = render_ink(sample, reference, weight)?;
        let size = reference * original.body / ink.body;
        let ink = render_ink(sample, size, weight)?;
        let size = size * original.body / ink.body;
        let error = (ink.stroke / ink.body - want).abs();
        if best.as_ref().is_none_or(|(e, _)| error < *e) {
            best = Some((error, Matched { size, weight }));
        }
    }
    best.map(|(_, m)| m)
}

/// Draws `text` black on white into a temporary bitmap and measures it.
fn render_ink(text: &str, size: f32, weight: i32) -> Option<style::Ink> {
    with_factories(|f| unsafe {
        let layout = text_layout(&f.dwrite, text, size, weight).ok()?;
        let mut metrics = DWRITE_TEXT_METRICS::default();
        layout.GetMetrics(&mut metrics).ok()?;
        let (w, h) = ((metrics.widthIncludingTrailingWhitespace.ceil() as i32 + 8).max(8), (metrics.height.ceil() as i32 + 8).max(8));
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
        let result = (|| {
            let dib = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
            let old = SelectObject(mem, dib.into());
            let target = dc_target(&f.d2d, mem, w, h).ok()?;
            target.BeginDraw();
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            target.Clear(Some(&color([255, 255, 255])));
            let black = target.CreateSolidColorBrush(&color([0, 0, 0]), None).ok()?;
            target.DrawTextLayout(Vector2 { X: 4.0, Y: 4.0 }, &layout, &black, D2D1_DRAW_TEXT_OPTIONS_NONE);
            target.EndDraw(None, None).ok()?;
            let px = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
            let ink = style::measure(px, w, h, RECT { left: 0, top: 0, right: w, bottom: h }, [255, 255, 255], [0, 0, 0]);
            SelectObject(mem, old);
            let _ = DeleteObject(dib.into());
            ink
        })();
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        result
    })
    .flatten()
}

pub(crate) fn dc_target(d2d: &ID2D1Factory, dc: HDC, w: i32, h: i32) -> Result<ID2D1DCRenderTarget> {
    let props = D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
        // 96 DPI: one Direct2D unit is one overlay pixel.
        dpiX: 96.0,
        dpiY: 96.0,
        usage: D2D1_RENDER_TARGET_USAGE_NONE,
        minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
    };
    unsafe {
        let target = d2d.CreateDCRenderTarget(&props)?;
        target.BindDC(dc, &RECT { left: 0, top: 0, right: w, bottom: h })?;
        Ok(target)
    }
}

fn color(c: Rgb) -> D2D1_COLOR_F {
    rgba(c, 1.0)
}

pub(crate) fn rgba(c: Rgb, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: c[0] as f32 / 255.0, g: c[1] as f32 / 255.0, b: c[2] as f32 / 255.0, a }
}

/// Draws `boxes` into the 32-bit bitmap selected into `dc` (size w x h).
pub fn draw(dc: HDC, w: i32, h: i32, boxes: &[DrawBox]) -> Result<()> {
    with_factories(|f| unsafe {
        let target = dc_target(&f.d2d, dc, w, h)?;
        target.BeginDraw();
        // ClearType needs an opaque target; grayscale works with alpha.
        target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        let rect = |b: &DrawBox| D2D_RECT_F { left: b.fill.left as f32, top: b.fill.top as f32, right: b.fill.right as f32, bottom: b.fill.bottom as f32 };
        // Hide all the original text first, then draw every line on top
        // without a background: filling box by box let one line's fill cut
        // the tone marks and descenders off the line drawn before it.
        for b in boxes {
            let background = target.CreateSolidColorBrush(&color(b.background), None)?;
            target.FillRectangle(&rect(b), &background);
        }
        for b in boxes {
            let r = rect(b);
            if b.text.is_empty() {
                continue;
            }
            let foreground = target.CreateSolidColorBrush(&color(b.foreground), None)?;
            let layout = text_layout(&f.dwrite, &b.text, b.size, b.weight)?;
            let top = match b.baseline {
                Some(baseline) => baseline - layout_baseline(&layout),
                None => {
                    let mut m = DWRITE_TEXT_METRICS::default();
                    layout.GetMetrics(&mut m)?;
                    r.top + (r.bottom - r.top - m.height) / 2.0
                }
            };
            target.DrawTextLayout(Vector2 { X: r.left + 2.0, Y: top }, &layout, &foreground, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
        target.EndDraw(None, None)
    })
    .unwrap_or_else(|| Err(Error::from_hresult(windows::Win32::Foundation::E_FAIL)))
}

/// Paints a layered window: a transparent `w` x `h` bitmap that `draw` fills
/// with Direct2D, shown at `pos`. Used by the popover and the tray menu.
pub(crate) fn update_layered(hwnd: HWND, pos: POINT, w: i32, h: i32, draw: impl FnOnce(&ID2D1DCRenderTarget, &Factories) -> Result<()>) {
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
            let drawn = with_factories(|f| -> Result<()> {
                let target = dc_target(&f.d2d, mem, w, h)?;
                target.BeginDraw();
                target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
                draw(&target, f)?;
                target.EndDraw(None, None)
            });
            if let Some(Err(e)) = drawn {
                crate::worker::log(format!("drawing a layered window failed: {e}"));
            }
            let size = SIZE { cx: w, cy: h };
            let src = POINT::default();
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            let _ = UpdateLayeredWindow(hwnd, Some(screen), Some(&pos), Some(&size), Some(mem), Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA);
            SelectObject(mem, old);
            let _ = DeleteObject(dib.into());
        }
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
    }
}

/// The rounded, bordered panel the popover and the tray menu sit on.
pub(crate) fn panel(target: &ID2D1DCRenderTarget, r: D2D_RECT_F, s: f32) -> Result<()> {
    use crate::theme;
    let p = theme::get();
    let radius = theme::RADIUS * s;
    let rounded = |r: D2D_RECT_F| D2D1_ROUNDED_RECT { rect: r, radiusX: radius, radiusY: radius };
    unsafe {
        target.FillRoundedRectangle(&rounded(r), &target.CreateSolidColorBrush(&rgba(p.panel, 0.98), None)?);
        let edge = D2D_RECT_F { left: r.left + 0.5, top: r.top + 0.5, right: r.right - 0.5, bottom: r.bottom - 0.5 };
        target.DrawRoundedRectangle(&rounded(edge), &target.CreateSolidColorBrush(&rgba(p.border.0, p.border.1), None)?, 1.0, None);
    }
    Ok(())
}
