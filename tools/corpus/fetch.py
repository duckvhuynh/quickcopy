"""Downloads the corpus images listed in manifests/*.json that are not on
disk yet. The images stay out of git (they belong to their authors); the
manifests keep where each came from.

    python fetch.py
"""
import glob
import json
import os
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
for path in sorted(glob.glob(os.path.join(HERE, "manifests", "*.json"))):
    for e in json.load(open(path, encoding="utf-8")):
        dst = os.path.join(HERE, e["file"])
        if os.path.exists(dst) or not e.get("download"):
            continue
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        req = urllib.request.Request(e["download"], headers={"User-Agent": "Mozilla/5.0 (corpus fetch)"})
        try:
            with urllib.request.urlopen(req, timeout=60) as r, open(dst, "wb") as f:
                f.write(r.read())
            print("got", e["file"])
        except Exception as ex:
            print("FAILED", e["id"], ex)
