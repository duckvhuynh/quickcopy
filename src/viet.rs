//! Vietnamese text-line recognition with VieSVTR v7 (ONNX, greedy CTC), its
//! unsure tone marks picked by syllable frequencies (`viet/lm.rs`).
//!
//! Model and charset: https://huggingface.co/ngocthua830/viesvtr
//! (CC BY-NC 4.0: non-commercial use only).
//!
//! Input is one cropped text line. Windows OCR finds the lines; this model
//! reads them, because Windows OCR has no Vietnamese recognizer.

use std::path::{Path, PathBuf};

mod lm;

use ort::session::Session;
use ort::value::Tensor;

const HEIGHT: usize = 64;
const W_STRIDE: usize = 4;
const W_MIN: usize = 32;
const W_MAX: usize = 1600;
pub const MODEL_FILE: &str = "viesvtr_v7.onnx";
/// Syllable frequencies that pick tone marks (`lm.rs`); in the release zip.
const SYLLABLES_FILE: &str = "vi_syllables.bin";
/// A letter with other marks is tried when the model gave it at most this
/// much less (nats) than its first choice; at most this many per letter.
const CLOSE: f32 = 9.0;
const TRIED: usize = 4;
/// Lines per model run. Measured on a 14-line page: 1 line 320 ms,
/// 4 lines 285 ms, 16 lines 350 ms (bigger batches pad too much).
const MAX_BATCH: usize = 4;

/// VietOCR's 229 characters; class i+1 is VOCAB[i], class 0 is the CTC blank.
const VOCAB: &str = include_str!("vocab.txt");

/// One recognized character and the horizontal span it came from, as
/// fractions (0..1) of the line crop's width.
#[derive(Debug, Clone, PartialEq)]
pub struct RecChar {
    pub ch: char,
    pub x0: f32,
    pub x1: f32,
}

pub struct Recognizer {
    session: Session,
    vocab: Vec<char>,
    syllables: Option<lm::Lm>,
}

/// ONNX Runtime allows one environment per process, so it is made once and
/// kept: the worker drops the model when idle and loads it again later, and
/// a second environment failed that load, which turned Vietnamese reading
/// off until the app restarted.
fn environment() -> Result<ort::environment::Environment, String> {
    static ENV: std::sync::OnceLock<Result<ort::environment::Environment, String>> = std::sync::OnceLock::new();
    ENV.get_or_init(|| {
        let runtime = runtime_path().ok_or("onnxruntime.dll not found next to the exe or in System32")?;
        ort::init_from(&runtime)
            .map_err(|e| format!("loading {}: {e}", runtime.display()))?
            .with_telemetry(false)
            .build()
            .map_err(|e| format!("starting ONNX Runtime: {e}"))
    })
    .clone()
}

impl Recognizer {
    pub fn load(path: &Path) -> Result<Self, String> {
        let err = |e: &dyn std::fmt::Display| format!("loading {}: {e}", path.display());
        let env = environment()?;
        // ONNX Runtime already defaults to full graph optimization; setting the
        // level explicitly is rejected by the 1.17 runtime Windows ships.
        // Its memory arena stays on: without it the session holds 65 MB
        // instead of 110-175 MB, but reads take 1.5-2x as long per line. The
        // worker unloads the model when idle instead.
        // Threads: about one per physical core, at most 8, and no spinning
        // between steps. Reading 42 lines on 16 cores, 8 threads take as long
        // as 16 (1.5-1.7 s) for half the CPU time (10 s vs 16-20 s); spinning
        // added another 50% of CPU time for no speed.
        let logical = std::thread::available_parallelism().map_or(2, |n| n.get());
        let session = Session::builder(&env)
            .map_err(|e| err(&e))?
            .with_intra_threads((logical / 2).clamp(1, 8))
            .map_err(|e| err(&e))?
            .with_intra_op_spinning(false)
            .map_err(|e| err(&e))?
            .commit_from_file(path)
            .map_err(|e| err(&e))?;
        // Without the table lines are read as the model reads them.
        let syllables = find(SYLLABLES_FILE)
            .ok_or_else(|| format!("{SYLLABLES_FILE} not found"))
            .and_then(|p| lm::Lm::load(&p))
            .map_err(|e| crate::common::log(format!("Vietnamese tone marks as read: {e}")))
            .ok();
        Ok(Self { session, vocab: VOCAB.chars().collect(), syllables })
    }

