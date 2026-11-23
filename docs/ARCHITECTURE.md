# How QuickCopy works

QuickCopy is one Rust program that uses Win32 directly, with no UI
framework. It runs in the notification area. A worker thread finds and
reads images. The UI thread owns the overlay, the selection menu, the tray
menu and the settings window, all drawn with Direct2D and DirectWrite.

This page follows what happens from the moment the cursor stops on an
image to the moment the text can be selected. For the test tools and debug
flags, see [TESTING.md](TESTING.md).

## 1. Find the image under the cursor

When the cursor rests for 150 ms, QuickCopy asks UI Automation what is
under it. The answer decides what gets read:

- An image element. Web pages and most apps report images this way.
- A web page's `<video>`. UI Automation shows it as a plain group, but its
  IAccessible2 attributes say `tag:video` (`src/video.rs`). Some sites, such
  as X, Facebook and Threads, lay a transparent layer beside the `<video>`
  instead of inside it. So QuickCopy also looks for the video in the other
  branches of the ancestors of the element at the point, up to 120
  elements.
- A picture that an app draws in its own control. UI Automation shows it as
  an unnamed custom element, pane or group with nothing inside, at least
  100x60 pixels. Photos' image and VLC's video are read this way. A child
  window of an app with a media session counts too, as in Media Player.
  Named canvases (Paint's, an editor whose clicks stay its own), top-level
  windows (games) and web page content are left alone.
- In apps that expose no accessibility tree, such as Electron apps like
  Zalo, the image rectangle is found from pixels (`src/rectfind.rs`). The
  search accepts a background that changes gradually as well as a flat one.
- On a web page where nothing exposed is under the cursor, a picture that
  the page leaves out of the tree is found from pixels too. Examples are an
  `<img alt="">`, as in Facebook's photo viewer, and a CSS background. It
  is read only if UI Automation, asked at 16 points across it, finds
  nothing of the page's own there (text, links, images, controls). That
  check leaves a box of real text alone.

A Chromium or Electron window gets up to 1 s on first contact to build its
accessibility tree. If nothing is found under a resting cursor, QuickCopy
asks again up to twice, 800 ms apart.

## 2. Read it with Windows OCR

The image is captured with GDI, upscaled 2x and run through Windows OCR,
which finds the lines and words. QuickCopy uses the English (United
States) OCR language if it is installed, otherwise the user's profile
languages.

Windows OCR skips some lines in tightly stacked text. An underline just
above the next line is enough. So whatever still has letter strokes outside
every word found is cut into lines. Those lines are laid out one under
another with space around each and read again (`src/missed.rs`). Shapes
that Windows OCR reads as letters, such as a chart's bars read as `III`,
are dropped.

The overlay appears as soon as Windows OCR has the words, so text can be
selected while step 3 runs.

## 3. Read Vietnamese with VieSVTR

Windows OCR has no Vietnamese recognizer. In Vietnamese mode, each line is
read again by [VieSVTR](https://huggingface.co/ngocthua830/viesvtr), an
ONNX model run through ONNX Runtime (`src/viet.rs`). Lines go to the model
4 at a time. ONNX Runtime does not spin its threads between steps and uses
at most 8 threads.

Where the model was unsure of a tone mark, the mark is picked by how often
each syllable and each pair of syllables occurs in Vietnamese Wikipedia
(`src/viet/lm.rs`). The table is `models/viesvtr/vi_syllables.bin`, built
by `tools/vietlm/build.py`.

A copy made before the Vietnamese reading finishes waits for it, so the
clipboard always gets the Vietnamese text. The selection is found again by
position when the words are swapped in.

The model loads on first use, on a second thread while Windows OCR reads
the image. It unloads one minute after its last read. The ONNX Runtime
environment is made once and kept, because ONNX Runtime allows one
environment per process.

## 4. Lay the overlay over the text

A transparent layered window covers the text lines only. Clicks elsewhere
on the image still reach the app underneath. Around each paragraph, the
overlay takes the mouse a little beyond the text, so a drag that starts
just before a line or between lines selects text instead of dragging the
image. Over a video this margin is smaller.

The overlay takes keyboard focus only on a click in it, never on hover. It
gives focus back on Esc, on a key it does not handle (which is replayed to
the app), or when it hides.

## 5. Group lines into blocks

Lines are grouped into blocks of lines stacked under one another: a
paragraph, a column, a table cell (`src/textmodel.rs`). A click goes to the
block it is in. A selection inside one block is everything between its
ends. Across blocks, it goes row after row within the width of the two
blocks. So a drag along a table row takes that row, and a drag down one
pane of a window leaves the panes beside it alone.

Windows OCR reports some lines as several pieces, for example around inline
code chips. Pieces on one row a few characters apart are joined.

## 6. Cache and read ahead

Results are cached by pixel hash. When a window comes to the front, such as
an image viewer that opens, the image at its center is read ahead of time,
so it is ready when the cursor gets there.

While the overlay is shown, QuickCopy checks every 300 ms that the image is
still there. It compares a 16x16 grid of mean colors and hides the overlay
when more than 15% of the cells change. Hover buttons that a browser draws
on an image do not hide it.

## Videos

A video counts as playing while its picture moves, or while its Windows
media session says so. A web video's session is matched by its tab's
title, a desktop player's by the app. A still picture with music is
therefore not read.

While the cursor rests on a playing video, the video is checked twice a
second, which costs about 1-2% of one core. Pausing it with a click, the
space key or a media key brings up the overlay without moving the mouse.
Playing it again hides the overlay within a second.

Text that the player draws over the picture is left out: its control bar
and time, a play button, captions, a title. QuickCopy drops OCR lines
between the ends of a control bar and words mostly under any other player
control, so the overlay never takes the player's clicks.

## Translation

The translate shortcut or the selection menu sends the selected lines to
Google Translate (`src/translate.rs`). It uses the endpoint and public key
of the Google Translate web client, over WinHTTP. The key can be replaced
with `QC_GOOGLE_KEY`. Requests share one HTTP/2 connection, and the cache
holds 256 entries.

The selected lines are grouped into units (`translate::group`). A new unit
starts where a sentence ends at a line's end, where the text color, size
or weight changes, or at a gap clearly wider than the usual spacing. Each
unit is translated and laid out in its own lines (`src/layout.rs`).

`src/style.rs` measures the original text in pixels: body height,
baseline and stroke width. `paint::match_style` picks the Segoe UI weight
and size that match it. The translation is drawn on the original
baselines, over the background and text colors sampled around each line
(`src/paint.rs`).

## Memory and speed

Measured on a 14-line Vietnamese page: the overlay appears about 370 ms
after the cursor rests on a new image (about 200 ms on an image already
read), and the Vietnamese text is ready after about 500 ms.

The executable is 0.8 MB and uses no GPU. Direct2D renders in software,
because a hardware render target held about 31 MB for the life of the
process and drew these small surfaces slower. QuickCopy uses about 3 MB of
private memory when idle, and 110-175 MB while the Vietnamese model is
loaded. The syllable table adds about 12 MB while loaded.

Run the benchmarks with
`cargo test --release bench_ -- --ignored --nocapture`. See
[TESTING.md](TESTING.md#benchmarks).

## Source map

| File | What it does |
| --- | --- |
| `src/main.rs` | Entry point, command-line flags, overlay window, input |
| `src/worker.rs` | Finding images, capture, Windows OCR, cache |
| `src/video.rs` | Videos in web pages and desktop players: finding them, playing or paused, the player's own text |
| `src/rectfind.rs` | Finding an image rectangle from pixels |
| `src/missed.rs` | Reading lines that Windows OCR skipped |
| `src/viet.rs`, `src/viet/lm.rs` | VieSVTR and tone-mark picking |
| `src/textmodel.rs` | Blocks, carets and selection |
| `src/translate.rs`, `src/layout.rs`, `src/style.rs` | Translation and its layout |
| `src/paint.rs`, `src/theme.rs` | Direct2D drawing and colors |
| `src/popover.rs`, `src/menu.rs` | Selection menu and tray menu |
| `src/settings.rs`, `src/settings_ui.rs` | Settings file and window |
| `src/i18n.rs`, `src/languages.rs` | App strings and the language list |
