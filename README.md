# QuickCopy

Live Text for Windows: rest the cursor on any image on screen and select the
text inside it like normal text, then copy it with Ctrl+C or the right-click
menu. Works in browsers and in Electron apps such as Zalo. Reads English and
Vietnamese.

**Videos** in web pages (Edge, Chrome): a paused video's frame is read like
an image; a playing one is left alone. The video counts as playing while
its picture changes, or while its tab plays sound (Windows media sessions,
matched by the tab's title), so a still picture with music is not read
either. Pausing it (a click, the space key, a media key) brings up the
overlay without moving the mouse: a playing video under the resting cursor
is checked twice a second, which costs about 1-2% of one core. Text the
player draws over the picture (controls, time, title, captions) is left
out, so the overlay never takes the player's clicks.

Selection works like native text: drag (it may start a little before or
after a line), double-click a word, triple-click a line, Shift+click,
Shift+arrows, Ctrl+Shift+arrows by word, Shift+Home/End, Ctrl+A. The
selection stays until you click elsewhere or press Esc; other keys go back
to the app underneath.

**Settings** (tray icon > Settings, or `quickcopy --settings`): start with
Windows, the language translations go into (all of Google Translate's
languages, searchable), the translate shortcut, the app language (English or
Vietnamese) and Vietnamese text recognition. Saved in
`%APPDATA%\QuickCopy\settings.json`. Regenerate the language list with
`python tools/languages.py`.

**Translate** (the shortcut, default Ctrl+T, or the selection menu): the selected
text is translated with Google Translate and painted over the original in
the same boxes, with sampled background and text colors, like Google Lens.
Text already in Vietnamese is translated to English. Ctrl+C copies the
translation; Esc shows the original again. This sends the selected text to
Google, only when you ask. It uses the endpoint and public key of the Google
Translate web client (not an official API; it may change). Set
`QC_GOOGLE_KEY` to use your own key.

- 0.7 MB executable, no GPU. About 3 MB of private memory when idle, and
  110-175 MB while the Vietnamese model is in use: it loads when an image is
  read and unloads after an idle minute.
- Rust + Win32, no UI framework. Runs in the tray (Settings, Pause, Quit);
  starting it again opens the running copy's settings.

## How it works

1. When the cursor rests for 150 ms, UI Automation is asked whether there is
   an image under it. Apps that expose no accessibility tree (Electron apps
   like Zalo) fall back to finding the image rectangle from pixels
   (`src/rectfind.rs`).
2. The image is captured with GDI, upscaled 2x and run through Windows OCR,
   which finds the lines and words. The overlay appears right away, so text
   can be selected while step 3 runs.
3. In Vietnamese mode the lines are re-read by
   [VieSVTR](https://huggingface.co/ngocthua830/viesvtr), because Windows OCR
   has no Vietnamese recognizer (`src/viet.rs`). A copy made before it
   finishes waits for it, so the clipboard always gets the Vietnamese text.
4. A transparent overlay is laid over the text lines only, so clicks elsewhere
   on the image still reach the app underneath.
5. Lines are grouped into blocks of lines stacked under one another: a
   paragraph, a column, a table cell (`src/textmodel.rs`). A click goes to
   the block it is in. A selection inside one block is everything between
   its ends; across blocks it goes row after row within the width of the two
   blocks, so dragging along a table row takes that row and dragging down
   one pane of a window leaves the panes beside it alone.
6. Results are cached by pixel hash. When a window comes to the front (an
   image viewer opening), the image at its center is read ahead of time, so
   it is ready when the cursor gets there.

Measured on a 14-line Vietnamese page: overlay ~370 ms after the cursor
rests on a new image (~200 ms on one already read), Vietnamese text ~500 ms.

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
quickcopy --layout image.png     # lines in reading order, with their blocks
quickcopy --uia X Y              # what UI Automation and IAccessible2 report at a point
quickcopy --drag image.png X0 Y0 X1 Y1   # what a drag between two points selects
```

`tools/selection/` tests selection on many layouts (a two-column article, a
table, forms, a chat, a dashboard, a receipt, code, a three-pane issue
tracker). `gen_layouts.py <dir>` renders them with headless Edge and writes
one case per marked element (drag over it, expect its text);
`run_cases.py <exe> <dir>/cases.json` runs them through `--drag`;
`viz_layout.py` draws the blocks on an image.

Debug builds log to the console. Release builds log to stderr when `QC_LOG`
is set. `QC_ENGLISH=1` forces Windows OCR only. `QC_THEME=light` or `dark`
overrides the Windows app mode (the UI otherwise follows it and the accent
color).

The icons in `assets/` are built from the PNG masters with
`python tools/icons.py`.

`cargo test --release bench_ -- --ignored --nocapture` prints what screen
captures, Direct2D, the OCR engines and translation requests cost in time
and memory on this machine.

## Known limitations

- Images drawn on wallpapers or with edges close to the background color may
  not be found in Electron apps.
- OCR gives a box per word, not per letter: a selection that starts or ends
  inside a word places the caret by dividing the word's box evenly, so it
  can be a letter off in proportional fonts.
- Select all reads block after block. For a table whose columns are far
  apart that is column by column; dragging over its rows reads row by row.
- A single click on a word inside the image is taken by the overlay, so image
  links do not open when clicked exactly on their text.
- Content behind DRM or windows running as administrator cannot be read.
- Videos are recognised in Chromium browsers (tested in Edge, with HTML5
  video and YouTube). A muted video whose picture does not move counts as
  paused. Video players in desktop apps (Media Player, VLC) are not handled.
- Over a paused video, picture text on the same row as one of the player's
  controls or captions is left out with them.
- A video with a still picture that has only just started playing (under a
  second) can count as paused until Windows lists its media session; the
  overlay, still invisible, goes away at the next check (0.3 s).
