# Changelog

All notable changes to QuickCopy are listed in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and QuickCopy uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.2] - 2026-11-24

### Fixed

- QuickCopy did not start on a Windows without the Visual C++
  Redistributable. The exe now links the C runtime statically.

### Changed

- The zip carries ONNX Runtime 1.32 in `runtime\`, with the Visual C++
  runtime DLLs it needs, so Vietnamese text recognition works on Windows 10
  without copying `onnxruntime.dll` by hand.

## [0.1.1] - 2026-11-24

### Changed

- Starting `quickcopy.exe` opens the settings window, so it is clear that
  QuickCopy runs. The start at sign-in (**Start with Windows**) passes
  `--tray` and stays in the tray; an existing Start with Windows entry is
  updated to it.

## [0.1.0] - 2026-11-24

The first public release.

### Added

- Rest the cursor on an image to select its text. QuickCopy finds images
  with UI Automation, and from pixels in apps that expose no accessibility
  tree, such as Zalo.
- Text recognition with Windows OCR. Lines that Windows OCR skips in
  tightly stacked text are cut out and read again.
- Vietnamese text recognition with the VieSVTR model (CC BY-NC 4.0),
  which QuickCopy downloads from its author's page at the first start and
  checks against a pinned SHA-256 hash. Uncertain tone marks are picked by
  syllable frequencies counted from Vietnamese Wikipedia
  (`models/viesvtr/vi_syllables.bin`).
- Selection like native text: drag, double-click, triple-click,
  Shift+click, Shift+arrows, Ctrl+Shift+arrows, Shift+Home and Shift+End,
  and Ctrl+A. Copy with Ctrl+C, Ctrl+Insert or the selection menu.
- Lines grouped into blocks, so a selection follows columns, table rows and
  window panes.
- Translation of the selection with Google Translate, painted over the
  original with matching colors, size and weight.
- Paused videos read like images on web pages, including X, Facebook and
  Threads, and in Media Player and VLC. Playing videos are left alone.
- Pictures that Photos draws, and pictures that a web page leaves out of
  its accessibility tree, such as Facebook's photo viewer.
- Settings window: start with Windows, translation language and shortcut,
  app language (English or Vietnamese) and Vietnamese text recognition.
- Tray icon with a menu for **Settings…**, **Pause** and **Quit QuickCopy**.
- Light and dark themes that follow Windows and its accent color, and an
  app icon.
- Reading ahead: the image at the center of a window that comes to the
  front is read before the cursor gets there.
- Debug flags `--ocr-file`, `--probe`, `--layout`, `--uia` and `--drag`,
  and `--check-update` and `--download-model`.
- Test tools: an OCR corpus (`tools/corpus`), selection cases
  (`tools/selection`) and video cases (`tools/video`).
- Update checker: once a day (and from **Check now** in the settings) it
  looks for a newer version in GitHub Releases, offers **Update to X.Y.Z**
  in the tray, checks the download's SHA-256 against `SHA256SUMS.txt`,
  installs it and restarts, and puts the old version back if the new one
  does not start.
- Version information and publisher in the exe, and an application
  manifest.
- Release packaging (`scripts/package.ps1`), CI, a tag-driven release
  workflow with build provenance attestations, and winget and Scoop
  manifests.
- The platform-independent code builds and passes its tests on Linux, a
  first step toward a Linux version (`docs/linux.md`).
- Licenses (MIT OR Apache-2.0), third-party notices, and documents for
  users and contributors.

### Changed

- Idle memory went from 89 MB to 3 MB. Direct2D renders in software, and
  the Vietnamese model unloads one minute after its last read.
- The overlay appears as soon as Windows OCR has the words. The Vietnamese
  reading replaces them when it is ready.
- The cursor dwell time went from 300 ms to 150 ms.
- Translation requests share one HTTP/2 connection.
- ONNX Runtime uses at most 8 threads and does not spin between steps.

### Fixed

- Vietnamese text was copied without tone marks after the model was
  unloaded and loaded again.
- A drag in a two-column form selected nothing, or text from the other
  column.
- Translating many lines at once ran the translation across headings and
  paragraphs.
- A second launch added a second overlay and tray icon. It now opens the
  settings of the copy that is running.
- The tray icon disappeared when Explorer restarted.
- A highlight stayed on screen when the page scrolled under a selection.
- A late translation was painted over a newer selection.

[Unreleased]: https://github.com/duckvhuynh/quickcopy/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/duckvhuynh/quickcopy/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/duckvhuynh/quickcopy/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/duckvhuynh/quickcopy/releases/tag/v0.1.0
