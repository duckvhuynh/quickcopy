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

/// Wraps `text` into boxes of the given `widths`, starting at font size
/// `base`. `measure(text, size)` returns the drawn width.
pub fn fit(text: &str, widths: &[f32], base: f32, measure: &mut dyn FnMut(&str, f32) -> f32) -> Fit {
    let words: Vec<&str> = text.split_whitespace().collect();
    if widths.is_empty() {
        return Fit { size: base, lines: Vec::new() };
    }
    let min = base * MIN_SCALE;
    let mut size = base;
    loop {
        let (mut lines, placed) = wrap(&words, widths, size, measure);
        if placed == words.len() {
            return Fit { size, lines };
        }
        if size <= min {
            // Still too long: the rest goes on the last box (it overflows).
            let last = lines.last_mut().expect("widths is not empty");
            for word in &words[placed..] {
                if !last.is_empty() {
                    last.push(' ');
                }
                last.push_str(word);
            }
            return Fit { size, lines };
        }
        size = (size * STEP).max(min);
    }
}

/// Greedy wrap; returns the lines and how many words were placed.
fn wrap(words: &[&str], widths: &[f32], size: f32, measure: &mut dyn FnMut(&str, f32) -> f32) -> (Vec<String>, usize) {
    let mut i = 0;
    let mut lines = Vec::with_capacity(widths.len());
    for &width in widths {
        let mut line = String::new();
        while i < words.len() {
            let candidate = if line.is_empty() { words[i].to_string() } else { format!("{line} {}", words[i]) };
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
    fn overflows_last_box_at_minimum_size() {
        let f = fit("aaaa bbbb cccc dddd eeee", &[10.0], 10.0, &mut mono);
        assert_eq!(f.size, 6.0);
        assert_eq!(f.lines, vec!["aaaa bbbb cccc dddd eeee".to_string()]);
    }
}
