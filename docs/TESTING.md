# Testing and debugging QuickCopy

This page lists the tests, the test tools in `tools/`, and the flags and
environment variables that help when something is read wrong. How the
program works is in [ARCHITECTURE.md](ARCHITECTURE.md).

## Unit tests

```powershell
cargo test --release
```

The tests that load the Vietnamese model run only when
`models/viesvtr/viesvtr_v7.onnx` is present.

## Benchmarks

```powershell
cargo test --release bench_ -- --ignored --nocapture
```

This prints what screen captures, Direct2D, the OCR engines and
translation requests cost in time and memory on your machine. The
translation benchmark sends requests to Google.

## Command-line flags

`quickcopy.exe` has these flags for checking what it reads without
hovering. A release build is a Windows GUI program, so PowerShell does not
wait for it or show its output unless you pipe it, for example
`quickcopy --probe X Y | Out-Host`.

```powershell
quickcopy --ocr-file image.png            # OCR an image file, print text and word boxes
quickcopy --probe X Y                     # detect and OCR the image at a screen point
quickcopy --layout image.png              # lines in reading order, with their blocks
quickcopy --uia X Y                       # what UI Automation and IAccessible2 report at a point
quickcopy --drag image.png X0 Y0 X1 Y1    # what a drag between two points selects
quickcopy --settings                      # open the settings window
quickcopy --check-update                  # latest release, whether it is newer, its asset URLs
quickcopy --download-model                # download the VieSVTR model, as Download in the settings does
```

## Environment variables

| Variable | Effect |
| --- | --- |
| `QC_LOG` | Release builds log to stderr when it is set. Debug builds always log to the console. |
| `QC_ENGLISH=1` | Use Windows OCR only, without VieSVTR. |
| `QC_THEME=light` or `dark` | Override the Windows app mode. Otherwise the UI follows it and the accent color. |
| `QC_GOOGLE_KEY` | Use this key for translation instead of the Google Translate web client's key. |
| `QC_CAPTURABLE=1` | Let screenshots see the overlay while it has focus, to check what it draws. |
| `QC_UPDATE_API` | Full URL of a release JSON to check instead of GitHub's (plain http allowed), for testing updates. |
| `QC_INSTANCE` | Name of a separate single-instance mutex, so a test copy runs beside an installed QuickCopy. |

`tools/update/e2e.ps1` runs a real update against a fake release served
from this PC: it installs a newer exe into a temp copy of the app, checks
that a wrong hash installs nothing (`-BadHash`), and that a new version
that does not start is rolled back (`-Fails`).

## OCR corpus (`tools/corpus`)

`tools/corpus/` checks that the text in images is read. It has real
screenshots, documents, photos of print, video frames and Vietnamese text
from the web, plus pages made for the hard cases. Each image has a list of
phrases, written by a person looking at the image. `run_corpus.py` prints
the phrases each image missed, and a total.

```powershell
python tools/corpus/fetch.py                       # download web images not on disk yet
python tools/corpus/gen_synth.py                   # render synth/*.html to web/synth/
python tools/corpus/run_corpus.py target/release/quickcopy.exe            # Vietnamese mode
python tools/corpus/run_corpus.py target/release/quickcopy.exe --english  # Windows OCR only
```

The web images are not in git. `fetch.py` downloads them from the URLs in
the manifests. [tools/corpus/README.md](../tools/corpus/README.md) explains
the manifest format and how to add images.

## Selection cases (`tools/selection`)

`tools/selection/` tests selection on many layouts: a two-column article, a
table, forms, a chat, a dashboard, a receipt, code and a three-pane issue
tracker.

```powershell
python tools/selection/gen_layouts.py out\selection
python tools/selection/run_cases.py target\release\quickcopy.exe out\selection\cases.json
```

`gen_layouts.py` renders the layouts with headless Edge and writes one case
per marked element: drag over the element and expect its text.
`run_cases.py` runs the cases through `--drag`. Add `--english` for Windows
OCR only. `viz_layout.py` draws the blocks on an image.

## Video cases (`tools/video`)

`tools/video/` checks paused and playing videos. It needs `ffmpeg`.

1. Run `make_videos.ps1` to make the test videos in `tools/video/out/`.
2. Run `e2e.ps1` for the browser cases. The comment at the top of the
   script lists the four cases: paused, pausing under a resting cursor,
   playing, and a still picture with sound.
3. Run `desktop.ps1` for a desktop player, such as VLC or Media Player,
   that already shows the test video. The comment at the top of the script
   shows the commands.

## Generated files

- `src/languages.rs`, the list of translation languages:
  `python tools/languages.py`.
- The icons in `assets/`, from the PNG masters: `python tools/icons.py`.
- `models/viesvtr/vi_syllables.bin`, the Vietnamese syllable table:
  `python tools/vietlm/build.py`. Its docstring gives the input files.
- The crate list in `THIRD-PARTY-NOTICES.md`: `python tools/third_party.py`.
