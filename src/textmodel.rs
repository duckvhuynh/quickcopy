//! Character-level view of the OCR result, so selection behaves like native
//! text: a caret between characters, Shift+arrows, word and line moves.
//!
//! OCR gives word boxes; each word's box is split evenly between its
//! characters, spaces fill the gaps between words, and lines end in '\n'.
//! Offsets are caret positions: 0 ..= chars.len().

use windows::Win32::Foundation::{POINT, RECT};

use crate::worker::Word;

#[derive(Debug, Clone)]
pub struct Ch {
    pub c: char,
    /// Horizontal extent (overlay coordinates); vertical extent is the line's.
    pub x0: i32,
    pub x1: i32,
}

pub struct TextModel {
    pub chars: Vec<Ch>,
    /// Per line: [start, end) offsets, `end` excluding the line's '\n'.
    pub lines: Vec<(usize, usize)>,
    /// Per line: its box (union of its words).
    pub line_rects: Vec<RECT>,
}

impl TextModel {
    pub fn new(words: &[Word], line_rects: &[RECT]) -> Self {
        let mut chars = Vec::new();
        let mut lines = Vec::new();
        let mut kept_rects = Vec::new();
        let mut i = 0;
        while i < words.len() {
            let line = words[i].line;
            let mut j = i;
            while j < words.len() && words[j].line == line {
                j += 1;
            }
            if !lines.is_empty() {
                let prev_right = chars.last().map_or(0, |c: &Ch| c.x1);
                chars.push(Ch { c: '\n', x0: prev_right, x1: prev_right });
            }
            let start = chars.len();
            for (k, word) in words[i..j].iter().enumerate() {
                if k > 0 {
                    let prev_right = chars.last().map_or(word.rect.left, |c: &Ch| c.x1);
                    chars.push(Ch { c: ' ', x0: prev_right, x1: word.rect.left.max(prev_right) });
                }
                let letters: Vec<char> = word.text.chars().collect();
                let n = letters.len().max(1) as i32;
                let (left, width) = (word.rect.left, word.rect.right - word.rect.left);
                for (ci, c) in letters.into_iter().enumerate() {
                    let ci = ci as i32;
                    chars.push(Ch { c, x0: left + width * ci / n, x1: left + width * (ci + 1) / n });
                }
            }
            lines.push((start, chars.len()));
            kept_rects.push(line_rects.get(line).copied().unwrap_or_default());
            i = j;
        }
        Self { chars, lines, line_rects: kept_rects }
    }

    pub fn len(&self) -> usize {
        self.chars.len()
    }

    /// The line a caret offset belongs to (a caret right after a line's last
    /// character stays on that line).
    pub fn line_of(&self, offset: usize) -> usize {
        self.lines.iter().position(|&(_, e)| offset <= e).unwrap_or(self.lines.len().saturating_sub(1))
    }

    /// Caret offset on `line` closest to `x`.
    pub fn offset_at(&self, line: usize, x: i32) -> usize {
        let Some(&(s, e)) = self.lines.get(line) else { return 0 };
        (s..e).find(|&i| x < (self.chars[i].x0 + self.chars[i].x1) / 2).unwrap_or(e)
    }

    /// Caret offset for a point (nearest line, then nearest gap in it).
    pub fn hit(&self, pt: POINT) -> usize {
        let line = (0..self.lines.len())
            .min_by_key(|&l| {
                let r = self.line_rects[l];
                if pt.y < r.top {
                    r.top - pt.y
                } else if pt.y > r.bottom {
                    pt.y - r.bottom
                } else {
                    0
                }
            })
            .unwrap_or(0);
        self.offset_at(line, pt.x)
    }

    /// Line and x of a caret offset (used to move between lines and to find a
    /// selection again after the words change).
    pub fn pos_of(&self, offset: usize) -> (usize, i32) {
        let line = self.line_of(offset);
        let (s, e) = self.lines.get(line).copied().unwrap_or((0, 0));
        let x = if offset < e {
            self.chars[offset].x0
        } else if e > s {
            self.chars[e - 1].x1
        } else {
            self.line_rects.get(line).map_or(0, |r| r.left)
        };
        (line, x)
    }

    fn is_word(&self, i: usize) -> bool {
        self.chars.get(i).is_some_and(|c| !c.c.is_whitespace())
    }

