# Porting QuickCopy to Linux

QuickCopy runs on Windows only. On Linux the crate builds and its platform-independent tests pass, but `quickcopy` prints that the Linux version is not available yet and exits with code 1. This page says what already builds on Linux, what each Windows dependency becomes, and the order to do the work in.

## What builds on Linux today

The windows crates are in a `[target.'cfg(windows)'.dependencies]` table, and `build.rs` compiles `assets/quickcopy.rc` only for Windows targets. These modules build on every platform:

| Module | What it does |
| --- | --- |
| `common` | `RECT`, `POINT` and virtual-key codes, plus `log`. On Windows these are the Win32 types themselves. Elsewhere they are stand-ins with the same fields and values. |
| `textmodel` | Characters, lines, blocks, hit testing and selection. `Word` lives here. |
| `layout` | Fits a translation into the boxes of the original text. |
| `rectfind` | Finds an image's bounds from pixels alone. |
| `missed` | Finds text lines that OCR skipped. |
| `style` | Measures x-height, baseline and stroke width. |
| `translate` | Request building, grouping, color sampling and the cache. Only `spawn`, `run` and the WinHTTP transport are Windows-only. Off Windows, `https_get` returns an error. |
| `settings` | The settings data and file. The file is `$XDG_CONFIG_HOME/quickcopy/settings.json`, or `~/.config/quickcopy/settings.json`. Shortcuts stay stored as Windows virtual-key codes on every platform. |
| `viet`, `viet/lm` | VieSVTR line recognition with `ort` (`load-dynamic`) and the tone-mark table. |
| `i18n`, `languages` | UI strings and the language lists. |

The Win32 modules start with `#![cfg(windows)]`: `worker`, `video`, `paint`, `popover`, `menu`, `theme` and `settings_ui`. In `main.rs`, the `windows_only!` macro puts `#[cfg(windows)]` on every item of the Windows app. A new Windows-only module needs `#![cfg(windows)]` at its top, or the Linux build breaks.

To check the Linux build from Windows, run:

	rustup target add x86_64-unknown-linux-gnu
	cargo check --target x86_64-unknown-linux-gnu --tests

To run the tests on Linux, run `cargo test` there. At the commit that added this page, Linux runs 55 tests (1 ignored) and Windows runs 73 (4 ignored).

## What each Windows dependency becomes

Effort is in working days for one developer who knows the code. All estimates are guesses.

