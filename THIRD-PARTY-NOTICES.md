# Third-party notices

QuickCopy's own code is licensed under MIT OR Apache-2.0 (see
[LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE)). This file
lists what QuickCopy uses that other people made, under which license, and
whether it is in this repository or in a release.

## VieSVTR model (not distributed)

- Model: VieSVTR, <https://huggingface.co/ngocthua830/viesvtr>, by
  ngocthua830 on Hugging Face.
- File used: `weights/viesvtr_v7.onnx`.
- License: Creative Commons Attribution-NonCommercial 4.0 International
  (CC BY-NC 4.0), <https://creativecommons.org/licenses/by-nc/4.0/>.

The model is not in this repository and not in any QuickCopy release. Users
get it from the page above: QuickCopy downloads the file from there at its
first start, checks its SHA-256 hash, and names the model and its license
in the README of the release zip. Turning off Vietnamese text recognition
stops the download. CC BY-NC 4.0 does not allow commercial use, so
a commercial use of QuickCopy's Vietnamese mode needs the model author's
permission. QuickCopy works without the model, with Windows OCR only.

`src/vocab.txt`, which is in this repository, lists the 229 characters the
model outputs, in the model's class order. It is taken from the VieSVTR
page and is the same character set that VietOCR
(<https://github.com/pbcquoc/vietocr>) uses. It holds only the Vietnamese
letters, digits and punctuation, and QuickCopy needs it to turn the
model's output into text.

## Vietnamese syllable table (in this repository)

- File: `models/viesvtr/vi_syllables.bin`.
- Built from: Vietnamese Wikipedia, as published in the `wikimedia/wikipedia`
  dataset, configuration `20231101.vi`
  (<https://huggingface.co/datasets/wikimedia/wikipedia>).
- Source license: the text of Wikipedia is licensed under the Creative
  Commons Attribution-ShareAlike 4.0 International license (CC BY-SA 4.0,
  <https://creativecommons.org/licenses/by-sa/4.0/>) and the GNU Free
  Documentation License.
- Attribution: contains information counted from Vietnamese Wikipedia
  (<https://vi.wikipedia.org/>), by its contributors.
- License of the table: CC BY-SA 4.0.

The table holds no Wikipedia text. It holds how often each syllable, and
each pair of syllables, occurs. `tools/vietlm/build.py` builds it. That
script lowercases the text, cuts it into syllables made of VieSVTR's
letters, keeps syllables seen at least 20 times and pairs seen at least 3
times, and stores each as a rounded cost. Its docstring gives the exact
file format and the command to rebuild it. The table is published under
CC BY-SA 4.0 because it is derived from text under that license. It is not
covered by QuickCopy's MIT OR Apache-2.0 license.

## ONNX Runtime (in the release zip)

- Project: ONNX Runtime, <https://onnxruntime.ai>, by Microsoft.
- File: `runtime\onnxruntime.dll` 1.32.0, from Microsoft's NuGet package
  `Microsoft.ML.OnnxRuntime`, signed by Microsoft.
- License: MIT. The zip has its license as
  `runtime\onnxruntime-LICENSE.txt` and its third-party notices as
  `runtime\onnxruntime-ThirdPartyNotices.txt`.

QuickCopy loads `onnxruntime.dll` at runtime, only for Vietnamese text
recognition: the copy in `runtime\`, else one next to `quickcopy.exe`, else
the copy Windows 11 has in `System32`.

## Microsoft Visual C++ runtime (in the release zip)

`runtime\` also has `msvcp140.dll`, `msvcp140_1.dll`, `vcruntime140.dll`
and `vcruntime140_1.dll`, which `onnxruntime.dll` needs. They are
redistributable files of Microsoft Visual Studio, shipped with QuickCopy
under the Visual Studio license terms for redistribution.
`quickcopy.exe` links the C runtime statically.

## Windows components

QuickCopy calls Windows OCR (`Windows.Media.Ocr`), UI Automation, Direct2D,
DirectWrite, WinHTTP and other Windows APIs. They are part of Windows and
are used under the Windows license. QuickCopy does not ship any of them.
Translations are drawn with the Segoe UI font that Windows provides.

## Google Translate

Translation sends the selected text to `translate-pa.googleapis.com`, the
endpoint the Google Translate web client calls, with that client's public
key. This is not an official Google API, and Google's terms apply to its
use. Set `QC_GOOGLE_KEY` to use your own key. See the Privacy section of
the [README](README.md#privacy).

## OCR test corpus (`tools/corpus`)

The test images are not in this repository. The manifests in
`tools/corpus/manifests/` record, for each image, the page it is shown on
(`source`), its download URL (`download`) and its license (`license`), as
found on that page. `tools/corpus/fetch.py` downloads the images from their
authors' sites into `tools/corpus/web/`, which git ignores. The pages in
`tools/corpus/synth/` were made for this project and are licensed with the
code. See [tools/corpus/README.md](tools/corpus/README.md).

The manifests also hold short phrases transcribed from each image, which
the tests look for. The phrases of `manifests/vi-web.json` are the visible
text lines of the public web pages listed in `tools/corpus/web_pages.json`.
Those pages are copyright their publishers. The screenshots themselves are
not redistributed.

## Rust crates

These crates are compiled into QuickCopy or used to build it, with the
license each one declares in its `Cargo.toml`. Where a crate offers a
choice, such as `MIT OR Apache-2.0`, QuickCopy uses it under the MIT
license. The full license texts and copyright notices are in each crate's
source, linked below. Regenerate this list after changing `Cargo.lock`:

```powershell
python tools/third_party.py
```

<!-- crates:begin (generated by tools/third_party.py, do not edit) -->

Dependencies of quickcopy.exe (38 crates, including the proc-macro crates that run while compiling):

| Crate | Version | License | Source |
| --- | --- | --- | --- |
| autocfg | 1.5.1 | Apache-2.0 OR MIT | <https://github.com/cuviper/autocfg> |
| itoa | 1.0.19 | MIT OR Apache-2.0 | <https://github.com/dtolnay/itoa> |
| libloading | 0.9.1 | ISC | <https://github.com/nagisa/rust_libloading/> |
| matrixmultiply | 0.3.11 | MIT/Apache-2.0 | <https://github.com/bluss/matrixmultiply/> |
| memchr | 2.8.3 | Unlicense OR MIT | <https://github.com/BurntSushi/memchr> |
| ndarray | 0.17.2 | MIT OR Apache-2.0 | <https://github.com/rust-ndarray/ndarray> |
| num-complex | 0.4.6 | MIT OR Apache-2.0 | <https://github.com/rust-num/num-complex> |
| num-integer | 0.1.47 | MIT OR Apache-2.0 | <https://github.com/rust-num/num-integer> |
| num-traits | 0.2.19 | MIT OR Apache-2.0 | <https://github.com/rust-num/num-traits> |
| once_cell | 1.21.4 | MIT OR Apache-2.0 | <https://github.com/matklad/once_cell> |
| ort | 2.0.0-rc.15 | MIT OR Apache-2.0 | <https://github.com/pykeio/ort> |
| ort-sys | 2.0.0-rc.15 | MIT OR Apache-2.0 | <https://github.com/pykeio/ort> |
| pin-project-lite | 0.2.18 | Apache-2.0 OR MIT | <https://github.com/taiki-e/pin-project-lite> |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 | <https://github.com/dtolnay/proc-macro2> |
| quote | 1.0.47 | MIT OR Apache-2.0 | <https://github.com/dtolnay/quote> |
| rawpointer | 0.2.1 | MIT/Apache-2.0 | <https://github.com/bluss/rawpointer/> |
| serde | 1.0.229 | MIT OR Apache-2.0 | <https://github.com/serde-rs/serde> |
| serde_core | 1.0.229 | MIT OR Apache-2.0 | <https://github.com/serde-rs/serde> |
| serde_derive | 1.0.229 | MIT OR Apache-2.0 | <https://github.com/serde-rs/serde> |
| serde_json | 1.0.154 | MIT OR Apache-2.0 | <https://github.com/serde-rs/json> |
| smallvec | 1.16.6 | MIT OR Apache-2.0 | <https://github.com/smallvec/smallvec> |
| syn | 2.0.119 | MIT OR Apache-2.0 | <https://github.com/dtolnay/syn> |
| syn | 3.0.10 | MIT OR Apache-2.0 | <https://github.com/dtolnay/syn> |
| tracing | 0.1.44 | MIT | <https://github.com/tokio-rs/tracing> |
| tracing-core | 0.1.36 | MIT | <https://github.com/tokio-rs/tracing> |
| unicode-ident | 1.0.27 | (MIT OR Apache-2.0) AND Unicode-3.0 | <https://github.com/dtolnay/unicode-ident> |
| windows | 0.62.2 | MIT OR Apache-2.0 | <https://github.com/microsoft/windows-rs> |
| windows-collections | 0.3.2 | MIT OR Apache-2.0 | <https://github.com/microsoft/windows-rs> |
| windows-core | 0.62.2 | MIT OR Apache-2.0 | <https://github.com/microsoft/windows-rs> |
| windows-future | 0.3.2 | MIT OR Apache-2.0 | <https://github.com/microsoft/windows-rs> |
| windows-implement | 0.60.2 | MIT OR Apache-2.0 | <https://github.com/microsoft/windows-rs> |
| windows-interface | 0.59.3 | MIT OR Apache-2.0 | <https://github.com/microsoft/windows-rs> |
| windows-link | 0.2.1 | MIT OR Apache-2.0 | <https://github.com/microsoft/windows-rs> |
| windows-numerics | 0.3.1 | MIT OR Apache-2.0 | <https://github.com/microsoft/windows-rs> |
| windows-result | 0.4.1 | MIT OR Apache-2.0 | <https://github.com/microsoft/windows-rs> |
| windows-strings | 0.5.1 | MIT OR Apache-2.0 | <https://github.com/microsoft/windows-rs> |
| windows-threading | 0.2.1 | MIT OR Apache-2.0 | <https://github.com/microsoft/windows-rs> |
| zmij | 1.0.23 | MIT | <https://github.com/dtolnay/zmij> |

Used only by build.rs while building (21 crates):

| Crate | Version | License | Source |
| --- | --- | --- | --- |
| cc | 1.11.3 | MIT OR Apache-2.0 | <https://github.com/rust-lang/cc-rs> |
| cfg-if | 1.0.5 | MIT OR Apache-2.0 | <https://github.com/rust-lang/cfg-if> |
| embed-resource | 3.0.12 | MIT | <https://github.com/nabijaczleweli/rust-embed-resource> |
| equivalent | 1.0.3 | Apache-2.0 OR MIT | <https://github.com/indexmap-rs/equivalent> |
| find-msvc-tools | 0.1.15 | MIT OR Apache-2.0 | <https://github.com/rust-lang/cc-rs> |
| hashbrown | 0.17.2 | MIT OR Apache-2.0 | <https://github.com/rust-lang/hashbrown> |
| indexmap | 2.14.2 | Apache-2.0 OR MIT | <https://github.com/indexmap-rs/indexmap> |
| libc | 0.2.191 | MIT OR Apache-2.0 | <https://github.com/rust-lang/libc> |
| rustc_version | 0.4.1 | MIT OR Apache-2.0 | <https://github.com/djc/rustc-version-rs> |
| semver | 1.0.28 | MIT OR Apache-2.0 | <https://github.com/dtolnay/semver> |
| serde_spanned | 1.1.3 | MIT OR Apache-2.0 | <https://github.com/toml-rs/toml> |
| shlex | 2.0.1 | MIT OR Apache-2.0 | <https://github.com/comex/rust-shlex> |
| toml | 1.2.2+spec-1.1.0 | MIT OR Apache-2.0 | <https://github.com/toml-rs/toml> |
| toml_datetime | 1.1.3+spec-1.1.0 | MIT OR Apache-2.0 | <https://github.com/toml-rs/toml> |
| toml_parser | 1.1.6+spec-1.1.0 | MIT OR Apache-2.0 | <https://github.com/toml-rs/toml> |
| toml_writer | 1.2.0+spec-1.1.0 | MIT OR Apache-2.0 | <https://github.com/toml-rs/toml> |
| vswhom | 0.1.0 | MIT | <https://github.com/nabijaczleweli/vswhom.rs> |
| vswhom-sys | 0.1.3 | MIT | <https://github.com/nabijaczleweli/vswhom-sys.rs> |
| windows-sys | 0.61.2 | MIT OR Apache-2.0 | <https://github.com/microsoft/windows-rs> |
| winnow | 1.0.5 | MIT | <https://github.com/winnow-rs/winnow> |
| winreg | 0.56.0 | MIT | <https://github.com/gentoo90/winreg-rs> |

<!-- crates:end -->
