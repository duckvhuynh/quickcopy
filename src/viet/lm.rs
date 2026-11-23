//! Picks the tone marks VieSVTR was unsure of with how often each syllable
//! and pair of syllables occurs in Vietnamese Wikipedia
//! (`tools/vietlm/build.py` builds the table).
//!
//! VieSVTR often puts a mark on a vowel that the line does not have
//! ("chuyển khoản" read as "chuyến khoản"). Where its next choices for a
//! letter were close, each word is tried with those letters too, and the
//! line keeps the words whose reading and Vietnamese together cost least.

use std::collections::HashMap;
use std::path::Path;

/// -ln P costs in the table are in steps of this.
const STEP: f32 = 0.1;
/// Cost of a syllable not in the table (names, other languages): a little
/// more than the rarest one kept.
const UNKNOWN: f32 = 18.0;
/// Added to a syllable's own cost when the pair was never seen.
const BACKOFF: f32 = 1.0;
/// How much Vietnamese counts against the model's reading (both in nats).
/// On the corpus's Vietnamese images 0.5-1.0 all cut wrong marks by 60-70%;
/// 1.5 starts overruling marks the model read right.
const LM_WEIGHT: f32 = 0.8;
/// Readings of a word tried, at most.
const BEAM: usize = 16;

pub struct Lm {
    ids: HashMap<Box<str>, u16>,
    /// Per syllable, its cost alone.
    alone: Vec<u8>,
    /// Followers of syllable i are `next[starts[i]..starts[i + 1]]`, ascending.
    starts: Vec<u32>,
    next: Vec<u16>,
    after: Vec<u8>,
}

/// A letter the model may have read, and how much less likely than its
/// first choice (nats, 0 for the first choice itself).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Choice {
    pub ch: char,
    pub cost: f32,
}

