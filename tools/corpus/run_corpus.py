"""Reads every image in the corpus with `quickcopy --ocr-file` and checks
that the phrases a person sees in it were read.

    python run_corpus.py <exe> [--english] [--only ID,ID] [--show-pass]

Manifests are manifests/*.json, each a list of:
  {"id": "...", "file": "web/<category>/<name>.png", "source": "<url>",
   "category": "...", "lang": "en" | "vi", "expect": ["phrase", ...]}
A phrase is found when the read text contains it, case and spacing
ignored. Phrases of 20 characters or more may differ a little (similarity
0.92: a wrong letter in a sentence passes, a missing word fails); shorter
ones must match exactly, since a wrong letter there is a wrong word. Prints each image's
score and the phrases it missed; exits 1 if any phrase was missed.
"""
import difflib
import glob
import json
import os
import re
import subprocess
import sys

sys.stdout.reconfigure(encoding="utf-8")
HERE = os.path.dirname(os.path.abspath(__file__))
args = [a for a in sys.argv[1:] if not a.startswith("--")]
exe = os.path.abspath(args[0])
only = None
if "--only" in sys.argv:
    only = set(sys.argv[sys.argv.index("--only") + 1].split(","))
env = dict(os.environ)
if "--english" in sys.argv:
    env["QC_ENGLISH"] = "1"
else:
    env.pop("QC_ENGLISH", None)


def norm(s):
    s = s.lower()
    s = re.sub(r"\s+", " ", s)
    # OCR puts spaces around punctuation that the image does not have.
    s = re.sub(r" ?([^\w\s]) ?", r"\1", s)
    return s.strip()


def found(phrase, text):
    p = norm(phrase)
    if not p:
        return True
    if p in text:
        return True
    n = len(p)
    if n < 20:
        return False
    best = 0.0
    sm = difflib.SequenceMatcher(None, autojunk=False)
    sm.set_seq2(p)
    for i in range(0, max(1, len(text) - n + 1)):
        for w in (n - 1, n, n + 1):
            sm.set_seq1(text[i : i + w])
            if sm.quick_ratio() < 0.92:
                continue
            best = max(best, sm.ratio())
            if best >= 0.92:
                return True
    return False


entries = []
for path in sorted(glob.glob(os.path.join(HERE, "manifests", "*.json"))):
    for e in json.load(open(path, encoding="utf-8")):
        e["_manifest"] = os.path.basename(path)
        entries.append(e)

total = hit = images = missing_files = 0
for e in entries:
    if only and e["id"] not in only:
        continue
    img = os.path.join(HERE, e["file"])
    if not os.path.exists(img):
        missing_files += 1
        print(f"SKIP {e['id']}: {e['file']} not downloaded (python fetch.py)")
        continue
    out = subprocess.run([exe, "--ocr-file", img], capture_output=True, env=env, timeout=300).stdout.decode("utf-8", "replace")
    m = re.search(r"--- text ---\n(.*?)\n--- word boxes ---", out, flags=re.S)
    text = norm(m.group(1)) if m else ""
    misses = [p for p in e["expect"] if not found(p, text)]
    images += 1
    total += len(e["expect"])
    hit += len(e["expect"]) - len(misses)
    if misses or "--show-pass" in sys.argv:
        print(f"{'FAIL' if misses else 'PASS'} {e['id']} [{e['category']}] {len(e['expect']) - len(misses)}/{len(e['expect'])}")
        for p in misses:
            print(f"    missed: {p!r}")
print(f"\n{images} images, {hit}/{total} phrases read ({100 * hit / max(1, total):.1f}%)" + (f", {missing_files} not downloaded" if missing_files else ""))
sys.exit(1 if hit < total else 0)
