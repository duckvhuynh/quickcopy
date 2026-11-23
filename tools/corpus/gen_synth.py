"""Renders synth/*.html with headless Edge to web/synth/<name>.png and
writes manifests/synth.json.

Each page sets its own size with <meta name="size" content="WxH">, its
category with <meta name="category" content="...">, and its language with
<html lang="...">. Every element marked data-expect is one phrase: its
innerText with whitespace collapsed (so text-transform applies, as it does
on screen). Rerunning rewrites the same outputs.

    python gen_synth.py [NAME ...]    # all pages, or only these
"""
import base64
import glob
import json
import os
import re
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
SRC = os.path.join(HERE, "synth")
OUT = os.path.join(HERE, "web", "synth")
MANIFEST = os.path.join(HERE, "manifests", "synth.json")
WORK = os.path.join(tempfile.gettempdir(), "quickcopy-synth")
EDGE = next(p for p in [r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe", r"C:\Program Files\Microsoft\Edge\Application\msedge.exe"] if os.path.exists(p))

PROBE = """<script>
const all=[...document.querySelectorAll('[data-expect]')],W=%d,H=%d;
const t=e=>(e.innerText??e.textContent).replace(/\\s+/g,' ').trim();
const off=all.filter(e=>{const r=e.getBoundingClientRect();return r.width<1||r.left<-1||r.top<-1||r.right>W+1||r.bottom>H+1}).map(t);
console.log('EXPECT:'+btoa(unescape(encodeURIComponent(JSON.stringify({expect:all.map(t).filter(x=>x),off})))));
</script>"""


def meta(html, name):
    m = re.search(r'<meta\s+name="%s"\s+content="([^"]*)"' % name, html)
    if not m:
        raise SystemExit(f"missing <meta name={name!r}>")
    return m.group(1)


def render(name, html, w, h):
    """Screenshots the page and returns the data-expect phrases."""
    page = os.path.join(WORK, name + ".html")
    open(page, "w", encoding="utf-8").write(html.replace("</body>", PROBE % (w, h) + "</body>"))
    png = os.path.join(OUT, name + ".png")
    log = os.path.join(WORK, name + ".log")
    flags = ["--headless=new", "--user-data-dir=" + os.path.join(WORK, "_profile"), "--no-first-run", "--disable-gpu", "--hide-scrollbars", "--force-device-scale-factor=1", f"--window-size={w},{h}"]
    text = ""
    for _ in range(4):
        for f in (log, png):
            if os.path.exists(f):
                os.remove(f)
        subprocess.run([EDGE, *flags, "--enable-logging", f"--log-file={log}", "--v=0", "--virtual-time-budget=2000", f"--screenshot={png}", "file:///" + page.replace("\\", "/")], capture_output=True, timeout=60)
        for _ in range(60):
            if os.path.exists(log):
                text = open(log, encoding="utf-8", errors="replace").read()
                if "EXPECT:" in text or "ProcessSingleton" in text:
                    break
            time.sleep(0.2)
        if "EXPECT:" in text and os.path.exists(png):
            break
        time.sleep(1.5)
    m = re.search(r"EXPECT:([A-Za-z0-9+/=]+)", text)
    if not m:
        raise SystemExit(f"{name}: Edge did not report phrases")
    got = json.loads(base64.b64decode(m.group(1)).decode("utf-8"))
    if got["off"]:
        raise SystemExit(f"{name}: marked text outside the {w}x{h} page: {got['off']}")
    return got["expect"]


def main():
    os.makedirs(OUT, exist_ok=True)
    os.makedirs(WORK, exist_ok=True)
    only = set(sys.argv[1:])
    old = {}
    if only and os.path.exists(MANIFEST):
        old = {e["id"]: e for e in json.load(open(MANIFEST, encoding="utf-8"))}
    entries = []
    for path in sorted(glob.glob(os.path.join(SRC, "*.html"))):
        name = os.path.splitext(os.path.basename(path))[0]
        eid = "synth-" + name
        if only and name not in only:
            if eid in old:
                entries.append(old[eid])
            continue
        html = open(path, encoding="utf-8").read()
        w, h = (int(v) for v in meta(html, "size").split("x"))
        lang = re.search(r'<html[^>]*\blang="([^"]+)"', html).group(1)
        expect = render(name, html, w, h)
        dupes = {p for p in expect if len(p) < 20 and expect.count(p) > 1}
        if dupes:
            raise SystemExit(f"{name}: short phrases repeat: {sorted(dupes)}")
        entries.append({"id": eid, "file": f"web/synth/{name}.png", "source": f"tools/corpus/synth/{name}.html",
                        "license": "own work", "category": meta(html, "category"), "lang": lang, "expect": expect})
        print(f"{name}: {w}x{h}, {len(expect)} phrases")
    json.dump(entries, open(MANIFEST, "w", encoding="utf-8", newline="\n"), ensure_ascii=False, indent=1)
    open(MANIFEST, "a", encoding="utf-8", newline="\n").write("\n")
    print(f"{len(entries)} pages, {sum(len(e['expect']) for e in entries)} phrases -> {os.path.relpath(MANIFEST, HERE)}")


main()
