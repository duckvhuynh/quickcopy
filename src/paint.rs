//! Draws translated text over the overlay's bitmap with Direct2D/DirectWrite.
//! GDI text would wipe the per-pixel alpha the layered overlay needs;
//! Direct2D writes premultiplied alpha and shapes Vietnamese properly.

use std::cell::OnceCell;

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::HDC;
use windows::core::*;

use crate::translate::Rgb;

const FONT: PCWSTR = w!("Segoe UI");

/// One box of translated text: its background is painted over `fill`
/// (hiding the original), then `text` is drawn left-aligned and vertically
/// centered in `fill`.
pub struct DrawBox {
    pub fill: RECT,
    pub background: Rgb,
    pub foreground: Rgb,
    pub text: String,
    pub size: f32,
}

struct Factories {
    d2d: ID2D1Factory,
    dwrite: IDWriteFactory,
}

thread_local! {
    static FACTORIES: OnceCell<Option<Factories>> = const { OnceCell::new() };
}

fn with_factories<R>(f: impl FnOnce(&Factories) -> R) -> Option<R> {
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

fn text_format(dwrite: &IDWriteFactory, size: f32) -> Result<IDWriteTextFormat> {
    unsafe {
        let format = dwrite.CreateTextFormat(
            FONT,
            None,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size.max(1.0),
            w!("vi-vn"),
        )?;
        format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        Ok(format)
    }
}

/// Width in pixels of `text` at font size `size` (pixels).
pub fn measure(text: &str, size: f32) -> f32 {
    with_factories(|f| unsafe {
        let format = text_format(&f.dwrite, size).ok()?;
        let wide: Vec<u16> = text.encode_utf16().collect();
        let layout = f.dwrite.CreateTextLayout(&wide, &format, 100_000.0, 10_000.0).ok()?;
        let mut metrics = DWRITE_TEXT_METRICS::default();
        layout.GetMetrics(&mut metrics).ok()?;
        Some(metrics.widthIncludingTrailingWhitespace)
    })
    .flatten()
    .unwrap_or(text.chars().count() as f32 * size * 0.5)
}

fn color(c: Rgb) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: c[0] as f32 / 255.0, g: c[1] as f32 / 255.0, b: c[2] as f32 / 255.0, a: 1.0 }
}

/// Draws `boxes` into the 32-bit bitmap selected into `dc` (size w x h).
pub fn draw(dc: HDC, w: i32, h: i32, boxes: &[DrawBox]) -> Result<()> {
    with_factories(|f| unsafe {
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            // 96 DPI: one Direct2D unit is one overlay pixel.
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let target = f.d2d.CreateDCRenderTarget(&props)?;
        target.BindDC(dc, &RECT { left: 0, top: 0, right: w, bottom: h })?;
        target.BeginDraw();
        // ClearType needs an opaque target; grayscale works with alpha.
        target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        for b in boxes {
            let r = D2D_RECT_F { left: b.fill.left as f32, top: b.fill.top as f32, right: b.fill.right as f32, bottom: b.fill.bottom as f32 };
            let background = target.CreateSolidColorBrush(&color(b.background), None)?;
            target.FillRectangle(&r, &background);
            if b.text.is_empty() {
                continue;
            }
            let foreground = target.CreateSolidColorBrush(&color(b.foreground), None)?;
            let format = text_format(&f.dwrite, b.size)?;
            let wide: Vec<u16> = b.text.encode_utf16().collect();
            target.DrawText(&wide, &format, &r, &foreground, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        }
        target.EndDraw(None, None)
    })
    .unwrap_or_else(|| Err(Error::from_hresult(windows::Win32::Foundation::E_FAIL)))
}
