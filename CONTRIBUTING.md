# Contributing to QuickCopy

Thank you for helping. You can help by reporting a bug, by adding test
images to the OCR corpus, or by sending a pull request.

Everyone taking part follows the [code of conduct](CODE_OF_CONDUCT.md).
Report security problems privately, as described in
[SECURITY.md](SECURITY.md), not in a public issue.

## Report a bug

Open an issue with the **Bug report** form. Say which app or web page the
image was in, what QuickCopy read or selected, and what you expected. A
screenshot of the image helps most. Remove anything private from it first.

To show what QuickCopy sees, run one of these and paste the output:

```powershell
quickcopy --probe X Y | Out-Host    # detect and OCR the image at screen point X, Y
quickcopy --uia X Y | Out-Host      # what UI Automation reports at that point
```

## Build

You need Windows 10 or 11 (x64), Rust 1.92 or newer with the
`x86_64-pc-windows-msvc` toolchain, and the Visual Studio Build Tools with
a Windows SDK.

```powershell
cargo build --release
```

The executable is `target\release\quickcopy.exe`. To work on Vietnamese
text recognition, put the VieSVTR model at
`models\viesvtr\viesvtr_v7.onnx`, as the
[README](README.md#vietnamese-text-recognition-optional) describes.

## Test

Run the unit tests:

```powershell
cargo test --release
```

For a change to what QuickCopy reads, run the OCR corpus before and after
the change, and compare the totals:

```powershell
python tools/corpus/fetch.py
python tools/corpus/gen_synth.py
python tools/corpus/run_corpus.py target/release/quickcopy.exe
python tools/corpus/run_corpus.py target/release/quickcopy.exe --english
```

For a change to selection, run the selection cases:

```powershell
python tools/selection/gen_layouts.py out\selection
python tools/selection/run_cases.py target\release\quickcopy.exe out\selection\cases.json
```

The tools need Python 3 and Microsoft Edge. [docs/TESTING.md](docs/TESTING.md)
lists every test tool, debug flag and environment variable.

## Add test images

[tools/corpus/README.md](tools/corpus/README.md) says how to add an image.
The rules that matter most:

- Commit the image's source URL, download URL and license, never the
  image. Prefer freely licensed images, such as those on Wikimedia
  Commons.
- Write the expected phrases by looking at the image, never from
  QuickCopy's output.
- A missed phrase is a finding about QuickCopy. Do not change the phrase
  unless the phrase itself is wrong.

## Code style

Follow the style of the code around your change.

- A comment says why the code does something, or what it found out, not
  what the next line does. For example: "Lines per model run. Measured on
  a 14-line page: 1 line 320 ms, 4 lines 285 ms, 16 lines 350 ms."
- A claim about speed, memory or accuracy comes with the number you
  measured and how you measured it.
- Keep dependencies few. QuickCopy uses Win32 through the `windows` crate,
  with no UI framework.
- The repository has no rustfmt configuration, and `cargo fmt` would
  rewrite most files. Do not reformat code that your change does not
  touch.

## Commit messages and pull requests

Look at `git log` for examples.

- Write the subject as a short sentence about what changes for the user,
  such as "Read paused videos in the browser; leave playing ones alone".
- In the body, describe the problem, its cause, and the fix.
- Give the measurements before and after the change: corpus totals,
  selection cases passed, unit tests passed, time or memory.

Open a pull request against `main` and fill in the template. Keep one
change per pull request.

## License

QuickCopy is licensed under MIT OR Apache-2.0. Unless you explicitly state
otherwise, any contribution intentionally submitted for inclusion in
QuickCopy by you, as defined in the Apache-2.0 license, shall be dual
licensed as above, without any additional terms or conditions.
