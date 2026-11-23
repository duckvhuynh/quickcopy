"""Runs selection cases through `quickcopy --drag` and scores them.

    python run_cases.py <exe> <cases.json> [<cases.json> ...] [--english] [--show-pass]

A case passes when the selected text reads like the expected text
(similarity >= 0.9 after collapsing whitespace) and is not much longer or
shorter (within 12%), so text dragged in from elsewhere or cut off fails.
"""
import difflib
import json
import os
import re
import subprocess
import sys

sys.stdout.reconfigure(encoding="utf-8")
args = [a for a in sys.argv[1:] if not a.startswith("--")]
exe, files = args[0], args[1:]
env = dict(os.environ)
if "--english" in sys.argv:
    env["QC_ENGLISH"] = "1"
else:
    env.pop("QC_ENGLISH", None)


def norm(s):
    return re.sub(r"\s+", " ", s).strip()


total = passed = 0
for path in files:
    cases = json.load(open(path, encoding="utf-8"))
    by_image = {}
    for c in cases:
        by_image.setdefault(c["image"], []).append(c)
    for image, group in by_image.items():
        coords = [str(c[k]) for c in group for k in ("x0", "y0", "x1", "y1")]
        out = subprocess.run([exe, "--drag", os.path.join(os.path.dirname(path), image), *coords], capture_output=True, env=env, timeout=120).stdout.decode("utf-8", "replace")
        got = [json.loads(m) for m in re.findall(r"\): (\".*\")\s*$", out, flags=re.M)]
        if len(got) != len(group):
            print(f"!! {image}: {len(got)} results for {len(group)} cases\n{out[:400]}")
            total += len(group)
            continue
        fails = []
        for c, g in zip(group, got):
            want, g = norm(c["expect"]), norm(g)
            ratio = difflib.SequenceMatcher(None, want, g).ratio()
            ok = ratio >= 0.9 and abs(len(g) - len(want)) <= max(2, 0.12 * len(want))
            total += 1
            passed += ok
            if not ok or "--show-pass" in sys.argv:
                fails.append(f"   {'ok  ' if ok else 'FAIL'} {c['name']:<16} {ratio:.2f}  want [{want[:70]}]\n{'':27}got  [{g[:110]}]{' ...' if len(g) > 110 else ''} ({len(g)} chars)")
        print(f"{image}: {len(group) - sum('FAIL' in f for f in fails)}/{len(group)}")
        print("\n".join(fails)) if fails else None
print(f"\nTOTAL {passed}/{total} pass")
