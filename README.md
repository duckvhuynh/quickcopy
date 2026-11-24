# QuickCopy

[![CI](https://github.com/duckvhuynh/quickcopy/actions/workflows/ci.yml/badge.svg)](https://github.com/duckvhuynh/quickcopy/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/duckvhuynh/quickcopy)](https://github.com/duckvhuynh/quickcopy/releases/latest)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

Select, copy and translate the text inside any image on your Windows screen.

QuickCopy works like Live Text or Google Lens, on Windows. Rest the cursor on an image, a
screenshot or a paused video, and the text in it becomes selectable like
normal text. Copy it with Ctrl+C or translate it in place. QuickCopy reads
English and Vietnamese, and all text recognition runs on your PC. It is a
1 MB Rust program with a tray icon, no UI framework and no GPU use.

<!-- TODO: add a short GIF of selecting and translating text in an image,
     saved as docs/images/demo.gif, and show it here:
     ![Selecting and translating text in an image](docs/images/demo.gif) -->

## Features

- Reads text in images on web pages, in apps and in Electron apps that
  expose no accessibility tree. Tested in Edge, Brave, Firefox, Facebook's
  photo viewer, Zalo, Photos, Media Player and VLC.
- Reads the frame of a paused video in a browser, in Media Player and in
  VLC. A playing video is left alone.
- Selects text like a text editor does: drag, double-click a word,
  triple-click a line, Shift+click, Shift+arrows, Ctrl+A.
- Keeps columns, tables and panes apart, so a drag down one column selects
  only that column.
- Translates the selection with Google Translate and paints the
  translation over the original, in matching colors, like Google Lens.
- Reads Vietnamese with tone marks through the optional VieSVTR model.
- Follows the Windows light or dark mode and accent color. The app is in
  English or Vietnamese.
- Uses about 3 MB of private memory when idle.
- Checks GitHub Releases for a new version once a day and installs it from
  the tray menu.

## Install

1. Download `QuickCopy-X.Y.Z-windows-x64.zip` from the
   [latest release](https://github.com/duckvhuynh/quickcopy/releases/latest).
2. Unzip it into any folder. QuickCopy needs no installer and no
   administrator rights.
3. Run `quickcopy.exe`. Its icon appears in the notification area.

To start QuickCopy when you sign in, turn on **Start with Windows** in its
settings.

### Verify the download

Each release has a `SHA256SUMS.txt` file. In PowerShell, in the folder that
holds the download, run this to compare the zip's hash with that file. It
prints `True` when they match:

```powershell
$zip = "QuickCopy-X.Y.Z-windows-x64.zip"
$want = (Select-String -Path SHA256SUMS.txt -SimpleMatch $zip).Line.Split(" ")[0]
(Get-FileHash $zip -Algorithm SHA256).Hash -eq $want
```

Each release file also has a build provenance attestation, made by the
GitHub Actions workflow that built it. With the [GitHub CLI](https://cli.github.com/),
run this to check that the file was built from this repository:

```powershell
gh attestation verify QuickCopy-X.Y.Z-windows-x64.zip --repo duckvhuynh/quickcopy
```

A release also has the executable on its own,
`quickcopy-X.Y.Z-windows-x64.exe`. Verify it the same way. How releases
are built is in [docs/release.md](docs/release.md).

### Windows SmartScreen

Release builds are not code-signed. The first time you run
`quickcopy.exe`, Windows SmartScreen can show "Windows protected your PC".
Verify the download first. Then select **More info** and **Run anyway**.

### Vietnamese text recognition

Windows OCR has no Vietnamese recognizer. To read Vietnamese with its tone
marks, QuickCopy uses the [VieSVTR](https://huggingface.co/ngocthua830/viesvtr)
model by ngocthua830. The model is not in the release zip, because its
license is **CC BY-NC 4.0, which allows non-commercial use only**.

The first time QuickCopy runs, it downloads the model (`viesvtr_v7.onnx`,
30.7 MB) from that page, checks its SHA-256 hash and saves it to
`%LOCALAPPDATA%\QuickCopy\models\viesvtr\`. The settings show the
download's progress in percent. Without an internet connection it tries
again at the next start, or when you select **Retry** in the settings.
To stop using the model, turn off **Vietnamese text recognition** in the
settings, then delete the file; QuickCopy does not download it again while
the setting is off.

On Windows 10, also put `onnxruntime.dll` version 1.17 or newer next to
`quickcopy.exe`. Windows 11 already has a copy in `System32`.

To install the model by hand instead, save `weights/viesvtr_v7.onnx` from
the model's page as `models\viesvtr\viesvtr_v7.onnx` in that
`%LOCALAPPDATA%\QuickCopy` folder or in the folder that holds
`quickcopy.exe`. The release zip already has `vi_syllables.bin` in
`models\viesvtr`; without it, tone marks are read as the model reads them.

## Usage

### Select and copy

Rest the cursor on an image for a moment. QuickCopy reads it and lays a
transparent overlay over its text lines. Clicks elsewhere on the image
still go to the app underneath.

- Drag over text to select it. The drag can start a little before or after
  a line.
- Double-click a word or triple-click a line.
- Extend the selection with Shift+click, Shift+arrows, Ctrl+Shift+arrows,
  Shift+Home and Shift+End. Select all with Ctrl+A.
- Copy with Ctrl+C, Ctrl+Insert or the menu that appears next to the
  selection.

The selection stays until you click elsewhere or press Esc. Other keys go
to the app underneath.

### Translate

Select text and press the translate shortcut (Ctrl+T by default), or pick
**Translate** in the selection menu. The translation is painted over the
original in the same boxes, with the background and text colors sampled
from the image. Text already in the target language is translated to
English, or to Vietnamese when English is the target. Ctrl+C copies the translation. Esc shows the original again.

### Videos

Pause a video and rest the cursor on it. The paused frame is read like an
image. This works on web pages and in Media Player and VLC. A video counts
as playing while its picture moves or while its Windows media session says
it is playing, so a still picture with music is not read.

You do not need to move the mouse after you pause. While the cursor rests on
a playing video, QuickCopy checks it twice a second, which costs about 1-2%
of one CPU core. The overlay appears after a pause and goes away within a
second when the video plays again. Text that the player draws over the
picture, such as its control bar, time and captions, is left out, so the
overlay never takes the player's clicks.

### Settings

Open the settings from the tray icon (**Settings…**), by double-clicking the
tray icon, or with `quickcopy --settings`. Starting `quickcopy.exe` again
also opens the settings of the copy that is running.

- **Start with Windows**.
- **Translate into**: any of Google Translate's languages, with search.
- **Translate shortcut**: Ctrl+T by default.
- **App language**: English or Vietnamese.
- **Vietnamese text recognition**: on or off, once the VieSVTR model is
  downloaded; until then, the download's progress.
- **Check for updates automatically**: the daily update check.
- **Version**: the running version, and a button that checks for updates
  and shows the result, or offers **Update to X.Y.Z**.

Settings are saved in `%APPDATA%\QuickCopy\settings.json`. The Start with
Windows setting is stored only in the `HKCU` Run key.

### Tray icon

Right-click the tray icon for **Settings…**, **Pause** and
**Quit QuickCopy**. While QuickCopy is paused, it reads nothing.

### Updates

QuickCopy checks GitHub Releases for a newer version once a day. When one
exists, the tray menu shows **Update to X.Y.Z**. Select it to download the
new version. QuickCopy checks the download's SHA-256 hash before it
installs it, swaps the exe and restarts; if the new version does not
start, the old one is put back. Turn off **Check for updates
automatically** in the settings to stop the daily check; **Check now** in
the settings checks at once. The update replaces `quickcopy.exe` only, so
keep QuickCopy in a folder you can write to (for example under
`%LOCALAPPDATA%`), not in Program Files. `quickcopy --check-update`
prints what the check finds without installing anything.

## Privacy

- All text recognition runs on your PC, with Windows OCR and, for
  Vietnamese, the VieSVTR model. Images are never uploaded.
- Translation sends the selected text to Google, and only when you ask for
  a translation. QuickCopy uses the endpoint and public key of the Google
  Translate web client. That is not an official Google API, and Google can
  change or block it. Set the `QC_GOOGLE_KEY` environment variable to use
  your own key.
- The update check contacts `api.github.com` once a day. Turn off
  **Check for updates automatically** to stop it.
- The first start contacts `huggingface.co` once, to download the
  Vietnamese model.
- QuickCopy sends nothing else and collects no usage data.

## Requirements

- Windows 10 or Windows 11, x64.
- A Windows OCR language. QuickCopy uses the English (United States) OCR
  language if it is installed, otherwise the OCR languages of your Windows
  display languages. Adding English (United States) in **Settings** >
  **Time & language** > **Language & region** installs it. To check which
  OCR languages are installed, run this in PowerShell:

  ```powershell
  Get-WindowsCapability -Online -Name "Language.OCR*" | Where-Object State -eq Installed
  ```

- For Vietnamese only: the VieSVTR model and ONNX Runtime 1.17 or newer.
  Windows 11 ships ONNX Runtime in `System32`. On Windows 10, put
  `onnxruntime.dll` next to `quickcopy.exe`. See
  [Vietnamese text recognition](#vietnamese-text-recognition).

## Build from source

You need Rust 1.92 or newer with the `x86_64-pc-windows-msvc` toolchain,
and the Visual Studio Build Tools with a Windows SDK. `build.rs` compiles
the icon resources with the SDK's resource compiler.

```powershell
git clone https://github.com/duckvhuynh/quickcopy
cd quickcopy
cargo build --release
```

The executable is `target\release\quickcopy.exe`. It finds the
`models\viesvtr` folder of the repository, because it looks in the parent
folders of the executable. Run the unit tests with `cargo test --release`.

## How it works

When the cursor rests for 150 ms, QuickCopy asks UI Automation what is
under it. Where UI Automation reports no image, as in Electron apps, it
finds the image from pixels. It captures the image, upscales it 2x and
reads it with Windows OCR. Lines that Windows OCR skips are cut out and
read again. In Vietnamese mode, VieSVTR reads the lines again, and a table
of syllable frequencies picks uncertain tone marks. A transparent overlay
covers only the text lines. Lines are grouped into blocks, so a selection
follows columns and table rows. Results are cached by pixel hash.

On a 14-line Vietnamese page, the overlay appears about 370 ms after the
cursor rests on a new image (about 200 ms on an image already read), and
the Vietnamese text is ready after about 500 ms.

The details, with the source file for each step, are in
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). The test tools and debug
flags are in [docs/TESTING.md](docs/TESTING.md).

## Known limitations

- Not every app can be read. QuickCopy cannot read pictures that an app
  draws without telling UI Automation (top-level windows drawn by hand, as
  in games), named canvases in editors such as Paint, thumbnails inside
  list items (Media Player's library, File Explorer), DRM-protected video
  (Netflix shows black to screen captures), and windows that run as
  administrator. Classic Windows Media Player was not tested.
- In Electron apps, images on wallpapers or with edges close to the
  background color may not be found.
- OCR gives a box per word, not per letter. A selection that starts or ends
  inside a word divides the word's box evenly, so the caret can be a letter
  off in proportional fonts.
- Select all reads block after block. For a table whose columns are far
  apart, that is column by column. A drag over its rows reads row by row.
- A single click on a word inside an image goes to the overlay, so an image
  link does not open when you click exactly on its text.
- A muted video whose picture does not move counts as paused.
- Over a paused video, picture text between the buttons of a control bar,
  or under the player's controls or captions, is left out with them.
- A video with a still picture that started playing less than a second ago
  can count as paused until Windows lists its media session. The overlay,
  still invisible, goes away at the next check, 0.3 s later.

## Roadmap

- Linux support. The plan is in [docs/linux.md](docs/linux.md).

## Contributing

Bug reports, test images and pull requests are welcome. Read
[CONTRIBUTING.md](CONTRIBUTING.md) for how to build, test and send a
change. Report security problems privately, as described in
[SECURITY.md](SECURITY.md). Everyone taking part follows the
[code of conduct](CODE_OF_CONDUCT.md).

## License

QuickCopy's code is licensed under either of these, at your option:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in QuickCopy by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.

These parts have their own licenses:

- The VieSVTR model is CC BY-NC 4.0. It is not in this repository or in
  releases; QuickCopy downloads it from its author's page.
- `models/viesvtr/vi_syllables.bin` is counted from Vietnamese Wikipedia
  and is licensed CC BY-SA 4.0.

[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md) lists these and every Rust
crate QuickCopy uses, with their licenses.