| Piece | Windows today | Linux replacement | Effort |
| --- | --- | --- | --- |
| Screen capture | GDI `BitBlt` in `worker::capture`. `WDA_EXCLUDEFROMCAPTURE` keeps the overlay out of captures. | X11: `XShmGetImage` on the root window through `x11rb`. X11 has no capture exclusion, so capture the window under the cursor with XComposite `NameWindowPixmap`, which leaves the overlay out. Wayland: the `xdg-desktop-portal` ScreenCast portal with a PipeWire stream (`ashpd`, `pipewire`). The user approves once, and a restore token keeps the approval. | X11 2, Wayland 4 |
| Cursor position | `GetCursorPos` every 20 ms, `WindowFromPoint` | X11: `XQueryPointer` on the root window. Wayland gives no client the global pointer position. A ScreenCast stream with cursor mode `metadata` reports the pointer in each frame, which can stand in (to verify). | X11 0.5 |
| Foreground change | `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)` | X11: `PropertyNotify` on `_NET_ACTIVE_WINDOW`. AT-SPI2 `window:activate` events also work on Wayland. | 0.5 |
| Finding images | UI Automation `ElementFromPoint`, IAccessible2 in Chromium, then `rectfind` | AT-SPI2 through the `atspi` crate. Use `Component.GetAccessibleAtPoint` with screen coordinates and roles `image`, `canvas` and `video`. Chromium, Electron and Firefox build their trees only after `org.a11y.Status.IsEnabled` is set. On Wayland, apps do not know their screen position, so AT-SPI screen coordinates are wrong there. Use `rectfind` alone on Wayland. | 4 to 6 |
| OCR | `Windows.Media.Ocr` finds and reads lines. VieSVTR then reads Vietnamese lines again. | Linux has no system OCR. Option A is Tesseract (`libtesseract`, `vie` and `eng` data). It is a system dependency, slower, and weaker on colored screenshot backgrounds. Option B is a PaddleOCR detector and recognizer as ONNX models through `ort`, which the app already loads. The detector gives the line boxes that `viet` needs today from Windows OCR. The CTC spans in `viet::RecChar` give word boxes. Choose option B, and measure it with `tools/corpus`. Use Tesseract only for a first end-to-end prototype. | B 5 to 8, A 1 to 2 |
| ONNX Runtime | `onnxruntime.dll` next to the exe or in System32 (`viet::runtime_path`) | Ship `libonnxruntime.so` next to the binary, and add a Linux branch to `runtime_path`. | 0.5 |
| Overlay | Layered, topmost, no-activate popup with `UpdateLayeredWindow` | X11: an override-redirect window with a 32-bit ARGB visual. Transparency needs a compositing window manager. wlroots compositors and KDE: a `wlr-layer-shell` surface. GNOME has no layer-shell and does not let a client place a window at screen coordinates. On GNOME, use the snapshot mode in phase 5. | X11 3 to 4, layer-shell 3 |
| Input to the app below | `SendInput` passes the wheel and right-click through | X11: XTest (`XTestFakeButtonEvent`). Wayland has no input injection for clients. The RemoteDesktop portal can inject input but asks the user for permission. | 1 |
| Drawing | Direct2D and DirectWrite in `paint`, `popover`, `menu` and `settings_ui` | `tiny-skia` for shapes and `cosmic-text` for shaped text, so Vietnamese marks and Japanese text render correctly. Both draw into a BGRA buffer, the same shape that `UpdateLayeredWindow` takes today. | 5 to 8 |
| Tray | `Shell_NotifyIconW` and the menu that `menu.rs` draws | StatusNotifierItem over D-Bus with `ksni`. The menu becomes a native dbusmenu, so `menu.rs` is not ported. GNOME shows tray items only with the AppIndicator extension, which Ubuntu ships. | 1 to 2 |
| Clipboard | `OpenClipboard`, `SetClipboardData(CF_UNICODETEXT)` | `arboard`. On X11 the app must keep running to serve the CLIPBOARD selection, which a tray app does. On Wayland, writing needs keyboard focus, which the overlay has after a selection. | 0.5 |
| HTTPS | WinHTTP in `translate::https_get` | `ureq` with `rustls`. Keep one agent for the life of the process, as `connection` does today. | 0.5 |
| Theme | Registry `AppsUseLightTheme` and `AccentPalette` | The Settings portal keys `org.freedesktop.appearance` `color-scheme` and `accent-color` | 0.5 |
| DPI | `GetDpiForMonitor` | X11: RandR and `Xft.dpi`. Wayland: `wl_output` scale and `wp_fractional_scale_v1`. | 1 |
| Media state | `GlobalSystemMediaTransportControlsSessionManager` in `video` | MPRIS over D-Bus (`PlaybackStatus` of each `org.mpris.MediaPlayer2.*` name) | 2 to 3 |
| Start at sign-in | The `Run` registry value | A `.desktop` file in `~/.config/autostart` | 0.5 |
| Single instance | `CreateMutexW` | `flock` on a file in `$XDG_RUNTIME_DIR`, or a D-Bus name | 0.5 |
| Settings window | `settings_ui`, a Win32 window drawn with Direct2D | A `winit` window drawn with `softbuffer` and the drawing stack above | 3 to 4 |
| Packaging | `.exe` with embedded resources | AppImage or `.deb` with `libonnxruntime.so` and the models | 1 to 2 |

The X11 version is about 30 to 40 days. Wayland adds about 10. Both numbers are guesses.

## Order the work so each phase can be checked

1. **Seams and headless OCR.** Add Linux modules beside the Windows ones, with the same free functions: `capture`, the OCR entry point, `https_get`, clipboard, and `runtime_path`. Do not add a platform trait. Replace the `WM_APP_*` messages with a channel to the event loop, and move `OcrData` next to `Word` in `textmodel`. Port the `--ocr-file` and `--layout` modes first. They need no display, so CI can run them against `tools/corpus`.
2. **X11 MVP.** Capture, cursor polling, the overlay, selection, copy, the tray, and translation drawing. Find images with `rectfind` alone. This phase runs on Xorg sessions only. Under XWayland, the root window shows native Wayland windows as black, so the X11 build does not help on Wayland sessions.
3. **AT-SPI2.** Find images and videos through the accessibility tree, and read media state from MPRIS.
4. **Settings window.** Port it after the drawing stack has been in use for two phases.
5. **Wayland snapshot mode.** Hovering needs the global pointer and an overlay at screen coordinates, and Wayland gives neither. Instead, a shortcut from the GlobalShortcuts portal captures the screen through the Screenshot or ScreenCast portal. The app then shows a full-screen surface with the frozen image, and the user selects text on it. Use a layer-shell surface where the compositor has one, and a full-screen window on GNOME.

## Risks

- **OCR quality.** Windows OCR is the baseline that users know. An ONNX detector and recognizer must be measured on `tools/corpus` before phase 2 starts.
- **Accessibility cost.** Setting `org.a11y.Status.IsEnabled` makes every GTK, Qt and Chromium app build its tree. That slows some apps, as a screen reader does.
- **GNOME.** GNOME has no layer-shell, no tray without an extension, and no global pointer. On GNOME Wayland, only phase 5 works.
- **The `viet` reload test.** On a Linux machine that has the model file but no `libonnxruntime.so`, `viet::reload_tests` fails until `runtime_path` has its Linux branch.