impl Lm {
    pub fn load(path: &Path) -> Result<Self, String> {
        let data = std::fs::read(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        Self::parse(&data).ok_or_else(|| format!("{} is not a syllable table", path.display()))
    }

    fn parse(data: &[u8]) -> Option<Self> {
        let mut at = 0usize;
        let mut take = |n: usize| {
            let s = data.get(at..at + n)?;
            at += n;
            Some(s)
        };
        if take(4)? != b"VLM1" {
            return None;
        }
        let u32_at = |s: &[u8]| u32::from_le_bytes(s.try_into().unwrap()) as usize;
        let n = u32_at(take(4)?);
        let len = u32_at(take(4)?);
        let words = std::str::from_utf8(take(len)?).ok()?;
        let ids: HashMap<Box<str>, u16> = words.split('\n').enumerate().map(|(i, w)| (w.into(), i as u16)).collect();
        if ids.len() != n || n > u16::MAX as usize + 1 {
            return None;
        }
        let alone = take(n)?.to_vec();
        let mut rest = &data[at..];
        fn byte(rest: &mut &[u8]) -> Option<u8> {
            let (&b, tail) = rest.split_first()?;
            *rest = tail;
            Some(b)
        }
        fn varint(rest: &mut &[u8]) -> Option<usize> {
            let mut v = 0usize;
            for shift in (0..35).step_by(7) {
                let b = byte(rest)?;
                v |= ((b & 0x7F) as usize) << shift;
                if b < 0x80 {
                    return Some(v);
                }
            }
            None
        }
        let mut starts = Vec::with_capacity(n + 1);
        let mut next = Vec::new();
        let mut after = Vec::new();
        starts.push(0);
        for _ in 0..n {
            let count = varint(&mut rest)?;
            let mut id = 0usize;
            for _ in 0..count {
                id += varint(&mut rest)?;
                next.push(u16::try_from(id).ok().filter(|&i| (i as usize) < n)?);
                after.push(byte(&mut rest)?);
            }
            starts.push(next.len() as u32);
        }
        next.shrink_to_fit();
        after.shrink_to_fit();
        Some(Self { ids, alone, starts, next, after })
    }

    /// Cost of syllable `w` (lowercase) after `prev`.
    fn cost(&self, prev: Option<u16>, w: Option<u16>) -> f32 {
        let Some(w) = w else { return UNKNOWN };
        if let Some(p) = prev {
            let (lo, hi) = (self.starts[p as usize] as usize, self.starts[p as usize + 1] as usize);
            if let Ok(i) = self.next[lo..hi].binary_search(&w) {
                return self.after[lo + i] as f32 * STEP;
            }
            return BACKOFF + self.alone[w as usize] as f32 * STEP;
        }
        self.alone[w as usize] as f32 * STEP
    }

    fn id(&self, word: &str) -> Option<u16> {
        self.ids.get(word.to_lowercase().as_str()).copied()
    }

    /// One line's letters, each with the model's choices for it (first
    /// choice first); returns the letter picked for each.
    pub fn pick(&self, line: &[Vec<Choice>]) -> Vec<char> {
        let mut out: Vec<char> = line.iter().map(|c| c[0].ch).collect();
        // Words: runs of letters between spaces, without punctuation
        // around them. Anything else breaks the run of neighbours.
        let mut runs: Vec<Vec<(usize, usize)>> = vec![Vec::new()];
        let mut i = 0;
        while i < line.len() {
            if out[i] == ' ' {
                i += 1;
                continue;
            }
            let start = i;
            while i < line.len() && out[i] != ' ' {
                i += 1;
            }
            let (mut a, mut b) = (start, i);
            while a < b && !out[a].is_alphabetic() {
                a += 1;
            }
            while b > a && !out[b - 1].is_alphabetic() {
                b -= 1;
            }
            let word = a < b && out[a..b].iter().all(|c| c.is_alphabetic());
            if !word || a > start {
                runs.push(Vec::new());
            }
            if word {
                runs.last_mut().unwrap().push((a, b));
            }
            if b < i {
                runs.push(Vec::new());
            }
        }
        for run in runs.iter().filter(|r| !r.is_empty()) {
            // Capitals without marks are meant that way: names on bank
            // transfers and cards ("NGUYEN VAN HUONG").
            let bare = |&(a, b): &(usize, usize)| out[a..b].iter().all(|&c| c.is_ascii_uppercase());
            if run.len() >= 2 && run.iter().all(bare) {
                continue;
            }
            let cands: Vec<Vec<(String, f32, Option<u16>)>> = run.iter().map(|&(a, b)| self.readings(&line[a..b])).collect();
            // Viterbi over the run's words.
            let mut best: Vec<(f32, usize)> = cands[0].iter().map(|c| (c.1 + LM_WEIGHT * self.cost(None, c.2), 0)).collect();
            let mut back: Vec<Vec<usize>> = Vec::new();
            for k in 1..cands.len() {
                let mut now = Vec::with_capacity(cands[k].len());
                for c in &cands[k] {
                    let (from, score) = cands[k - 1]
                        .iter()
                        .enumerate()
                        .map(|(j, p)| (j, best[j].0 + LM_WEIGHT * self.cost(p.2, c.2)))
                        .min_by(|x, y| x.1.total_cmp(&y.1))
                        .unwrap();
                    now.push((score + c.1, from));
                }
                back.push(now.iter().map(|n| n.1).collect());
                best = now;
            }
            let mut j = best.iter().enumerate().min_by(|x, y| x.1.0.total_cmp(&y.1.0)).map_or(0, |b| b.0);
            for k in (0..run.len()).rev() {
                let (a, _) = run[k];
                for (o, ch) in out[a..].iter_mut().zip(cands[k][j].0.chars()) {
                    *o = ch;
                }
                if k > 0 {
                    j = back[k - 1][j];
                }
            }
        }
        out
    }

    /// A word's likeliest readings by the model: (text, cost, id). The
    /// first choice of every letter comes first.
    fn readings(&self, letters: &[Vec<Choice>]) -> Vec<(String, f32, Option<u16>)> {
        let mut beam: Vec<(String, f32)> = vec![(String::new(), 0.0)];
        for choices in letters {
            let mut grown: Vec<(String, f32)> = beam
                .iter()
                .flat_map(|(s, c)| {
                    choices.iter().map(move |ch| {
                        let mut s = s.clone();
                        s.push(ch.ch);
                        (s, c + ch.cost)
                    })
                })
                .collect();
            // Stable: ties keep the first choices first.
            grown.sort_by(|x, y| x.1.total_cmp(&y.1));
            grown.truncate(BEAM);
            beam = grown;
        }
        beam.into_iter()
            .map(|(s, c)| {
                let id = self.id(&s);
                (s, c, id)
            })
            .collect()
    }
}

/// Letters that differ only in their marks, uppercase ones apart.
const FAMILIES: [&str; 7] = [
    "aàảãáạăằẳẵắặâầẩẫấậ",
    "eèẻẽéẹêềểễếệ",
    "iìỉĩíị",
    "oòỏõóọôồổỗốộơờởỡớợ",
    "uùủũúụưừửữứự",
    "yỳỷỹýỵ",
    "dđ",
];

/// The letters that differ from `ch` only in their marks, `ch` included,
/// in the same case; empty for anything else.
pub fn family(ch: char) -> Vec<char> {
    let lower = ch.to_lowercase().next().unwrap_or(ch);
    let upper = ch != lower;
    FAMILIES
        .iter()
        .find(|f| f.contains(lower))
        .map(|f| f.chars().map(|c| if upper { c.to_uppercase().next().unwrap_or(c) } else { c }).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(words: &[(&str, u8)], pairs: &[(usize, usize, u8)]) -> Lm {
        let mut d = b"VLM1".to_vec();
        let joined = words.iter().map(|w| w.0).collect::<Vec<_>>().join("\n");
        d.extend((words.len() as u32).to_le_bytes());
        d.extend((joined.len() as u32).to_le_bytes());
        d.extend(joined.as_bytes());
        d.extend(words.iter().map(|w| w.1));
        for i in 0..words.len() {
            let mine: Vec<_> = pairs.iter().filter(|p| p.0 == i).collect();
            d.push(mine.len() as u8);
            let mut prev = 0;
            for p in mine {
                d.push((p.1 - prev) as u8);
                prev = p.1;
                d.push(p.2);
            }
        }
        Lm::parse(&d).unwrap()
    }

    fn line(s: &[&[(char, f32)]]) -> Vec<Vec<Choice>> {
        s.iter().map(|c| c.iter().map(|&(ch, cost)| Choice { ch, cost }).collect()).collect()
    }

    #[test]
    fn pair_picks_the_mark() {
        // "chuyến" alone is as common as "chuyển", but not before "khoản".
        let lm = table(&[("chuyển", 80), ("chuyến", 80), ("khoản", 90)], &[(0, 2, 10), (1, 2, 90)]);
        let mut l = line(&[&[('C', 0.0)], &[('h', 0.0)], &[('u', 0.0)], &[('y', 0.0)], &[('ế', 0.0), ('ể', 1.0)], &[('n', 0.0)], &[(' ', 0.0)]]);
        l.extend(line(&[&[('k', 0.0)], &[('h', 0.0)], &[('o', 0.0)], &[('ả', 0.0)], &[('n', 0.0)], &[(',', 0.0)]]));
        let got: String = lm.pick(&l).into_iter().collect();
        assert_eq!(got, "Chuyển khoản,");
    }

    #[test]
    fn unknown_words_keep_the_first_choice() {
        let lm = table(&[("a", 10)], &[]);
        let l = line(&[&[('X', 0.0)], &[('o', 0.0), ('ó', 0.5)], &[('m', 0.0)], &[(' ', 0.0)], &[('2', 0.0)]]);
        assert_eq!(lm.pick(&l).into_iter().collect::<String>(), "Xom 2");
    }

    #[test]
    fn capitals_without_marks_stay() {
        let lm = table(&[("nguyễn", 10), ("văn", 10), ("nguyen", 200), ("van", 200)], &[]);
        let caps: Vec<&[(char, f32)]> = vec![&[('N', 0.0)], &[('G', 0.0)], &[('U', 0.0)], &[('Y', 0.0)], &[('E', 0.0), ('Ễ', 0.5)], &[('N', 0.0)], &[(' ', 0.0)], &[('V', 0.0)], &[('A', 0.0), ('Ă', 0.5)], &[('N', 0.0)]];
        assert_eq!(lm.pick(&line(&caps)).into_iter().collect::<String>(), "NGUYEN VAN");
    }

    #[test]
    fn families_keep_case() {
        assert!(family('Ể').contains(&'E') && family('Ể').contains(&'Ệ'));
        assert_eq!(family('đ'), vec!['d', 'đ']);
        assert!(family('k').is_empty());
    }

    #[test]
    fn varint_counts_and_deltas_round_trip() {
        let lm = table(&[("a", 5), ("b", 6), ("c", 7)], &[(0, 1, 3), (0, 2, 4), (2, 0, 9)]);
        assert_eq!(lm.cost(Some(0), Some(2)), 4.0 * STEP);
        assert_eq!(lm.cost(Some(2), Some(0)), 9.0 * STEP);
        assert_eq!(lm.cost(Some(1), Some(0)), BACKOFF + 5.0 * STEP);
        assert_eq!(lm.cost(None, None), UNKNOWN);
    }
}
