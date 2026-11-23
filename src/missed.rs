//! Finds text lines Windows OCR left out, so they can be read on their own.
//!
//! Windows OCR finds lines before reading them, and on tightly stacked
//! lines it sometimes skips whole ones: an underline running just above
//! the next line's letters is enough (a list of underlined `/command`
//! links read 18 of 32). The same line cut out with empty space around it
//! is read correctly. So after the first read, what still looks like text
//! (letter strokes outside every word found) is cut into lines, the lines
//! are laid out one under another with space between them on a new image,
//! and that image is read too.

/// A rectangle in pixels: left, top, right, bottom (exclusive).
pub type Band = (usize, usize, usize, usize);

/// Color difference (|dR|+|dG|+|dB|) two pixels apart that counts as a
/// letter's edge. Letters have many such edges side by side; an underline
/// or a table rule has none along its length.
const EDGE: i32 = 90;
/// Grid cell, in pixels of the (2x upscaled) capture.
const CELL_W: usize = 8;
const CELL_H: usize = 4;
/// Edges a cell needs to count as ink.
const CELL_EDGES: u32 = 3;
/// Cells apart (horizontally) that still belong to one line: a word gap.
const GAP_CELLS: usize = 3;
/// Line height range, in pixels of the 2x capture (text 6-45 px tall).
const MIN_H: usize = 12;
const MAX_H: usize = 90;
/// More candidate lines than fit in this many pixels of sheets is a photo's
/// texture (leaves, a brick wall, a chalkboard), not text Windows OCR
/// skipped: reading them took up to 0.7 s (1.9 s before sheets were
/// made narrow) and gave back a few junk words,
/// so none are returned. Time follows the area read.
pub const MAX_AREA: usize = 5_000_000;
/// Rows of a component with at most this share of its busiest row's
/// edges separate two lines.
const VALLEY: f32 = 0.08;

fn diff(px: &[u8], i: usize, j: usize) -> i32 {
    (px[i] as i32 - px[j] as i32).abs() + (px[i + 1] as i32 - px[j + 1] as i32).abs() + (px[i + 2] as i32 - px[j + 2] as i32).abs()
}

