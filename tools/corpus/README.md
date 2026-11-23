# OCR corpus

Images with text, and the phrases a person sees in each, to check that
QuickCopy reads everything on them:

```powershell
python tools/corpus/fetch.py                       # download web images not on disk yet
python tools/corpus/gen_synth.py                   # render synth/*.html to web/synth/
python tools/corpus/run_corpus.py target/release/quickcopy.exe            # Vietnamese mode
python tools/corpus/run_corpus.py target/release/quickcopy.exe --english  # Windows OCR only
```

`run_corpus.py` prints each image that missed a phrase, the phrases, and a
total. `--only ID,ID` runs some images; `--show-pass` lists passing ones too.

## Layout

- `manifests/<name>.json`: one file per contributor, never shared, so
  several people (or agents) can add images at once. Each entry:
  ```json
  {"id": "receipt-cafe-1", "file": "web/<name>/receipt-cafe-1.jpg",
   "source": "<page the image is shown on>", "download": "<direct image url>",
   "license": "CC BY-SA 4.0", "category": "receipt", "lang": "en",
   "expect": ["Cappuccino", "TOTAL 12.50", "Thank you for visiting"]}
  ```
- `web/`: downloaded images, not in git (they belong to their authors).
  `fetch.py` downloads them again from `download`.
- `synth/*.html`: pages made for hard cases; `gen_synth.py` renders them
  with headless Edge and writes `manifests/synth.json` from every element
  marked `data-expect` (its text is the phrase).

## Adding images

1. Pick images a person would hover in daily use: screenshots of apps,
   chats and code, social posts and infographics, slides, documents,
   receipts and menus, signs and posters, memes, charts, video frames with
   subtitles, Vietnamese text. Prefer free licenses (Wikimedia Commons,
   Unsplash, Pexels) and record the license.
2. Download with curl into `web/<name>/`, one new folder per contributor.
   Keep only real images (PNG, JPEG, WebP) that open; at least 300 px on
   the short side.
3. Write `expect` by **looking at the image**, never from QuickCopy's
   output (that would only test that it reads what it reads). Copy text
   exactly as shown, with its case and punctuation. 5-15 phrases per
   image: whole lines or short sentences, and the hard spots (small text,
   colored or underlined links, text on photos, dense lists, code).
   Phrases under 20 characters must match exactly, so make them distinct
   (`07 /text-behind`, not `text-behind` when a sidebar shows it too).
   Leave out text you cannot read with certainty.
4. Run `run_corpus.py --only <your ids>` to see the score. A miss is a
   finding about QuickCopy, not a reason to change the phrase, unless the
   phrase itself was wrong.
