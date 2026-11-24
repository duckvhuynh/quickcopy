//! Finds the bounds of an image from pixels alone, for apps that expose no
//! accessibility tree (Electron apps such as Zalo) and pictures a web page
//! leaves out of its tree.
//!
//! Chat apps draw images as rectangles on a flat background; photo viewers
//! on a blurred copy of the photo. Starting at the cursor we look left and
//! right for a long vertical edge whose outer side is flat or changes only
//! gradually, take the image height from those edges, refine top and bottom with
//! horizontal edges, and accept the result only if the background left and
//! right of it is the same color. That last check rejects list-row
//! highlights and panels, whose two sides sit on different backgrounds.

/// Min color difference (|dR|+|dG|+|dB|) across a boundary.
const EDGE: i32 = 24;
/// Max deviation from the mean for a pixel to count as flat background.
const FLAT: i32 = 18;
/// Max color difference between neighbouring pixels of a blurred background.
const SMOOTH: i32 = 6;
/// Consecutive non-edge pixels tolerated along a boundary.
const GAP: usize = 6;
/// Shortest image side we accept, in physical pixels.
const MIN_SIDE: usize = 40;
/// How far the top/bottom refinement searches around the side edges' ends.
const REFINE: usize = 24;
/// Side edges tried on each side of the cursor.
const MAX_CANDIDATES: usize = 6;
/// Max color difference between the backgrounds left and right of an image.
const SAME_BG: i32 = 48;

struct Img<'a> {
    px: &'a [u8],
    w: usize,
    h: usize,
}

type Rgb = [i32; 3];

fn diff(a: Rgb, b: Rgb) -> i32 {
    (a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs()
}

impl Img<'_> {
    fn at(&self, x: usize, y: usize) -> Rgb {
        let i = (y * self.w + x) * 4;
        [self.px[i] as i32, self.px[i + 1] as i32, self.px[i + 2] as i32]
    }

    /// Boundary with `outside` column on one side and `inside` on the other, at row y.
    /// Comparing two pixels apart skips over 1px borders and antialiasing.
    fn edge_v(&self, outside: usize, inside: usize, y: usize) -> bool {
        diff(self.at(outside, y), self.at(inside, y)) >= EDGE
    }

    fn edge_h(&self, x: usize, outside: usize, inside: usize) -> bool {
        diff(self.at(x, outside), self.at(x, inside)) >= EDGE
    }

    fn mean<I: Iterator<Item = Rgb>>(it: I) -> (Rgb, Vec<Rgb>) {
        let px: Vec<Rgb> = it.collect();
        let n = px.len().max(1) as i32;
        let mut s = [0; 3];
        for p in &px {
            for c in 0..3 {
                s[c] += p[c];
            }
        }
        ([s[0] / n, s[1] / n, s[2] / n], px)
    }

    /// Mean color of the segment and whether >= 90% of it is close to that mean.
    fn flat<I: Iterator<Item = Rgb>>(it: I) -> (Rgb, bool) {
        let (mean, px) = Self::mean(it);
        let close = px.iter().filter(|p| diff(**p, mean) <= FLAT).count();
        (mean, close * 10 >= px.len() * 9)
    }

    fn col(&self, x: usize, y0: usize, y1: usize) -> impl Iterator<Item = Rgb> + '_ {
        (y0..=y1).map(move |y| self.at(x, y))
    }

    fn row(&self, y: usize, x0: usize, x1: usize) -> impl Iterator<Item = Rgb> + '_ {
        (x0..x1).map(move |x| self.at(x, y))
    }

    /// A vertical image boundary between `outside` and `inside` columns that
    /// passes near row `cy`: returns its extent (inclusive) if it is long enough,
    /// its outer side is flat, and the inner side clearly differs from it.
    fn side(&self, outside: usize, inside: usize, cy: usize) -> Option<(usize, usize)> {
        let edge = |y: usize| self.edge_v(outside, inside, y);
        let anchor = (cy.saturating_sub(GAP)..=(cy + GAP).min(self.h - 1)).find(|&y| edge(y))?;
        let (mut y0, mut y1) = (anchor, anchor);
        let mut gap = 0;
        for y in (0..anchor).rev() {
            if edge(y) {
                y0 = y;
                gap = 0;
            } else {
                gap += 1;
                if gap > GAP {
                    break;
                }
            }
        }
        gap = 0;
        for y in anchor + 1..self.h {
            if edge(y) {
                y1 = y;
                gap = 0;
            } else {
                gap += 1;
                if gap > GAP {
                    break;
                }
            }
        }
        if y1 - y0 + 1 < MIN_SIDE {
            return None;
        }
        let outer: Vec<Rgb> = self.col(outside, y0, y1).collect();
        if !Self::flat(outer.iter().copied()).1 && !Self::smooth(&outer) {
            return None;
        }
        let differs = (y0..=y1).filter(|&y| diff(self.at(inside, y), outer[y - y0]) >= EDGE).count();
        (differs * 10 >= (y1 - y0 + 1) * 6).then_some((y0, y1))
    }

    /// Whether colors along a line change only gradually, as on a blurred
    /// backdrop (a photo viewer's blurred copy of the photo).
    fn smooth(px: &[Rgb]) -> bool {
        let steps = px.windows(2).filter(|p| diff(p[0], p[1]) <= SMOOTH).count();
        steps * 10 >= px.len().saturating_sub(1) * 9
    }

    /// Most common color of the whole buffer (sampled, quantized to 4 bits per channel).
    fn dominant_color(&self) -> Rgb {
        let mut bins = vec![0u32; 4096];
        for y in (0..self.h).step_by(4) {
            for x in (0..self.w).step_by(4) {
                let p = self.at(x, y);
                bins[((p[0] >> 4) << 8 | (p[1] >> 4) << 4 | (p[2] >> 4)) as usize] += 1;
            }
        }
        let best = (0..4096).max_by_key(|&i| bins[i]).unwrap_or(0) as i32;
        [((best >> 8) << 4) + 8, (((best >> 4) & 15) << 4) + 8, ((best & 15) << 4) + 8]
    }

    /// Fraction (in 0..=10) of the row segment that has a horizontal edge,
    /// plus whether the outer row is flat.
    fn top_bottom_score(&self, outside: usize, inside: usize, x0: usize, x1: usize) -> (usize, bool) {
        let n = x1 - x0;
        let edges = (x0..x1).filter(|&x| self.edge_h(x, outside, inside)).count();
        (edges * 10 / n.max(1), Self::flat(self.row(outside, x0, x1)).1)
    }
}