    /// [start, end) of the word (or run of spaces) at a caret offset.
    pub fn word_at(&self, offset: usize) -> (usize, usize) {
        let line = self.line_of(offset);
        let (s, e) = self.lines[line];
        if s == e {
            return (s, s);
        }
        let i = offset.min(e - 1).max(s);
        let kind = self.is_word(i);
        let mut a = i;
        while a > s && self.is_word(a - 1) == kind {
            a -= 1;
        }
        let mut b = i + 1;
        while b < e && self.is_word(b) == kind {
            b += 1;
        }
        (a, b)
    }

    /// [start, end) of the line at a caret offset, without its '\n'.
    pub fn line_at(&self, offset: usize) -> (usize, usize) {
        self.lines[self.line_of(offset)]
    }

    /// Ctrl+Right: to the start of the next word.
    pub fn next_word(&self, mut i: usize) -> usize {
        let n = self.len();
        if i < n && self.chars[i].c == '\n' {
            return i + 1;
        }
        while i < n && self.is_word(i) {
            i += 1;
        }
        while i < n && self.chars[i].c == ' ' {
            i += 1;
        }
        i
    }

    /// Ctrl+Left: to the start of this or the previous word.
    pub fn prev_word(&self, mut i: usize) -> usize {
        if i > 0 && self.chars[i - 1].c == '\n' {
            return i - 1;
        }
        while i > 0 && self.chars[i - 1].c == ' ' {
            i -= 1;
        }
        while i > 0 && self.is_word(i - 1) {
            i -= 1;
        }
        i
    }

    /// Up/Down: the same x on the neighbouring line.
    pub fn vertical(&self, offset: usize, down: bool) -> usize {
        let (line, x) = self.pos_of(offset);
        let target = if down { (line + 1).min(self.lines.len() - 1) } else { line.saturating_sub(1) };
        if target == line {
            return if down { self.lines[line].1 } else { self.lines[line].0 };
        }
        self.offset_at(target, x)
    }

    pub fn text(&self, a: usize, b: usize) -> String {
        self.chars[a.min(b)..a.max(b).min(self.len())].iter().map(|c| c.c).collect()
    }

    /// Highlight boxes for the selection [a, b): one per line it touches.
    pub fn highlight(&self, a: usize, b: usize) -> Vec<RECT> {
        let (a, b) = (a.min(b), a.max(b));
        let mut out = Vec::new();
        for (l, &(s, e)) in self.lines.iter().enumerate() {
            let (from, to) = (a.max(s), b.min(e));
            if from >= to {
                continue;
            }
            let r = self.line_rects[l];
            out.push(RECT { left: self.chars[from].x0, top: r.top, right: self.chars[to - 1].x1, bottom: r.bottom });
        }
        out
    }
}

/// Where the overlay takes the mouse: each paragraph's box grown by a wide
/// margin (3 line heights sideways, 1.5 above and below). People start a drag
/// above and left of a paragraph, end it below and right of it, or press
/// between lines; those presses must start a selection, not drag the image.
/// Lines close together (gap up to 1.5 line heights, overlapping sideways)
/// form one paragraph; a far-off caption keeps its own smaller area, so the
/// rest of the image still gets the app's own clicks and drags.
pub fn grab_areas(lines: &[RECT]) -> Vec<RECT> {
    paragraphs(lines)
        .into_iter()
        .map(|p| {
            let (b, h) = (p.rect, p.line_height);
            let (dx, dy) = ((3 * h).max(32), (3 * h / 2).max(16));
            RECT { left: b.left - dx, top: b.top - dy, right: b.right + dx, bottom: b.bottom + dy }
        })
        .collect()
}

pub struct Paragraph {
    /// Indices into the `lines` given to `paragraphs`, top to bottom.
    pub lines: Vec<usize>,
    pub rect: RECT,
    /// Tallest line.
    pub line_height: i32,
}

