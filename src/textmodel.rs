//! Character-level view of the OCR result, so selection behaves like native
//! text: a caret between characters, Shift+arrows, word and line moves.
//!
//! OCR gives word boxes; each word's box is split evenly between its
//! characters, spaces fill the gaps between words, and lines end in '\n'.
//! Offsets are caret positions: 0 ..= chars.len().
//!
//! Screens are not one column of text: a form has fields side by side, a
//! table has cells, a page has columns. Lines are therefore grouped into
//! blocks of lines stacked under one another (`stack`). A click goes to the
//! block it is in, a selection inside a block is everything between its
//! ends, and a selection across blocks goes by position, row after row,
//! within the width of the two blocks (`pieces`): dragging along a table row
//! selects that row, and dragging down one pane of a window leaves the
//! panes beside it alone.

use crate::common::{POINT, RECT};

#[derive(Clone)]
pub struct Word {
    /// Relative to the top-left of `OcrData::rect`.
    pub rect: RECT,
    pub text: String,
    pub line: usize,
}

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
    /// Lines are in reading order: block after block, top to bottom in each.
    pub lines: Vec<(usize, usize)>,
    /// Per line: its box (union of its words).
    pub line_rects: Vec<RECT>,
    /// Per line: the block it is stacked in.
    block_of: Vec<usize>,
    /// Per block: the box around its lines.
    block_rects: Vec<RECT>,
}

/// Groups lines into blocks of lines stacked under one another: a paragraph,
/// a column, a table cell, a label. A line joins the block whose last line
/// is right above it (overlapping sideways, at most 1.5 line heights up),
/// so lines side by side are never in one block. Returns each block's lines
/// top to bottom, the blocks in the order `rects` first mentions them (the
/// OCR's reading order).
fn stack(rects: &[RECT]) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..rects.len()).collect();
    order.sort_by_key(|&i| (rects[i].top, rects[i].left));
    let mut blocks: Vec<Vec<usize>> = Vec::new();
    for i in order {
        let r = rects[i];
        let above = blocks
            .iter_mut()
            .filter_map(|block| {
                let last = rects[*block.last()?];
                let line_height = (r.bottom - r.top).max(last.bottom - last.top);
                let gap = r.top - last.bottom;
                let overlap = r.right.min(last.right) - r.left.max(last.left);
                (r.top >= (last.top + last.bottom) / 2 && gap <= line_height * 3 / 2 && overlap > 0).then_some((gap, block))
            })
            .min_by_key(|(gap, _)| *gap);
        match above {
            Some((_, block)) => block.push(i),
            None => blocks.push(vec![i]),
        }
    }
    blocks.sort_by_key(|block| block.iter().min().copied());
    blocks
}