/// Returns `(left, top, right, bottom)` (right/bottom exclusive) of the image
/// under `(cx, cy)` in a top-down BGRA buffer, or `None` if there is no
/// convincing image rectangle there.
pub fn find_image_rect(px: &[u8], w: usize, h: usize, cx: usize, cy: usize) -> Option<(usize, usize, usize, usize)> {
    if w < MIN_SIDE + 4 || h < MIN_SIDE + 4 || cx < 2 || cy < 2 || cx + 2 >= w || cy + 2 >= h {
        return None;
    }
    let img = Img { px, w, h };

    // Candidate sides, innermost first. The innermost pair can be a panel
    // inside the image (a dialog in a screenshot), so try a few outward.
    // Left: boundary between column x-2 (outside) and x (inside).
    let lefts = candidates((2..=cx).rev(), |x| img.side(x - 2, x, cy).map(|r| (x - 1, r)));
    // Right: boundary between column x (inside) and x+2 (outside).
    let rights = candidates(cx..w - 2, |x| img.side(x + 2, x, cy).map(|r| (x + 2, r)));
    debug(&format!("lefts {lefts:?}, rights {rights:?}"));
    // An image's two sides are about as tall as each other; a short edge
    // paired with a long one is a small box inside the image.
    let len = |s: &Side| s.1.1 - s.1.0 + 1;
    let mut pairs: Vec<_> = lefts
        .iter()
        .flat_map(|l| rights.iter().map(move |r| (*l, *r)))
        .filter(|(l, r)| len(l).min(len(r)) * 3 >= len(l).max(len(r)))
        .collect();
    pairs.sort_by_key(|((l, _), (r, _))| r - l);
    let valid: Vec<(Rect, Rgb)> = pairs.into_iter().filter_map(|(l, r)| evaluate(&img, cx, cy, l, r)).collect();

    // Prefer the innermost rectangle sitting on the window's main background
    // (chat background, viewer backdrop): rectangles inside a screenshot sit
    // on the screenshot's own colors instead.
    let main_bg = img.dominant_color();
    debug(&format!("main background {main_bg:?}"));
    valid
        .iter()
        .find(|(_, bg)| diff(*bg, main_bg) <= SAME_BG)
        .or(valid.first())
        .map(|(r, _)| *r)
        .or_else(|| band(&img, cx, cy, main_bg))
}