/// Lines of text-like ink in a BGRA image that no box in `found` covers
/// (none when their sheets would be larger than `MAX_AREA`).
pub fn bands(px: &[u8], w: usize, h: usize, found: &[Band]) -> Vec<Band> {
    if w < 16 || h < MIN_H {
        return Vec::new();
    }
    // Edge mask, with the words already read (and a little around them,
    // for the bits of letters their boxes leave out) cleared.
    let mut edge = vec![false; w * h];
    for y in 0..h {
        for x in 0..w - 2 {
            let i = (y * w + x) * 4;
            edge[y * w + x] = diff(px, i, i + 8) >= EDGE;
        }
    }
    for &(l, t, r, b) in found {
        let pad = ((b - t) / 6).clamp(2, 8);
        for y in t.saturating_sub(pad)..(b + pad).min(h) {
            for x in l.saturating_sub(pad)..(r + pad).min(w) {
                edge[y * w + x] = false;
            }
        }
    }

    // Cells with ink, grouped into components (a word gap apart at most).
    let (gw, gh) = (w / CELL_W, h / CELL_H);
    let mut count = vec![0u32; gw * gh];
    for y in 0..gh * CELL_H {
        for x in 0..gw * CELL_W {
            if edge[y * w + x] {
                count[(y / CELL_H) * gw + x / CELL_W] += 1;
            }
        }
    }
    let inked: Vec<bool> = count.iter().map(|&c| c >= CELL_EDGES).collect();
    let mut parent: Vec<usize> = (0..gw * gh).collect();
    fn root(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    for cy in 0..gh {
        for cx in 0..gw {
            let i = cy * gw + cx;
            if !inked[i] {
                continue;
            }
            for (dx, dy) in (1..=GAP_CELLS).map(|d| (d, 0)).chain([(0, 1), (1, 1)]) {
                let (nx, ny) = (cx + dx, cy + dy);
                if nx < gw && ny < gh && inked[ny * gw + nx] {
                    let (a, b) = (root(&mut parent, i), root(&mut parent, ny * gw + nx));
                    parent[a] = b;
                }
            }
        }
    }
    let mut boxes: std::collections::HashMap<usize, Band> = Default::default();
    for cy in 0..gh {
        for cx in 0..gw {
            let i = cy * gw + cx;
            if inked[i] {
                let r = root(&mut parent, i);
                let b = boxes.entry(r).or_insert((cx, cy, cx + 1, cy + 1));
                *b = (b.0.min(cx), b.1.min(cy), b.2.max(cx + 1), b.3.max(cy + 1));
            }
        }
    }

    // Each component cut into lines at the rows without letter edges.
    let mut out = Vec::new();
    for (cl, ct, cr, cb) in boxes.into_values() {
        let (l, t, r, b) = (cl * CELL_W, ct * CELL_H, (cr * CELL_W).min(w), (cb * CELL_H).min(h));
        let rows: Vec<u32> = (t..b).map(|y| edge[y * w + l..y * w + r].iter().filter(|&&e| e).count() as u32).collect();
        for (y0, y1) in lines_of(&rows, 0, rows.len(), VALLEY).into_iter().map(|(a, z)| (t + a, t + z)) {
            // The line's own left and right, from its edges.
            let cols = (l..r).filter(|&x| (y0..y1).any(|yy| edge[yy * w + x]));
            let (Some(x0), Some(x1)) = (cols.clone().min(), cols.max()) else { continue };
            let x1 = x1 + 3;
            if x1 - x0 >= (y1 - y0) * 3 / 2 {
                out.push((x0, y0, x1.min(w), y1));
            }
        }
    }
    if out.iter().map(|b| outer(b).0 * outer(b).1).sum::<usize>() > MAX_AREA {
        return Vec::new();
    }
    out.sort_by_key(|b| (b.1, b.0));
    out
}

/// The runs of `rows[from..to]` (edge counts per row) that are one line of
/// text tall, split at rows with at most `valley` of the busiest row's
/// edges. A run too tall for one line is split again with a higher bar:
/// a label right above a bar of a chart is joined to it by the bar's
/// sides, a few edges per row against the label's many.
fn lines_of(rows: &[u32], from: usize, to: usize, valley: f32) -> Vec<(usize, usize)> {
    let peak = rows[from..to].iter().copied().max().unwrap_or(0);
    if peak == 0 {
        return Vec::new();
    }
    let low = (peak as f32 * valley) as u32;
    let mut out = Vec::new();
    let mut y = from;
    while y < to {
        if rows[y] <= low {
            y += 1;
            continue;
        }
        let start = y;
        while y < to && rows[y] > low {
            y += 1;
        }
        match y - start {
            h if (MIN_H..=MAX_H).contains(&h) => out.push((start, y)),
            h if h > MAX_H && valley < 0.3 => out.extend(lines_of(rows, start, y, valley * 2.0)),
            _ => {}
        }
    }
    out
}

/// Lines cut out of an image and laid out one under another, each on its
/// own background with space around it.
pub struct Sheet {
    pub px: Vec<u8>,
    pub w: usize,
    pub h: usize,
    /// Where each line went: its band in the source, and its top-left on
    /// the sheet.
    pub placed: Vec<(Band, (usize, usize))>,
}

/// Space around a line on a sheet: its own height, at least 8 px.
fn margin(b: &Band) -> usize {
    (b.3 - b.1).max(8)
}

/// A line's size on a sheet, with its margins.
fn outer(b: &Band) -> (usize, usize) {
    (b.2 - b.0 + 2 * margin(b), b.3 - b.1 + 2 * margin(b))
}

/// Lays `bands` of `px` out on sheets no taller than `max_h`. Lines of
/// about the same width share a sheet as wide as the widest of them, so a
/// few long lines do not make every sheet wide (a photo's candidates once
/// made 87 Mpx of sheets for 2 Mpx of lines).
pub fn sheets(px: &[u8], w: usize, bands: &[Band], max_h: usize) -> Vec<Sheet> {
    let mut order: Vec<Band> = bands.iter().copied().filter(|b| outer(b).1 <= max_h).collect();
    order.sort_by_key(|b| std::cmp::Reverse(outer(b).0));
    let mut out: Vec<Sheet> = Vec::new();
    let mut start = 0;
    while start < order.len() {
        let width = outer(&order[start]).0;
        let mut end = start;
        let mut height = 0;
        while end < order.len() && height + outer(&order[end]).1 <= max_h && outer(&order[end]).0 * 2 > width {
            height += outer(&order[end]).1;
            end += 1;
        }
        let mut sheet = Sheet { px: vec![0; width * height * 4], w: width, h: height, placed: Vec::new() };
        let mut y = 0;
        for b in &order[start..end] {
            let m = margin(b);
            let (bw, bh) = (b.2 - b.0, b.3 - b.1);
            // The line's background: the most common of its border pixels.
            let bg = background(px, w, b);
            for p in sheet.px[y * width * 4..(y + bh + 2 * m) * width * 4].chunks_exact_mut(4) {
                p.copy_from_slice(&bg);
            }
            for row in 0..bh {
                let src = ((b.1 + row) * w + b.0) * 4;
                let dst = ((y + m + row) * width + m) * 4;
                sheet.px[dst..dst + bw * 4].copy_from_slice(&px[src..src + bw * 4]);
            }
            sheet.placed.push((*b, (m, y + m)));
            y += bh + 2 * m;
        }
        out.push(sheet);
        start = end;
    }
    out
}

/// The most common color on the edge of `b` (quantized), as BGRA.
fn background(px: &[u8], w: usize, b: &Band) -> [u8; 4] {
    let mut counts: std::collections::HashMap<[u8; 3], (u32, [u8; 4])> = Default::default();
    let mut add = |x: usize, y: usize| {
        let i = (y * w + x) * 4;
        let p = [px[i], px[i + 1], px[i + 2], 255];
        let e = counts.entry([p[0] >> 3, p[1] >> 3, p[2] >> 3]).or_insert((0, p));
        e.0 += 1;
    };
    for x in b.0..b.2 {
        add(x, b.1);
        add(x, b.3 - 1);
    }
    for y in b.1..b.3 {
        add(b.0, y);
        add(b.2 - 1, y);
    }
    // Ties go to one color always (not to the map's order, which changes).
    counts.into_values().max_by_key(|e| (e.0, e.1)).map_or([255; 4], |e| e.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A BGRA canvas with "text": runs of vertical strokes.
    fn canvas(w: usize, h: usize) -> Vec<u8> {
        vec![30; w * h * 4]
    }
    fn strokes(px: &mut [u8], w: usize, x0: usize, x1: usize, y0: usize, y1: usize) {
        for y in y0..y1 {
            for x in (x0..x1).step_by(6) {
                for dx in 0..2 {
                    let i = (y * w + x + dx) * 4;
                    px[i..i + 3].copy_from_slice(&[230, 230, 230]);
                }
            }
        }
    }
    fn rule(px: &mut [u8], w: usize, x0: usize, x1: usize, y: usize) {
        for yy in y..y + 3 {
            for x in x0..x1 {
                let i = (yy * w + x) * 4;
                px[i..i + 3].copy_from_slice(&[230, 230, 230]);
            }
        }
    }

    #[test]
    fn finds_stacked_underlined_lines_that_were_not_read() {
        let (w, h) = (600, 200);
        let mut px = canvas(w, h);
        // Three lines 40 px apart, each underlined just above the next.
        for (i, y) in [20, 60, 100].into_iter().enumerate() {
            strokes(&mut px, w, 40, 300 + i * 20, y, y + 28);
            rule(&mut px, w, 40, 300 + i * 20, y + 31);
        }
        // The middle one was read; the others were not.
        let got = bands(&px, w, h, &[(40, 60, 320, 88)]);
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(got[0].1.abs_diff(20) <= 4 && got[0].3.abs_diff(48) <= 4, "{got:?}");
        assert!(got[1].1.abs_diff(100) <= 4 && got[1].3.abs_diff(128) <= 4, "{got:?}");
    }

    #[test]
    fn finds_a_label_sitting_on_a_chart_bar() {
        let (w, h) = (400, 300);
        let mut px = canvas(w, h);
        strokes(&mut px, w, 100, 220, 60, 84);
        // A solid bar right under the label, its sides edges in every row.
        for y in 88..290 {
            for x in 110..200 {
                let i = (y * w + x) * 4;
                px[i..i + 3].copy_from_slice(&[180, 120, 60]);
            }
        }
        let got = bands(&px, w, h, &[]);
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(got[0].1.abs_diff(60) <= 4 && got[0].3.abs_diff(84) <= 4, "{got:?}");
    }

    #[test]
    fn nothing_when_everything_was_read() {
        let (w, h) = (400, 100);
        let mut px = canvas(w, h);
        strokes(&mut px, w, 20, 300, 30, 58);
        assert!(bands(&px, w, h, &[(20, 30, 300, 58)]).is_empty());
    }

    #[test]
    fn rules_and_flat_areas_are_not_text() {
        let (w, h) = (400, 120);
        let mut px = canvas(w, h);
        rule(&mut px, w, 10, 390, 40);
        rule(&mut px, w, 10, 390, 80);
        assert!(bands(&px, w, h, &[]).is_empty());
    }

    #[test]
    fn the_background_of_a_line_is_the_same_every_time() {
        // Two colors equally common on the line's border: which one was
        // picked followed the hash map's order, so the same image read
        // differently from one hover to the next.
        let (w, h) = (10, 2);
        let mut px = canvas(w, h);
        for x in 0..w {
            px[(w + x) * 4..][..3].copy_from_slice(&[200, 200, 200]);
        }
        let first = background(&px, w, &(0, 0, w, h));
        assert!((0..64).all(|_| background(&px, w, &(0, 0, w, h)) == first));
    }

    #[test]
    fn sheets_put_each_line_on_its_own_background() {
        let (w, h) = (300, 100);
        let mut px = canvas(w, h);
        strokes(&mut px, w, 20, 200, 10, 30);
        strokes(&mut px, w, 20, 150, 50, 70);
        let bands = [(20, 10, 200, 30), (20, 50, 150, 70)];
        let s = sheets(&px, w, &bands, 10_000);
        assert_eq!(s.len(), 1);
        let sheet = &s[0];
        assert_eq!(sheet.placed.len(), 2);
        // Lines are 20 px tall with 20 px of margin on every side.
        assert_eq!(sheet.h, 2 * (20 + 40));
        let (_, (x, y)) = sheet.placed[1];
        assert_eq!((x, y), (20, 80));
        // Margins are background, the line's pixels are copied.
        assert_eq!(sheet.px[0], 30);
        let i = ((y) * sheet.w + x) * 4;
        assert_eq!(sheet.px[i], 230);
    }
}
