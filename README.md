# QuickCopy

Live Text for Windows: rest the cursor on any image on screen and select the
text inside it like normal text, then copy it with Ctrl+C or the right-click
menu. Reads English and Vietnamese. Tested in Edge, Brave and Firefox (also
Facebook's photo viewer), Electron apps such as Zalo, Photos, Media Player
and VLC.

**Videos**: a paused video's frame is read like an image; a playing one is
left alone. In web pages (Edge, Firefox) and desktop players (Media Player,
VLC). A video counts as playing while its picture moves, or while its
media session says so (Windows media sessions: a web video's is matched by
its tab's title, a player's by the app), so a still picture with music is
not read either. Pausing it (a click, the space key, a media key) brings up
the overlay without moving the mouse: a playing video under the resting
cursor is checked twice a second, which costs about 1-2% of one core.
Playing it again hides the overlay within a second. Text the player draws
over the picture (its control bar and time, a play button, captions, a
title) is left out, so the overlay never takes the player's clicks.

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

1. When the cursor rests for 150 ms, UI Automation is asked what is under
   it, which decides what gets read:
   - an image element (web pages, most apps);
   - a web page's `<video>`, which UI Automation shows as a plain group: the
     IAccessible2 attributes say `tag:video` (`src/video.rs`);
   - a picture an app draws in its own control, shown as an unnamed custom
     element, pane or group with nothing inside, at least 100x60 (Photos'
     image, VLC's video), or a child window of an app with a media session
     (Media Player). Named canvases (Paint's: an editor, whose clicks stay
     its own), top-level windows (games) and web page content are left
     alone;
   - in apps that expose no accessibility tree (Electron apps like Zalo),
     the image rectangle found from pixels (`src/rectfind.rs`);
   - on a web page where nothing exposed is under the cursor, a picture the
     page leaves out of the tree (an `<img alt="">`, as in Facebook's photo
     viewer, or a CSS background), also found from pixels. It is read only
     if UI Automation, asked at 16 points across it, finds nothing of the
     page's own there (text, links, images, controls), so a box of real
     text is left alone.
2. The image is captured with GDI, upscaled 2x and run through Windows OCR,
   which finds the lines and words. Windows OCR skips some lines in tightly
   stacked text (an underline just above the next line is enough), so what
   still has letter strokes outside every word found is cut into lines, laid
   out one under another with space around each, and read again
   (`src/missed.rs`). Shapes it reads as letters (a chart's bars as `III`)
   are dropped. The overlay appears right away, so text can be selected
   while step 3 runs.
3. In Vietnamese mode the lines are re-read by
   [VieSVTR](https://huggingface.co/ngocthua830/viesvtr), because Windows OCR
   has no Vietnamese recognizer (`src/viet.rs`). Where it was unsure of a
   tone mark, the mark is picked by how often each syllable and pair of
   syllables occurs in Vietnamese Wikipedia (`src/viet/lm.rs`). A copy made
   before it finishes waits for it, so the clipboard always gets the
   Vietnamese text.
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

`models/viesvtr/vi_syllables.bin` (5.5 MB, in the repository) goes next to
it: syllable and pair frequencies that pick tone marks, loaded and freed
with the model (about 12 MB while loaded). Without it lines are read as the
model reads them. It is counted from Vietnamese Wikipedia (text CC BY-SA
4.0) by `tools/vietlm/build.py`, which says how to rebuild it.

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

`tools/corpus/` checks that the text in about 70 images is read: real
screenshots, documents, photos of print and Vietnamese text from the web
(downloaded by `fetch.py`, not kept in git), and pages made for the hard
cases (`gen_synth.py`). `run_corpus.py <exe>` prints the phrases each image
missed and a total; see its README.

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
- Not every app can be read. Pictures an app draws without telling UI
  Automation anything (a top-level window drawn by hand, as games are), a
  named canvas (editors such as Paint), thumbnails inside list items (Media
  Player's library, File Explorer), DRM-protected video (Netflix, which
  shows black to screen captures) and windows running as administrator are
  not read. Classic Windows Media Player was not tested.
- A muted video whose picture does not move counts as paused.
- Over a paused video, picture text between the buttons of a control bar,
  or under one of the player's controls or captions, is left out with them.
- A video with a still picture that has only just started playing (under a
  second) can count as paused until Windows lists its media session; the
  overlay, still invisible, goes away at the next check (0.3 s).
