//! Vietnamese text-line recognition with VieSVTR v7 (ONNX, greedy CTC).
//!
//! Model and charset: https://huggingface.co/ngocthua830/viesvtr
//! (CC BY-NC 4.0: non-commercial use only).
//!
//! Input is one cropped text line. Windows OCR finds the lines; this model
//! reads them, because Windows OCR has no Vietnamese recognizer.

use std::path::{Path, PathBuf};

use ort::session::Session;
use ort::value::Tensor;

const HEIGHT: usize = 64;
const W_STRIDE: usize = 4;
const W_MIN: usize = 32;
const W_MAX: usize = 1600;
const MODEL_FILE: &str = "models/viesvtr/viesvtr_v7.onnx";

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
}

impl Recognizer {
    pub fn load(path: &Path) -> Result<Self, String> {
        let err = |e: &dyn std::fmt::Display| format!("loading {}: {e}", path.display());
        let runtime = runtime_path().ok_or("onnxruntime.dll not found next to the exe or in System32")?;
        let env = ort::init_from(&runtime)
            .map_err(|e| format!("loading {}: {e}", runtime.display()))?
            .with_telemetry(false)
            .build()
            .map_err(|e| err(&e))?;
        // ONNX Runtime already defaults to full graph optimization; setting the
        // level explicitly is rejected by the 1.17 runtime Windows ships.
        let session = Session::builder(&env)
            .map_err(|e| err(&e))?
            .with_intra_threads(4)
            .map_err(|e| err(&e))?
            .commit_from_file(path)
            .map_err(|e| err(&e))?;
        Ok(Self { session, vocab: VOCAB.chars().collect() })
    }

    /// Recognizes one text line given as a top-down BGRA buffer.
    pub fn recognize(&mut self, px: &[u8], w: usize, h: usize) -> Result<Vec<RecChar>, String> {
        if w == 0 || h == 0 {
            return Ok(Vec::new());
        }
        let tw = target_width(w, h);
        let input = preprocess(px, w, h, tw);
        let tensor = Tensor::from_array(([1usize, 3, HEIGHT, tw], input)).map_err(|e| e.to_string())?;
        let outputs = self.session.run(ort::inputs![tensor]).map_err(|e| e.to_string())?;
        let (shape, logits) = outputs[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
        // Expected [1, T, classes].
        let dims: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
        let (t, v) = match dims.as_slice() {
            [1, t, v] => (*t, *v),
            other => return Err(format!("unexpected output shape {other:?}")),
        };
        Ok(ctc_greedy(logits, t, v)
            .into_iter()
            .filter_map(|(class, t0, t1)| {
                let ch = *self.vocab.get(class.checked_sub(1)?)?;
                Some(RecChar { ch, x0: t0 as f32 / t as f32, x1: (t1 + 1) as f32 / t as f32 })
            })
            .collect())
    }
}

/// Looks for the model next to the executable, then in its parent folders
/// (so `target/release/quickcopy.exe` finds the project's `models/`).
pub fn model_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors().skip(1).take(4).map(|dir| dir.join(MODEL_FILE)).find(|p| p.is_file())
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