    /// Recognizes text lines. Lines of similar width run together as one
    /// batch, which is much faster than one model run per line.
    pub fn recognize_batch(&mut self, lines: &[Line]) -> Result<Vec<Vec<RecChar>>, String> {
        let mut out = vec![Vec::new(); lines.len()];
        let mut order: Vec<(usize, usize)> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.w > 0 && l.h > 0)
            .map(|(i, l)| (i, target_width(l.w, l.h)))
            .collect();
        order.sort_by_key(|&(_, tw)| tw);

        let mut start = 0;
        while start < order.len() {
            // Grow the batch while the padding it costs stays under 50%.
            let mut end = start + 1;
            while end < order.len() && end - start < MAX_BATCH && order[end].1 * 2 <= order[start].1 * 3 {
                end += 1;
            }
            let group = &order[start..end];
            let wmax = group[group.len() - 1].1;
            let plane = HEIGHT * wmax;
            let mut input = vec![0f32; group.len() * 3 * plane];
            for (b, &(i, tw)) in group.iter().enumerate() {
                let line = &lines[i];
                let one = preprocess(line.px, line.w, line.h, tw);
                for c in 0..3 {
                    for y in 0..HEIGHT {
                        let src = &one[c * HEIGHT * tw + y * tw..][..tw];
                        let dst = &mut input[b * 3 * plane + c * plane + y * wmax..][..wmax];
                        dst[..tw].copy_from_slice(src);
                        // Pad with the row's last pixel (background), not black.
                        dst[tw..].fill(src[tw - 1]);
                    }
                }
            }
            let tensor = Tensor::from_array(([group.len(), 3, HEIGHT, wmax], input)).map_err(|e| e.to_string())?;
            let outputs = self.session.run(ort::inputs![tensor]).map_err(|e| e.to_string())?;
            let (shape, logits) = outputs[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
            let dims: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
            let (t, v) = match dims.as_slice() {
                [n, t, v] if *n == group.len() => (*t, *v),
                other => return Err(format!("unexpected output shape {other:?}")),
            };
            for (b, &(i, tw)) in group.iter().enumerate() {
                // Only the steps covering this line's own width, not its padding.
                let steps = (t * tw).div_ceil(wmax).clamp(1, t);
                let rows = &logits[b * t * v..][..steps * v];
                let read: Vec<(usize, usize, usize)> =
                    ctc_greedy(rows, steps, v).into_iter().filter(|&(class, _, _)| class >= 1 && class <= self.vocab.len()).collect();
                let mut chars: Vec<char> = read.iter().map(|&(class, _, _)| self.vocab[class - 1]).collect();
                if let Some(lm) = &self.syllables {
                    chars = lm.pick(&choices(&self.vocab, rows, v, &read));
                }
                out[i] = read
                    .iter()
                    .zip(chars)
                    .map(|(&(_, t0, t1), ch)| RecChar { ch, x0: t0 as f32 / steps as f32, x1: (t1 + 1) as f32 / steps as f32 })
                    .collect();
            }
            start = end;
        }
        Ok(out)
    }
}

/// For each character read, the letters with other marks the model nearly
/// read there instead, with how much less likely (nats) at the steps it
/// read the character.
fn choices(vocab: &[char], rows: &[f32], v: usize, read: &[(usize, usize, usize)]) -> Vec<Vec<lm::Choice>> {
    let logp = log_probs(rows, v);
    let class_of = |ch: char| vocab.iter().position(|&c| c == ch).map(|i| i + 1);
    let best = |class: usize, t0: usize, t1: usize| (t0..=t1).map(|t| logp[t * v + class]).fold(f32::MIN, f32::max);
    read.iter()
        .map(|&(class, t0, t1)| {
            let ch = vocab[class - 1];
            let own = best(class, t0, t1);
            let mut out = vec![lm::Choice { ch, cost: 0.0 }];
            let mut others: Vec<lm::Choice> = lm::family(ch)
                .into_iter()
                .filter(|&c| c != ch)
                .filter_map(|c| Some(lm::Choice { ch: c, cost: own - best(class_of(c)?, t0, t1) }))
                .filter(|c| c.cost < CLOSE)
                .collect();
            others.sort_by(|a, b| a.cost.total_cmp(&b.cost));
            out.extend(others.into_iter().take(TRIED - 1));
            out
        })
        .collect()
}

