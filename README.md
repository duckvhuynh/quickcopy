# QuickCopy

Live Text for Windows: rest the cursor on any image on screen, then
double-click a word, triple-click a line, or drag across text to copy it.
Works in browsers and in Electron apps such as Zalo. Reads English and
Vietnamese.

- ~330 KB executable, no GPU, ~3 MB RAM idle (~90 MB while the Vietnamese
  model is loaded; it unloads after 5 idle minutes).
- Rust + Win32, no UI framework. Runs in the tray (Pause, Vietnamese toggle, Quit).

## How it works

1. When the cursor rests for 300 ms, UI Automation is asked whether there is
   an image under it. Apps that expose no accessibility tree (Electron apps
   like Zalo) fall back to finding the image rectangle from pixels
   (`src/rectfind.rs`).
2. The image is captured with GDI, upscaled 2x and run through Windows OCR,
   which finds the lines and words.
3. In Vietnamese mode each line is re-read by
   [VieSVTR](https://huggingface.co/ngocthua830/viesvtr), because Windows OCR
   has no Vietnamese recognizer (`src/viet.rs`).
4. A transparent overlay is laid over the text lines only, so clicks elsewhere
   on the image still reach the app underneath.

## Build

```powershell
cargo build --release
```

### Vietnamese model (optional)

The model is not in this repository. Download
`weights/viesvtr_v7.onnx` (30.7 MB) from
<https://huggingface.co/ngocthua830/viesvtr> and place it at:

```
models/viesvtr/viesvtr_v7.onnx
```

next to the executable or in a parent folder of it. VieSVTR is licensed
**CC BY-NC 4.0 (non-commercial use only)**.

ONNX Runtime is loaded at runtime: `onnxruntime.dll` next to the executable
if present, otherwise the copy Windows 11 ships in `System32`. On Windows 10,
put an `onnxruntime.dll` (1.17 or newer) next to the executable.

## Debugging

```powershell
quickcopy --ocr-file image.png   # OCR an image file, print text and word boxes
quickcopy --probe X Y            # detect and OCR the image at a screen point
```

Debug builds log to the console. Release builds log to stderr when `QC_LOG`
is set. `QC_ENGLISH=1` forces Windows OCR only.

## Known limitations

- Images drawn on wallpapers or with edges close to the background color may
  not be found in Electron apps.
- A single click on a word inside the image is taken by the overlay, so image
  links do not open when clicked exactly on their text.
- Content behind DRM or windows running as administrator cannot be read.