/// Groups lines into paragraphs: lines close together (gap up to 1.5 line
/// heights) that overlap sideways (within 2 line heights).
pub fn paragraphs(lines: &[RECT]) -> Vec<Paragraph> {
    let mut order: Vec<usize> = (0..lines.len()).filter(|&i| lines[i].right > lines[i].left && lines[i].bottom > lines[i].top).collect();
    order.sort_by_key(|&i| lines[i].top);
    let mut out: Vec<Paragraph> = Vec::new();
    for i in order {
        let r = lines[i];
        let h = r.bottom - r.top;
        let near = out.iter_mut().find(|p| {
            let (b, lh) = (p.rect, h.max(p.line_height));
            r.top - b.bottom <= lh * 3 / 2 && r.bottom >= b.top && r.left <= b.right + 2 * lh && r.right >= b.left - 2 * lh
        });
        match near {
            Some(p) => {
                p.rect = RECT {
                    left: p.rect.left.min(r.left),
                    top: p.rect.top.min(r.top),
                    right: p.rect.right.max(r.right),
                    bottom: p.rect.bottom.max(r.bottom),
                };
                p.line_height = p.line_height.max(h);
                p.lines.push(i);
            }
            None => out.push(Paragraph { lines: vec![i], rect: r, line_height: h }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(l: i32, t: i32, r: i32, b: i32) -> RECT {
        RECT { left: l, top: t, right: r, bottom: b }
    }

    #[test]
    fn paragraph_lines_share_one_wide_area() {
        // Three 20 px lines, 16 px apart: one paragraph.
        let lines = [rect(100, 100, 500, 120), rect(100, 136, 480, 156), rect(100, 172, 300, 192)];
        let areas = grab_areas(&lines);
        assert_eq!(areas, vec![rect(40, 70, 560, 222)]);
    }

    #[test]
    fn distant_caption_keeps_its_own_area() {
        let lines = [rect(100, 100, 500, 120), rect(100, 136, 480, 156), rect(600, 700, 700, 716)];
        let areas = grab_areas(&lines);
        assert_eq!(areas.len(), 2);
        // 16 px line: 48 px sideways, 24 px above and below.
        assert_eq!(areas[1], rect(552, 676, 748, 740));
    }

    fn word(text: &str, line: usize, left: i32, right: i32) -> Word {
        let top = line as i32 * 40;
        Word { rect: RECT { left, top, right, bottom: top + 20 }, text: text.into(), line }
    }

    /// "Hôm nay\nđẹp quá" with 10 px per character.
    fn sample() -> TextModel {
        let words = vec![word("Hôm", 0, 0, 30), word("nay", 0, 40, 70), word("đẹp", 1, 0, 30), word("quá", 1, 40, 70)];
        let rects = vec![RECT { left: 0, top: 0, right: 70, bottom: 20 }, RECT { left: 0, top: 40, right: 70, bottom: 60 }];
        TextModel::new(&words, &rects)
    }

    #[test]
    fn builds_text_with_spaces_and_newlines() {
        let m = sample();
        assert_eq!(m.text(0, m.len()), "Hôm nay\nđẹp quá");
        assert_eq!(m.lines, vec![(0, 7), (8, 15)]);
    }

    #[test]
    fn hit_finds_caret_between_characters() {
        let m = sample();
        assert_eq!(m.hit(POINT { x: 12, y: 10 }), 1); // past the middle of 'H'
        assert_eq!(m.hit(POINT { x: -50, y: 10 }), 0); // before the line
        assert_eq!(m.hit(POINT { x: 500, y: 10 }), 7); // after the line, before '\n'
        assert_eq!(m.hit(POINT { x: 16, y: 52 }), 10); // second line
    }

    #[test]
    fn words_and_lines() {
        let m = sample();
        assert_eq!(m.word_at(5), (4, 7));
        assert_eq!(m.text(4, 7), "nay");
        assert_eq!(m.line_at(9), (8, 15));
        assert_eq!(m.next_word(0), 4);
        assert_eq!(m.next_word(4), 7);
        assert_eq!(m.next_word(7), 8); // over the line break
        assert_eq!(m.prev_word(7), 4);
        assert_eq!(m.prev_word(8), 7);
    }

    #[test]
    fn vertical_keeps_x() {
        let m = sample();
        assert_eq!(m.vertical(5, true), 13);
        assert_eq!(m.vertical(13, false), 5);
        assert_eq!(m.vertical(13, true), 15); // last line: to its end
    }

    #[test]
    fn highlight_spans_lines() {
        let m = sample();
        let r = m.highlight(5, 10);
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].left, r[0].right), (50, 70));
        assert_eq!((r[1].left, r[1].right), (0, 20));
        assert_eq!(m.text(5, 10), "ay\nđẹ");
    }
}