impl TextModel {
    pub fn new(words: &[Word], line_rects: &[RECT]) -> Self {
        // The words of each OCR line (they arrive grouped by line).
        let mut spans: Vec<std::ops::Range<usize>> = Vec::new();
        let mut i = 0;
        while i < words.len() {
            let mut j = i;
            while j < words.len() && words[j].line == words[i].line {
                j += 1;
            }
            spans.push(i..j);
            i = j;
        }
        let rects: Vec<RECT> = spans.iter().map(|span| line_rects.get(words[span.start].line).copied().unwrap_or_default()).collect();

        let mut model = Self { chars: Vec::new(), lines: Vec::new(), line_rects: Vec::new(), block_of: Vec::new(), block_rects: Vec::new() };
        let chars = &mut model.chars;
        for (block, members) in stack(&rects).into_iter().enumerate() {
            model.block_rects.push(members.iter().map(|&l| rects[l]).fold(rects[members[0]], |a, r| RECT {
                left: a.left.min(r.left),
                top: a.top.min(r.top),
                right: a.right.max(r.right),
                bottom: a.bottom.max(r.bottom),
            }));
            for line in members {
                if !model.lines.is_empty() {
                    let prev_right = chars.last().map_or(0, |c: &Ch| c.x1);
                    chars.push(Ch { c: '\n', x0: prev_right, x1: prev_right });
                }
                let start = chars.len();
                for (k, word) in words[spans[line].clone()].iter().enumerate() {
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
                model.lines.push((start, chars.len()));
                model.line_rects.push(rects[line]);
                model.block_of.push(block);
            }
        }
        model
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

    /// Caret offset for a point: first the block it belongs to, then the
    /// line of that block level with it, then the nearest gap in that line.
    ///
    /// The block is the one whose bounds hold the point (beside a short last
    /// line, or between two rows of a column, the point is still in that
    /// paragraph or column). Outside every block it is the block of the
    /// line level with the point (the margin beside a centered heading),
    /// and failing that of the nearest line.
    pub fn hit(&self, pt: POINT) -> usize {
        let away = |r: RECT| {
            let dx = (r.left - pt.x).max(pt.x - r.right).max(0) as i64;
            let dy = (r.top - pt.y).max(pt.y - r.bottom).max(0) as i64;
            (dx, dy)
        };
        let lines = 0..self.lines.len();
        let line_away = |l: usize| away(self.line_rects[l]);
        let nearest = |candidates: &mut dyn Iterator<Item = usize>| {
            candidates.min_by_key(|&l| {
                let (dx, dy) = line_away(l);
                dx * dx + dy * dy
            })
        };
        let inside = |l: &usize| away(self.block_rects[self.block_of[*l]]) == (0, 0);
        let level = |l: &usize| line_away(*l).1 == 0;
        let Some(found) = nearest(&mut lines.clone().filter(inside)).or_else(|| nearest(&mut lines.clone().filter(level))).or_else(|| nearest(&mut lines.clone())) else {
            return 0;
        };
        let line = lines.filter(|&l| self.block_of[l] == self.block_of[found]).min_by_key(|&l| {
            let (dx, dy) = line_away(l);
            (dy, dx)
        });
        self.offset_at(line.unwrap_or(found), pt.x)
    }

    /// The block each line is in, for printing the layout (`--drag`).
    pub fn block_of(&self, line: usize) -> usize {
        self.block_of[line]
    }

    /// Where a caret offset is drawn: its x, halfway down its line.
    pub fn point_of(&self, offset: usize) -> POINT {
        let (line, x) = self.pos_of(offset);
        let r = self.line_rects.get(line).copied().unwrap_or_default();
        POINT { x, y: (r.top + r.bottom) / 2 }
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

    /// What is selected between the carets `a` and `b`: `(line, from, to)`
    /// for each line with selected characters, in the order to read them.
    ///
    /// With both carets in one block, that is every line between them, and
    /// from the first character to the last it is the whole text (select
    /// all). With the carets in different blocks the selection goes by
    /// position, as if the screen were one page read row after row: from
    /// the caret that comes first to the end of its line, every line after
    /// it and before the other caret's line, then that line up to the
    /// caret. Only lines within the width of the two blocks count, so text
    /// in a pane left or right of them is left out.
    fn pieces(&self, a: usize, b: usize) -> Vec<(usize, usize, usize)> {
        let (a, b) = (a.min(b), a.max(b).min(self.len()));
        if a >= b {
            return Vec::new();
        }
        let (la, lb) = (self.line_of(a), self.line_of(b));
        if (a == 0 && b == self.len()) || self.block_of[la] == self.block_of[lb] {
            return (la..=lb)
                .filter_map(|line| {
                    let (s, e) = self.lines[line];
                    let (from, to) = (a.max(s), b.min(e));
                    (from < to).then_some((line, from, to))
                })
                .collect();
        }

        let ((start, first), (end, last)) = if self.before(la, lb) { ((a, la), (b, lb)) } else { ((b, lb), (a, la)) };
        let (near, far) = (self.block_rects[self.block_of[first]], self.block_rects[self.block_of[last]]);
        let (left, right) = (near.left.min(far.left), near.right.max(far.right));
        let mut out: Vec<(usize, usize, usize)> = (0..self.lines.len())
            .filter_map(|line| {
                let (s, e) = self.lines[line];
                let r = self.line_rects[line];
                let (from, to) = if line == first {
                    (start, e)
                } else if line == last {
                    (s, end)
                } else if self.before(first, line) && self.before(line, last) && r.left < right && r.right > left {
                    (s, e)
                } else {
                    return None;
                };
                (from < to).then_some((line, from, to))
            })
            .collect();
        // Row after row, left to right in each.
        out.sort_by_key(|&(line, ..)| self.line_rects[line].top + self.line_rects[line].bottom);
        let mut row_start = 0;
        for i in 1..=out.len() {
            if i == out.len() || !self.same_row(out[row_start].0, out[i].0) {
                out[row_start..i].sort_by_key(|&(line, ..)| self.line_rects[line].left);
                row_start = i;
            }
        }
        out
    }

    /// Reading row after row: `a` is on a row above `b`'s, or left of it on
    /// the same row.
    fn before(&self, a: usize, b: usize) -> bool {
        let (ra, rb) = (self.line_rects[a], self.line_rects[b]);
        if self.same_row(a, b) { ra.left < rb.left } else { ra.top + ra.bottom < rb.top + rb.bottom }
    }

    /// Two lines on one row (a table row's cells, a label and its value).
    fn same_row(&self, a: usize, b: usize) -> bool {
        let (a, b) = (self.line_rects[a], self.line_rects[b]);
        let shared = a.bottom.min(b.bottom) - a.top.max(b.top);
        shared * 2 > (a.bottom - a.top).min(b.bottom - b.top)
    }

    /// The selected text: lines separated by newlines, or by a space when
    /// they sit on one row.
    pub fn text(&self, a: usize, b: usize) -> String {
        let mut out = String::new();
        let mut prev = None;
        for (line, from, to) in self.pieces(a, b) {
            if let Some(prev) = prev {
                out.push(if self.same_row(prev, line) { ' ' } else { '\n' });
            }
            out.extend(self.chars[from..to].iter().map(|c| c.c));
            prev = Some(line);
        }
        out
    }

    fn piece_rect(&self, (line, from, to): (usize, usize, usize)) -> RECT {
        let r = self.line_rects[line];
        RECT { left: self.chars[from].x0, top: r.top, right: self.chars[to - 1].x1, bottom: r.bottom }
    }

    /// The selection line by line: each line's highlight box and the
    /// selected text on it.
    pub fn selected_lines(&self, a: usize, b: usize) -> Vec<(RECT, String)> {
        self.pieces(a, b).into_iter().map(|p| (self.piece_rect(p), self.chars[p.1..p.2].iter().map(|c| c.c).collect())).collect()
    }

    /// Highlight boxes for the selection: one per line it touches.
    pub fn highlight(&self, a: usize, b: usize) -> Vec<RECT> {
        self.pieces(a, b).into_iter().map(|p| self.piece_rect(p)).collect()
    }
}

/// Where the overlay takes the mouse: each paragraph's box grown by a wide
/// margin (3 line heights sideways, 1.5 above and below). People start a drag
/// above and left of a paragraph, end it below and right of it, or press
/// between lines; those presses must start a selection, not drag the image.
/// Lines close together (gap up to 1.5 line heights, overlapping sideways)
/// form one paragraph; a far-off caption keeps its own smaller area, so the
/// rest of the image still gets the app's own clicks and drags.
///
/// On a paused video or an app's own surface (`tight`) the margin is one line
/// height sideways and a third above and below: players and apps put their
/// controls right next to text (the seek bar under a subtitle), and they keep
/// their drags.
pub fn grab_areas(lines: &[RECT], tight: bool) -> Vec<RECT> {
    paragraphs(lines)
        .into_iter()
        .map(|p| {
            let (b, h) = (p.rect, p.line_height);
            let (dx, dy) = if tight { (h, h / 3) } else { ((3 * h).max(32), (3 * h / 2).max(16)) };
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
        let areas = grab_areas(&lines, false);
        assert_eq!(areas, vec![rect(40, 70, 560, 222)]);
    }

    #[test]
    fn distant_caption_keeps_its_own_area() {
        let lines = [rect(100, 100, 500, 120), rect(100, 136, 480, 156), rect(600, 700, 700, 716)];
        let areas = grab_areas(&lines, false);
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

    /// One word per line, given as (left, top, right, bottom, text), in the
    /// order OCR reported them.
    fn layout(lines: &[(i32, i32, i32, i32, &str)]) -> TextModel {
        let words: Vec<Word> = lines.iter().enumerate().map(|(i, &(l, t, r, b, text))| Word { rect: rect(l, t, r, b), text: text.into(), line: i }).collect();
        let rects: Vec<RECT> = words.iter().map(|w| w.rect).collect();
        TextModel::new(&words, &rects)
    }

    fn drag(m: &TextModel, from: (i32, i32), to: (i32, i32)) -> String {
        m.text(m.hit(POINT { x: from.0, y: from.1 }), m.hit(POINT { x: to.0, y: to.1 }))
    }

    /// A settings form: two radio labels and a checkbox label on one row,
    /// each with another line under it (boxes from a real screenshot).
    fn form() -> TextModel {
        layout(&[
            (65, 480, 250, 497, "created"),
            (296, 480, 467, 497, "assigned"),
            (65, 513, 307, 530, "account"),
            (684, 480, 995, 497, "instant"),
            (684, 513, 1252, 530, "never"),
        ])
    }

    #[test]
    fn a_click_lands_on_the_line_under_it_not_the_first_on_that_row() {
        let m = form();
        assert_eq!(drag(&m, (686, 487), (994, 487)), "instant");
        assert_eq!(drag(&m, (298, 487), (466, 487)), "assigned");
        // Between two rows of the right column: still that column.
        assert_eq!(drag(&m, (686, 509), (1250, 521)), "never");
    }

    #[test]
    fn dragging_down_a_column_leaves_the_column_beside_it() {
        let m = form();
        assert_eq!(drag(&m, (686, 487), (1250, 521)), "instant\nnever");
        assert_eq!(drag(&m, (66, 487), (306, 521)), "created\naccount");
    }

    #[test]
    fn dragging_along_a_row_takes_that_row() {
        let m = form();
        assert_eq!(drag(&m, (66, 487), (994, 487)), "created assigned instant");
        // The same from right to left.
        assert_eq!(drag(&m, (994, 487), (66, 487)), "created assigned instant");
    }

    #[test]
    fn a_selection_in_one_pane_skips_the_panes_beside_it() {
        // Two comments in the middle pane, a sidebar on the left and a
        // property on the right at the same heights.
        let m = layout(&[
            (20, 100, 200, 116, "sidebar"),
            (400, 100, 900, 116, "first"),
            (1000, 100, 1100, 116, "status"),
            (20, 180, 200, 196, "inbox"),
            (400, 180, 900, 196, "second"),
        ]);
        assert_eq!(drag(&m, (401, 108), (899, 188)), "first\nsecond");
    }

    #[test]
    fn a_line_ocr_broke_in_two_reads_row_after_row() {
        // "tab [6]. compared" with the reference dropped: two pieces on one
        // row, each with a line under it, reported out of reading order.
        let m = layout(&[(12, 700, 406, 716, "above"), (12, 808, 179, 821, "tab"), (12, 828, 177, 844, "methods"), (210, 808, 406, 825, "compared"), (244, 829, 406, 845, "achieves")]);
        assert_eq!(drag(&m, (13, 815), (405, 837)), "tab compared\nmethods achieves");
    }

    #[test]
    fn everything_is_selected_from_the_first_character_to_the_last() {
        let m = form();
        assert_eq!(m.text(0, m.len()).split_whitespace().count(), 5);
        assert_eq!(m.highlight(0, m.len()).len(), 5);
    }

    #[test]
    fn beside_a_short_last_line_is_that_line() {
        let m = layout(&[(100, 100, 500, 116, "long"), (100, 124, 180, 140, "end"), (560, 124, 900, 140, "other")]);
        // Right of "end", inside its paragraph: not the longer line above,
        // nor the column that starts further right.
        let at = m.hit(POINT { x: 450, y: 132 });
        assert_eq!(m.text(m.line_at(at).0, at), "end");
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