/// A picture as wide as the area, which has no side edges: a viewer fits a
/// wide screenshot to its width (Teams). Its top and bottom are the nearest
/// rows above and below the cursor whose edge with the row beyond runs
/// unbroken through the cursor's column across most of the width (a window
/// over part of the viewer cuts it short), with the backdrop (the area's
/// main color) beyond.
fn band(img: &Img, cx: usize, cy: usize, backdrop: Rgb) -> Option<Rect> {
    let (w, h) = (img.w, img.h);
    // The edge between rows `outside` and `inside`, through column cx,
    // allowing short breaks (text touching the edge, antialiasing).
    let run = |outside: usize, inside: usize| -> Option<(usize, usize)> {
        let edge = |x: usize| img.edge_h(x, outside, inside);
        let reach = |xs: &mut dyn Iterator<Item = usize>| {
            let (mut last, mut gap) = (cx, 0);
            for x in xs {
                if edge(x) {
                    (last, gap) = (x, 0);
                } else {
                    gap += 1;
                    if gap > GAP * 2 {
                        break;
                    }
                }
            }
            last
        };
        let (l, r) = (reach(&mut (0..cx).rev()), reach(&mut (cx..w)));
        let long = (r - l) * 10 >= w * 6;
        (long && diff(Img::mean(img.row(outside, l, r + 1)).0, backdrop) <= SAME_BG).then_some((l, r + 1))
    };
    let (top, (tl, tr)) = (2..=cy).rev().find_map(|y| run(y - 2, y).map(|s| (y - 1, s)))?;
    let (bottom, (bl, br)) = (cy..h - 2).find_map(|y| run(y + 2, y).map(|s| (y + 2, s)))?;
    let (left, right) = (tl.max(bl), tr.min(br));
    let ih = bottom - top;
    debug(&format!("band {left},{top} - {right},{bottom}"));
    (ih >= MIN_SIDE && ih * 100 <= h * 95 && left <= cx && cx < right).then_some((left, top, right, bottom))
}

type Rect = (usize, usize, usize, usize);

type Side = (usize, (usize, usize));

/// The first few distinct sides found while scanning `xs` outward.
fn candidates(xs: impl Iterator<Item = usize>, side: impl Fn(usize) -> Option<Side>) -> Vec<Side> {
    let mut out = Vec::new();
    let mut last_hit: Option<usize> = None;
    for x in xs {
        // Neighbouring columns usually belong to the same edge.
        if last_hit.is_some_and(|l| l.abs_diff(x) <= 3) {
            continue;
        }
        if let Some(s) = side(x) {
            out.push(s);
            last_hit = Some(x);
            if out.len() == MAX_CANDIDATES {
                break;
            }
        }
    }
    out
}

/// Works out top and bottom for one left/right pair and checks that the
/// result looks like an image sitting on a background.
/// Returns the rectangle and its background color.
fn evaluate(img: &Img, cx: usize, cy: usize, (left, ly): Side, (right, ry): Side) -> Option<(Rect, Rgb)> {
    let (w, h) = (img.w, img.h);
    if right - left < MIN_SIDE {
        return None;
    }

    // Height from the side edges, then refine with horizontal edges (rounded
    // corners and weak contrast make the side edges a bit short).
    let t0 = ly.0.min(ry.0);
    let b0 = ly.1.max(ry.1);
    let margin = 12.min((right - left) / 4);
    let (x0, x1) = (left + margin, right - margin);

    let top = (t0.saturating_sub(REFINE).max(2)..=(t0 + REFINE).min(cy))
        .find(|&y| {
            let (score, flat) = img.top_bottom_score(y - 2, y, x0, x1);
            score >= 6 && flat
        })
        .map(|y| y - 1)
        .unwrap_or(t0);
    let bottom = ((b0.saturating_sub(REFINE)).max(cy)..=(b0 + REFINE).min(h - 3))
        .rev()
        .find(|&y| {
            let (score, flat) = img.top_bottom_score(y + 2, y, x0, x1);
            score >= 6 && flat
        })
        .map(|y| y + 2)
        .unwrap_or(b0 + 1);

    debug(&format!("pair {left}..{right}: top {top}, bottom {bottom}"));
    if !(left <= cx && cx < right && top <= cy && cy < bottom) {
        return None;
    }
    let (iw, ih) = (right - left, bottom - top);
    if ih < MIN_SIDE || iw * 100 > w * 95 || ih * 100 > h * 95 {
        debug("rejected: size");
        return None;
    }

    // Same background on both sides, or this is a panel / highlighted row.
    let (bg_l, _) = Img::mean(img.col(left.saturating_sub(4), top, bottom - 1));
    let (bg_r, _) = Img::mean(img.col((right + 3).min(w - 1), top, bottom - 1));
    if diff(bg_l, bg_r) > SAME_BG {
        debug(&format!("rejected: backgrounds differ {bg_l:?} vs {bg_r:?}"));
        return None;
    }
    debug(&format!("valid {left},{top} - {right},{bottom} on {bg_l:?}"));
    Some(((left, top, right, bottom), bg_l))
}