/// Log-probabilities per step: the model's output may be scores
/// (log-softmax them) or already probabilities.
fn log_probs(rows: &[f32], v: usize) -> Vec<f32> {
    let mut out = rows.to_vec();
    for row in out.chunks_mut(v) {
        let sum: f32 = row.iter().sum();
        if row.iter().all(|&x| (0.0..=1.0).contains(&x)) && (sum - 1.0).abs() < 0.01 {
            row.iter_mut().for_each(|x| *x = x.max(1e-30).ln());
        } else {
            let max = row.iter().copied().fold(f32::MIN, f32::max);
            let lse = max + row.iter().map(|x| (x - max).exp()).sum::<f32>().ln();
            row.iter_mut().for_each(|x| *x -= lse);
        }
    }
    out
}

/// One text line crop: top-down BGRA pixels.
pub struct Line<'a> {
    pub px: &'a [u8],
    pub w: usize,
    pub h: usize,
}

/// Where the settings window downloads the model to:
/// `%LOCALAPPDATA%\QuickCopy\models\viesvtr\` (`model.rs`).
pub fn download_dir() -> Option<PathBuf> {
    Some(model_dir(&PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("QuickCopy")))
}

fn model_dir(base: &Path) -> PathBuf {
    base.join("models").join("viesvtr")
}

/// `models/viesvtr/<name>` in the download folder, then next to the
/// executable and in its parent folders (so `target/release/quickcopy.exe`
/// finds the project's `models/`).
fn find(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let beside = exe.ancestors().skip(1).take(4).map(model_dir);
    download_dir().into_iter().chain(beside).map(|dir| dir.join(name)).find(|p| p.is_file())
}

pub fn model_path() -> Option<PathBuf> {
    find(MODEL_FILE)
}

/// ONNX Runtime: a copy next to the exe wins, else the one Windows 11 ships.
fn runtime_path() -> Option<PathBuf> {
    let local = std::env::current_exe().ok()?.with_file_name("onnxruntime.dll");
    let system = PathBuf::from(std::env::var_os("SystemRoot")?).join("System32").join("onnxruntime.dll");
    [local, system].into_iter().find(|p| p.is_file())
}

/// Width after resizing to HEIGHT, rounded up to the model's stride.
fn target_width(w: usize, h: usize) -> usize {
    let nw = (HEIGHT * w).div_ceil(h);
    (nw.div_ceil(W_STRIDE) * W_STRIDE).clamp(W_MIN, W_MAX)
}

