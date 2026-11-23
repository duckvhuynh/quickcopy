//! Fits translated text into the boxes the original text occupied: words are
//! wrapped into the boxes in order, and the font shrinks until they fit.

/// Smallest font, as a fraction of the original size, before giving up and
/// letting the last box overflow.
const MIN_SCALE: f32 = 0.6;
const STEP: f32 = 0.92;

#[derive(Debug, PartialEq)]
pub struct Fit {
    pub size: f32,
    /// Text for each box (may be empty).
    pub lines: Vec<String>,
}

/// A piece of text lines may break before, and whether a space separates it
/// from the previous piece.
struct Token<'a> {
    text: &'a str,
    space: bool,
}

/// Chinese and Japanese write without spaces: every ideograph, kana or
/// CJK punctuation mark is its own breaking point.
fn breaks_anywhere(c: char) -> bool {
    matches!(c as u32,
        0x3000..=0x30FF       // CJK punctuation, hiragana, katakana
        | 0x3400..=0x4DBF     // CJK extension A
        | 0x4E00..=0x9FFF     // CJK unified ideographs
        | 0xF900..=0xFAFF     // CJK compatibility ideographs
        | 0xFF00..=0xFFEF)    // full-width forms
}

fn tokens(text: &str) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    for word in text.split_whitespace() {
        let mut first = true;
        let mut start = 0;
        for (i, c) in word.char_indices() {
            if breaks_anywhere(c) {
                if start < i {
                    out.push(Token { text: &word[start..i], space: first });
                    first = false;
                }
                let end = i + c.len_utf8();
                out.push(Token { text: &word[i..end], space: first });
                first = false;
                start = end;
            }
        }
        if start < word.len() {
            out.push(Token { text: &word[start..], space: first });
        }
    }
    out
}

/// Wraps `text` into boxes of the given `widths`, starting at font size
/// `base`. `measure(text, size)` returns the drawn width.
pub fn fit(text: &str, widths: &[f32], base: f32, measure: &mut dyn FnMut(&str, f32) -> f32) -> Fit {
    let tokens = tokens(text);
    if widths.is_empty() {
        return Fit { size: base, lines: Vec::new() };
    }
    let min = base * MIN_SCALE;
    let mut size = base;
    loop {
        let (mut lines, placed) = wrap(&tokens, widths, size, measure);
        if placed == tokens.len() {
            return Fit { size, lines };
        }
        if size <= min {
            // Still too long: the rest goes on the last box (it overflows).
            let last = lines.last_mut().expect("widths is not empty");
            for t in &tokens[placed..] {
                if t.space && !last.is_empty() {
                    last.push(' ');
                }
                last.push_str(t.text);
            }
            return Fit { size, lines };
        }
        size = (size * STEP).max(min);
    }
}

/// Greedy wrap; returns the lines and how many tokens were placed.
fn wrap(tokens: &[Token], widths: &[f32], size: f32, measure: &mut dyn FnMut(&str, f32) -> f32) -> (Vec<String>, usize) {
    let mut i = 0;
    let mut lines = Vec::with_capacity(widths.len());
    for &width in widths {
        let mut line = String::new();
        while i < tokens.len() {
            let t = &tokens[i];
            let candidate = if line.is_empty() {
                t.text.to_string()
            } else if t.space {
                format!("{line} {}", t.text)
            } else {
                format!("{line}{}", t.text)
            };
            if measure(&candidate, size) <= width {
                line = candidate;
                i += 1;
            } else {
                break;
            }
        }
        lines.push(line);
    }
    (lines, i)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Monospace: every character is half the font size wide.
    fn mono(text: &str, size: f32) -> f32 {
        text.chars().count() as f32 * size * 0.5
    }

    #[test]
    fn wraps_into_boxes_at_full_size() {
        let f = fit("aaa bbb ccc", &[40.0, 40.0], 10.0, &mut mono);
        assert_eq!(f, Fit { size: 10.0, lines: vec!["aaa bbb".into(), "ccc".into()] });
    }

    #[test]
    fn shrinks_until_it_fits() {
        // "aaa bbb" is 35 px at size 10, the boxes are 30 px: needs a smaller font.
        let f = fit("aaa bbb ccc ddd", &[30.0, 30.0], 10.0, &mut mono);
        assert!(f.size < 10.0 && f.size >= 6.0, "{f:?}");
        assert_eq!(f.lines.join(" "), "aaa bbb ccc ddd");
    }

    #[test]
    fn wraps_japanese_between_characters() {
        // No spaces: breaks fall between characters, and none are added.
        let f = fit("売上高は前年比18％増加した。", &[40.0, 40.0], 10.0, &mut mono);
        assert_eq!(f.size, 10.0);
        assert_eq!(f.lines.concat(), "売上高は前年比18％増加した。");
        assert!(f.lines.iter().all(|l| mono(l, 10.0) <= 40.0), "{f:?}");
    }

    #[test]
    fn overflows_last_box_at_minimum_size() {
        let f = fit("aaaa bbbb cccc dddd eeee", &[10.0], 10.0, &mut mono);
        assert_eq!(f.size, 6.0);
        assert_eq!(f.lines, vec!["aaaa bbbb cccc dddd eeee".to_string()]);
    }
}