fn debug(msg: &str) {
    if cfg!(debug_assertions) && std::env::var_os("QC_DEBUG_RECT").is_some() {
        eprintln!("[rectfind] {msg}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BG: [u8; 3] = [241, 233, 226]; // BGR of a light chat background

    struct Canvas {
        w: usize,
        h: usize,
        px: Vec<u8>,
    }

    impl Canvas {
        fn new(w: usize, h: usize, bgr: [u8; 3]) -> Self {
            let mut px = vec![255u8; w * h * 4];
            for p in px.chunks_mut(4) {
                p[..3].copy_from_slice(&bgr);
            }
            Self { w, h, px }
        }
        fn set(&mut self, x: usize, y: usize, bgr: [u8; 3]) {
            let i = (y * self.w + x) * 4;
            self.px[i..i + 3].copy_from_slice(&bgr);
        }
        fn rect(&mut self, l: usize, t: usize, r: usize, b: usize, f: impl Fn(usize, usize) -> [u8; 3]) {
            for y in t..b {
                for x in l..r {
                    self.set(x, y, f(x, y));
                }
            }
        }
        /// A photo-like rectangle with rounded corners.
        fn photo(&mut self, l: usize, t: usize, r: usize, b: usize, radius: usize) {
            for y in t..b {
                for x in l..r {
                    let dx = if x < l + radius { l + radius - x } else if x >= r - radius { x + 1 + radius - r } else { 0 };
                    let dy = if y < t + radius { t + radius - y } else if y >= b - radius { y + 1 + radius - b } else { 0 };
                    if dx * dx + dy * dy > radius * radius {
                        continue;
                    }
                    let v = ((x * 7 + y * 13) % 97) as u8;
                    self.set(x, y, [60 + v, 90 + v / 2, 30 + v]);
                }
            }
        }
        fn find(&self, cx: usize, cy: usize) -> Option<(usize, usize, usize, usize)> {
            find_image_rect(&self.px, self.w, self.h, cx, cy)
        }
    }

    fn near(got: (usize, usize, usize, usize), want: (usize, usize, usize, usize)) -> bool {
        let d = |a: usize, b: usize| a.abs_diff(b) <= 2;
        d(got.0, want.0) && d(got.1, want.1) && d(got.2, want.2) && d(got.3, want.3)
    }

    #[test]
    fn finds_photo_on_flat_background() {
        let mut c = Canvas::new(800, 600, BG);
        c.photo(100, 80, 420, 300, 0);
        let r = c.find(250, 200).expect("photo not found");
        assert!(near(r, (100, 80, 420, 300)), "{r:?}");
    }

    #[test]
    fn finds_photo_with_rounded_corners() {
        let mut c = Canvas::new(800, 600, BG);
        c.photo(150, 120, 500, 400, 12);
        let r = c.find(300, 250).expect("photo not found");
        assert!(near(r, (150, 120, 500, 400)), "{r:?}");
    }

    #[test]
    fn finds_text_screenshot_with_inner_divider() {
        let mut c = Canvas::new(900, 700, BG);
        c.rect(200, 100, 700, 500, |_, _| [255, 255, 255]);
        // A full-width divider inside the screenshot must not become its top.
        c.rect(200, 180, 700, 182, |_, _| [200, 200, 200]);
        // Text-like lines.
        for line in 0..8 {
            let y = 210 + line * 34;
            c.rect(230, y, 650, y + 14, |x, _| if (x / 6) % 3 == 0 { [255, 255, 255] } else { [20, 20, 20] });
        }
        let r = c.find(400, 300).expect("screenshot not found");
        assert!(near(r, (200, 100, 700, 500)), "{r:?}");
    }

    #[test]
    fn finds_viewer_image_around_inner_dialog() {
        // Dark viewer background, a screenshot of a dimmed page with a white
        // dialog in it. The dialog's edges come first but are not the image.
        let mut c = Canvas::new(1600, 1000, [20, 20, 20]);
        c.rect(250, 180, 1350, 900, |x, y| if (x / 40 + y / 25) % 5 == 0 { [110, 110, 110] } else { [136, 136, 137] });
        c.rect(380, 185, 1150, 900, |_, _| [255, 255, 255]);
        for line in 0..10 {
            let y = 240 + line * 50;
            c.rect(420, y, 900, y + 14, |x, _| if (x / 6) % 3 == 0 { [255, 255, 255] } else { [40, 40, 40] });
        }
        let r = c.find(600, 500).expect("viewer image not found");
        assert!(near(r, (250, 180, 1350, 900)), "{r:?}");
    }

    #[test]
    fn finds_screenshot_as_wide_as_the_viewer() {
        // Teams' photo viewer: a wide screenshot fitted to the viewer's width
        // on a dimmed chat (dark, with faint text). No side edges; the
        // screenshot has full-width lines of its own (a header border).
        let mut c = Canvas::new(1500, 800, [36, 36, 36]);
        for line in 0..20 {
            let y = 20 + line * 38;
            c.rect(100, y, 600, y + 10, |x, _| if (x / 5) % 3 == 0 { [52, 52, 52] } else { [36, 36, 36] });
        }
        c.rect(0, 260, 1500, 460, |_, _| [250, 250, 250]);
        c.rect(0, 318, 1500, 320, |_, _| [200, 200, 200]);
        c.rect(0, 400, 1500, 460, |_, _| [235, 235, 235]);
        for x in (60..1400).step_by(180) {
            c.rect(x, 280, x + 120, 300, |_, _| [40, 40, 40]);
        }
        let r = c.find(300, 290).expect("wide screenshot not found");
        assert!(near(r, (0, 260, 1500, 460)), "{r:?}");
        // A window over the viewer's right part cuts the screenshot short.
        c.rect(1150, 0, 1500, 800, |_, _| [30, 30, 30]);
        let r = c.find(300, 290).expect("partly covered screenshot not found");
        assert!(near(r, (0, 260, 1150, 460)), "{r:?}");
    }

    #[test]
    fn finds_viewer_image_on_blurred_backdrop() {
        // Facebook's viewer: a dark backdrop that is a blurred copy of the
        // photo, so it changes color slowly down the sides; a white card
        // inside the photo comes first.
        let mut c = Canvas::new(1600, 1000, [74, 74, 74]);
        c.rect(0, 0, 1600, 1000, |_, y| {
            let t = (y as i32 - 300).unsigned_abs() as usize;
            let v = if t < 150 { 150 - t } else { 0 };
            [(74 + v / 2) as u8, (74 + v / 3) as u8, (74 - v / 3) as u8]
        });
        c.rect(600, 40, 1000, 960, |_, y| if y < 200 { [200, 120, 40] } else { [240, 230, 220] });
        c.rect(630, 400, 970, 600, |_, _| [255, 255, 255]);
        for line in 0..4 {
            let y = 430 + line * 40;
            c.rect(650, y, 940, y + 12, |x, _| if (x / 6) % 3 == 0 { [255, 255, 255] } else { [30, 30, 30] });
        }
        let r = c.find(800, 500).expect("viewer image not found");
        assert!(near(r, (600, 40, 1000, 960)), "{r:?}");
    }

    #[test]
    fn prefers_image_over_panel_inside_it() {
        // A white screenshot in a chat, containing a gray panel that also has
        // the same color on both sides. The chat background decides.
        let mut c = Canvas::new(900, 700, BG);
        c.rect(200, 100, 700, 560, |_, _| [255, 255, 255]);
        c.rect(300, 200, 600, 420, |_, _| [200, 200, 200]);
        for line in 0..4 {
            let y = 230 + line * 40;
            c.rect(320, y, 560, y + 12, |x, _| if (x / 6) % 3 == 0 { [200, 200, 200] } else { [30, 30, 30] });
        }
        let r = c.find(450, 300).expect("screenshot not found");
        assert!(near(r, (200, 100, 700, 560)), "{r:?}");
    }

    #[test]
    fn rejects_highlighted_list_row() {
        // Blue nav bar | white sidebar with a highlighted row | chat background.
        let mut c = Canvas::new(1000, 700, BG);
        c.rect(0, 0, 70, 700, |_, _| [220, 120, 0]);
        c.rect(70, 0, 400, 700, |_, _| [255, 255, 255]);
        c.rect(70, 200, 400, 280, |_, _| [255, 239, 229]);
        c.photo(90, 210, 150, 270, 0); // avatar
        assert_eq!(c.find(300, 240), None);
    }

    #[test]
    fn nothing_on_plain_background() {
        let c = Canvas::new(800, 600, BG);
        assert_eq!(c.find(400, 300), None);
    }

    #[test]
    fn rejects_tiny_sticker() {
        let mut c = Canvas::new(800, 600, BG);
        c.photo(300, 300, 330, 330, 0);
        assert_eq!(c.find(315, 315), None);
    }
}
