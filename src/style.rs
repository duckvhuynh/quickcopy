//! Measures how text looks in pixels, so a translation can be drawn at the
//! same size and weight: x-height (or cap height), baseline and stroke width.
//!
//! The same measurement runs on the original text in the image and on sample
//! text we render ourselves, so the two compare like for like.

use windows::Win32::Foundation::RECT;

use crate::translate::Rgb;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ink {
    /// Height of the dense body band: x-height for lowercase text, cap
    /// height for text without lowercase letters.
    pub body: f32,
    /// Bottom of that band (the baseline), in pixels from the buffer's top.
    pub baseline: f32,
    /// Typical width of vertical strokes.
    pub stroke: f32,
}

/// How much of the text color a pixel holds: 0 = background, 1 = ink.
fn coverage(p: Rgb, bg: Rgb, fg: Rgb) -> f32 {
    let d: [f32; 3] = std::array::from_fn(|i| fg[i] as f32 - bg[i] as f32);
    let v: [f32; 3] = std::array::from_fn(|i| p[i] as f32 - bg[i] as f32);
    let len2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    if len2 < 1.0 {
        return 0.0;
    }
    ((v[0] * d[0] + v[1] * d[1] + v[2] * d[2]) / len2).clamp(0.0, 1.0)
}

/// Measures the text inside `rect` of a top-down BGRA buffer `w` x `h`.
/// `rect` should span the whole line height (ascenders to descenders).
pub fn measure(px: &[u8], w: i32, h: i32, rect: RECT, bg: Rgb, fg: Rgb) -> Option<Ink> {
    let (x0, x1) = (rect.left.max(0), rect.right.min(w));
    let (y0, y1) = (rect.top.max(0), rect.bottom.min(h));
    if x1 - x0 < 4 || y1 - y0 < 4 {
        return None;
    }
    let cov = |x: i32, y: i32| {
        let i = ((y * w + x) * 4) as usize;
        coverage([px[i + 2], px[i + 1], px[i]], bg, fg)
    };

    // Find the body band (x-height) by counting strokes crossing each row:
    // every letter has stems through the body, while only some reach above
    // or below it. Plain ink totals would peak on horizontal strokes (the
    // x-height line, the baseline) and, for light weights, give a band only
    // a couple of rows tall.
    let crossings: Vec<usize> = (y0..y1)
        .map(|y| {
            let mut n = 0;
            let mut inside = false;
            for x in x0..x1 {
                let ink = cov(x, y) > 0.5;
                if ink && !inside {
                    n += 1;
                }
                inside = ink;
            }
            n
        })
        .collect();
    let (peak_i, &peak) = crossings.iter().enumerate().max_by_key(|&(_, n)| *n)?;
    if peak == 0 {
        return None;
    }
    let mut top = peak_i;
    while top > 0 && crossings[top - 1] * 2 >= peak {
        top -= 1;
    }
    let mut bottom = peak_i;
    while bottom + 1 < crossings.len() && crossings[bottom + 1] * 2 >= peak {
        bottom += 1;
    }

    // Sub-pixel edges. Band rows count whole; the antialiased row just
    // outside adds the share of a body row it holds, after removing what the
    // row beyond it holds (ascenders or descenders passing through).
    let rows: Vec<f32> = (y0..y1).map(|y| (x0..x1).map(|x| cov(x, y)).sum::<f32>()).collect();
    let mut band: Vec<f32> = rows[top..=bottom].to_vec();
    band.sort_by(f32::total_cmp);
    let level = band[band.len() / 2];
    let partial = |next: Option<usize>, beyond: Option<usize>| -> f32 {
        let Some(n) = next else { return 0.0 };
        let base = beyond.map_or(0.0, |b| rows[b]);
        if level <= base {
            return 0.0;
        }
        ((rows[n] - base) / (level - base)).clamp(0.0, 1.0)
    };
    let len = rows.len();
    let above = top.checked_sub(1);
    let below = (bottom + 1 < len).then_some(bottom + 1);
    let top_edge = top as f32 - partial(above, top.checked_sub(2));
    let bottom_edge = (bottom + 1) as f32 + partial(below, (bottom + 2 < len).then_some(bottom + 2));
    let body = bottom_edge - top_edge;
    if body < 2.0 {
        return None;
    }

    // Stroke: ink widths of horizontal runs through the middle of the band.
    let mut runs = Vec::new();
    let (m0, m1) = (top + (bottom - top) / 4, bottom - (bottom - top) / 4);
    for y in (y0 + m0 as i32)..=(y0 + m1 as i32) {
        let mut run = 0.0f32;
        for x in x0..x1 {
            let c = cov(x, y);
            if c > 0.15 {
                run += c;
            } else if run > 0.0 {
                runs.push(run);
                run = 0.0;
            }
        }
        if run > 0.0 {
            runs.push(run);
        }
    }
    if runs.is_empty() {
        return None;
    }
    runs.sort_by(f32::total_cmp);
    let stroke = runs[runs.len() / 2];
    Some(Ink { body, baseline: y0 as f32 + bottom_edge, stroke })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// White canvas; `ink` paints solid black rectangles.
    fn canvas(w: i32, h: i32, ink: &[(i32, i32, i32, i32)]) -> Vec<u8> {
        let mut px = vec![255u8; (w * h * 4) as usize];
        for &(l, t, r, b) in ink {
            for y in t..b {
                for x in l..r {
                    let i = ((y * w + x) * 4) as usize;
                    px[i..i + 3].copy_from_slice(&[0, 0, 0]);
                }
            }
        }
        px
    }

    #[test]
    fn finds_body_baseline_and_stroke() {
        // A line of "letters": 3 px stems, body rows 10..20, an ascender to
        // row 4 and a descender to row 24 on a few of them.
        let mut ink = Vec::new();
        for i in 0..12 {
            let x = 4 + i * 8;
            let (top, bottom) = match i % 4 {
                0 => (4, 20),
                3 => (10, 24),
                _ => (10, 20),
            };
            ink.push((x, top, x + 3, bottom));
        }
        let px = canvas(110, 30, &ink);
        let m = measure(&px, 110, 30, RECT { left: 0, top: 0, right: 110, bottom: 30 }, [255; 3], [0; 3]).unwrap();
        assert!((m.body - 10.0).abs() < 0.6, "{m:?}");
        assert!((m.baseline - 20.0).abs() < 0.6, "{m:?}");
        assert!((m.stroke - 3.0).abs() < 0.01, "{m:?}");
    }

    #[test]
    fn thicker_strokes_measure_thicker() {
        let thin: Vec<_> = (0..10).map(|i| (4 + i * 10, 10, 6 + i * 10, 20)).collect();
        let bold: Vec<_> = (0..10).map(|i| (4 + i * 10, 10, 9 + i * 10, 20)).collect();
        let r = RECT { left: 0, top: 0, right: 110, bottom: 30 };
        let a = measure(&canvas(110, 30, &thin), 110, 30, r, [255; 3], [0; 3]).unwrap();
        let b = measure(&canvas(110, 30, &bold), 110, 30, r, [255; 3], [0; 3]).unwrap();
        assert_eq!((a.stroke, b.stroke), (2.0, 5.0));
    }

    #[test]
    fn nothing_to_measure_on_blank() {
        let px = canvas(50, 20, &[]);
        assert_eq!(measure(&px, 50, 20, RECT { left: 0, top: 0, right: 50, bottom: 20 }, [255; 3], [0; 3]), None);
    }
}