/// BGRA crop -> 1x3xHEIGHTxtw RGB planes in [-1, 1].
/// Bilinear when enlarging, box average when shrinking (like INTER_AREA).
fn preprocess(px: &[u8], w: usize, h: usize, tw: usize) -> Vec<f32> {
    let plane = HEIGHT * tw;
    let mut out = vec![0f32; 3 * plane];
    let sx = w as f32 / tw as f32;
    let sy = h as f32 / HEIGHT as f32;
    let at = |x: usize, y: usize, c: usize| px[(y * w + x) * 4 + c] as f32;
    for oy in 0..HEIGHT {
        for ox in 0..tw {
            let mut rgb = [0f32; 3];
            if sx > 1.0 || sy > 1.0 {
                // Box filter over the source area of this output pixel.
                let (x0, x1) = ((ox as f32 * sx) as usize, (((ox + 1) as f32 * sx).ceil() as usize).min(w).max(1));
                let (y0, y1) = ((oy as f32 * sy) as usize, (((oy + 1) as f32 * sy).ceil() as usize).min(h).max(1));
                let (x0, y0) = (x0.min(x1 - 1), y0.min(y1 - 1));
                let n = ((x1 - x0) * (y1 - y0)) as f32;
                for y in y0..y1 {
                    for x in x0..x1 {
                        for (c, v) in rgb.iter_mut().enumerate() {
                            *v += at(x, y, 2 - c); // BGRA -> RGB
                        }
                    }
                }
                rgb.iter_mut().for_each(|v| *v /= n);
            } else {
                let fx = ((ox as f32 + 0.5) * sx - 0.5).max(0.0);
                let fy = ((oy as f32 + 0.5) * sy - 0.5).max(0.0);
                let (x0, y0) = ((fx as usize).min(w - 1), (fy as usize).min(h - 1));
                let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
                let (ax, ay) = (fx - x0 as f32, fy - y0 as f32);
                for (c, v) in rgb.iter_mut().enumerate() {
                    let ch = 2 - c;
                    let top = at(x0, y0, ch) * (1.0 - ax) + at(x1, y0, ch) * ax;
                    let bottom = at(x0, y1, ch) * (1.0 - ax) + at(x1, y1, ch) * ax;
                    *v = top * (1.0 - ay) + bottom * ay;
                }
            }
            for (c, v) in rgb.iter().enumerate() {
                out[c * plane + oy * tw + ox] = (v / 255.0 - 0.5) / 0.5;
            }
        }
    }
    out
}

/// Greedy CTC over T x V logits: returns (class, first step, last step) per
/// emitted character, collapsing repeats and dropping blanks (class 0).
fn ctc_greedy(logits: &[f32], t: usize, v: usize) -> Vec<(usize, usize, usize)> {
    let mut out: Vec<(usize, usize, usize)> = Vec::new();
    let mut prev = usize::MAX;
    for step in 0..t {
        let row = &logits[step * v..(step + 1) * v];
        let best = (0..v).max_by(|&a, &b| row[a].total_cmp(&row[b])).unwrap_or(0);
        if best != 0 {
            if best == prev {
                if let Some(last) = out.last_mut() {
                    last.2 = step;
                }
            } else {
                out.push((best, step, step));
            }
        }
        prev = best;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vocab_matches_vietocr() {
        let chars: Vec<char> = VOCAB.chars().collect();
        assert_eq!(chars.len(), 229);
        let unique: std::collections::HashSet<_> = chars.iter().collect();
        assert_eq!(unique.len(), 229);
        for c in "ạảấầếệởữỵĐƯ ".chars() {
            assert!(chars.contains(&c), "missing {c}");
        }
    }

    #[test]
    fn width_follows_aspect_and_stride() {
        assert_eq!(target_width(100, 32), 200);
        assert_eq!(target_width(10, 64), 32);
        assert_eq!(target_width(10_000, 20), 1600);
        assert_eq!(target_width(101, 64), 104);
    }

    #[test]
    fn ctc_collapses_repeats_and_keeps_spans() {
        // classes per step: 5 5 0 5 7 7 0
        let seq = [5, 5, 0, 5, 7, 7, 0];
        let v = 8;
        let mut logits = vec![0f32; seq.len() * v];
        for (t, &c) in seq.iter().enumerate() {
            logits[t * v + c] = 1.0;
        }
        assert_eq!(ctc_greedy(&logits, seq.len(), v), vec![(5, 0, 1), (5, 3, 3), (7, 4, 5)]);
    }
}

#[cfg(test)]
mod reload_tests {
    /// The worker drops the model when idle and loads it again; a second
    /// load once failed and turned Vietnamese reading off. Runs where the
    /// model file is present (it is not in git).
    #[test]
    fn loads_again_after_being_dropped() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("models/viesvtr/viesvtr_v7.onnx");
        if !path.exists() {
            return;
        }
        for i in 0..3 {
            let r = super::Recognizer::load(&path);
            assert!(r.is_ok(), "load {i}: {:?}", r.err());
        }
    }
}
