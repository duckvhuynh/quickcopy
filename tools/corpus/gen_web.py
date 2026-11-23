"""Screenshots public web pages with headless Edge to web/vi-web/<id>.png
and writes manifests/local-vi-web.json (gitignored: it holds the pages' text), whose phrases are the page's visible
text read from the DOM.

web_pages.json lists the pages:
  {"id": "vi-web-...", "url": "https://...", "width": 1280, "height": 800,
   "zoom": 1.25, "dark": true, "drop": ["phrase", ...]}
zoom (default 1) is browser zoom: the layout is width/zoom CSS pixels wide
and the screenshot is still width x height. dark emulates
prefers-color-scheme: dark. Widths under 600 emulate a phone (touch,
mobile user agent that still says Headless). drop lists phrases the DOM
reports but a person does not see as written in the screenshot (covered,
same color as the background, a superscript "o" drawn as a degree sign);
they were found by looking at the PNG at each missed phrase. A page that
shows a bot check or consent wall instead of its content is left out
(voz.vn: Cloudflare check).

Each page loads in a fresh temporary profile with no login, waits for the
load event plus a settle, has CSS animations and transitions cut to their
end state and media paused, then is probed, screenshotted and probed again.
A phrase is one rendered line of text in one box (text nodes split with
Range.getClientRects, one rect per character), kept when it is fully
inside the viewport and every clipping ancestor, not hidden, transparent
or under 9 px, not covered by an opaque element (elementFromPoint), has at
least 3 visible characters, and appears in both probes. Later copies of a
phrase are dropped. Rerunning reshoots the page as it is then.

    python gen_web.py [ID ...]    # all pages, or only these
"""
import base64
import json
import os
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import time
import unicodedata
import urllib.parse
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
PAGES = os.path.join(HERE, "web_pages.json")
OUT = os.path.join(HERE, "web", "vi-web")
MANIFEST = os.path.join(HERE, "manifests", "local-vi-web.json")
EDGE = next(p for p in [r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe", r"C:\Program Files\Microsoft\Edge\Application\msedge.exe"] if os.path.exists(p))
SETTLE = 3.0

FREEZE = r"""(() => {
  const s = new CSSStyleSheet();
  s.replaceSync('*,*::before,*::after{animation-duration:0s!important;animation-delay:0s!important;animation-iteration-count:1!important;transition-duration:0s!important;transition-delay:0s!important;scroll-behavior:auto!important;caret-color:transparent!important}');
  document.adoptedStyleSheets = [...document.adoptedStyleSheets, s];
  document.querySelectorAll('video,audio').forEach(m => { try { m.pause(); } catch (e) {} });
  scrollTo(0, 0);
})()"""

PROBE = r"""(async () => {
  await document.fonts.ready;
  const W = innerWidth, H = innerHeight, Z = devicePixelRatio;
  const styles = new Map(), shown = new Map(), clips = new Map();
  const st = e => { let s = styles.get(e); if (!s) styles.set(e, s = getComputedStyle(e)); return s; };
  const alpha = c => { const m = c.match(/rgba?\(([^)]*)\)/); if (!m) return c === 'transparent' ? 0 : 1; const p = m[1].split(/[ ,\/]+/).filter(Boolean); return p.length > 3 ? parseFloat(p[3]) : 1; };
  const isShown = e => {
    if (!e || e === document.documentElement) return true;
    if (shown.has(e)) return shown.get(e);
    const s = st(e);
    const ok = s.display !== 'none' && s.visibility === 'visible' && parseFloat(s.opacity) >= 0.2 && s.clip === 'auto'
      && !/inset\((49|50|100)%/.test(s.clipPath) && isShown(e.parentElement);
    shown.set(e, ok); return ok;
  };
  const clipOf = e => {
    if (!e || e === document.body || e === document.documentElement) return [0, 0, W, H];
    if (clips.has(e)) return clips.get(e);
    const s = st(e);
    let c = s.position === 'fixed' ? [0, 0, W, H] : clipOf(e.parentElement);
    if (s.overflowX !== 'visible' || s.overflowY !== 'visible') {
      const b = e.getBoundingClientRect();
      c = [Math.max(c[0], b.left), Math.max(c[1], b.top), Math.min(c[2], b.right), Math.min(c[3], b.bottom)];
    }
    clips.set(e, c); return c;
  };
  const boxOf = e => { while (e.parentElement && /^(inline|contents)$/.test(st(e).display)) e = e.parentElement; return e; };
  const opaque = e => { const s = st(e); return alpha(s.backgroundColor) > 0.5 || s.backgroundImage !== 'none' || /^(IMG|VIDEO|CANVAS|IFRAME|SVG|PICTURE|OBJECT|EMBED)$/i.test(e.tagName); };
  const covered = (el, x, y) => {
    const hit = document.elementFromPoint(x, y);
    if (!hit) return true;
    if (hit === el || el.contains(hit) || hit.contains(el)) return false;
    for (let e = hit; e && !e.contains(el); e = e.parentElement) if (opaque(e)) return true;
    return false;
  };
  const skip = /^(SCRIPT|STYLE|NOSCRIPT|TEMPLATE|TEXTAREA|SELECT|OPTION|TITLE)$/;
  const lines = [], range = document.createRange();
  const walk = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  for (let n; (n = walk.nextNode());) {
    const t = n.data, el = n.parentElement, last = lines[lines.length - 1];
    if (!el || skip.test(el.tagName) || el.closest('svg') || !isShown(el)) continue;
    if (!/\S/.test(t)) { if (last && last.box === boxOf(el)) last.text += ' '; continue; }
    const s = st(el);
    if (/icon|awesome|material symbols|glyph/i.test(s.fontFamily) || (alpha(s.color) < 0.2 && s.webkitTextFillColor === s.color)) continue;
    range.selectNodeContents(n);
    const all = range.getBoundingClientRect();
    if (all.bottom < 0 || all.top > H || all.right < 0 || all.left > W || all.width === 0) continue;
    const box = boxOf(el), c = clipOf(el), small = parseFloat(s.fontSize) * Z < 9;
    let cur = lines.length ? lines[lines.length - 1] : null, i = 0;
    for (const ch of t) {
      const len = ch.length; range.setStart(n, i); range.setEnd(n, i + len); i += len;
      if (/\s/.test(ch)) { if (cur && cur.box === box) cur.text += ' '; continue; }
      if (/\p{M}/u.test(ch)) { if (cur) cur.text += ch; continue; }
      const r = range.getClientRects()[0];
      if (!r || r.width === 0 || r.height === 0) continue;
      const cy = (r.top + r.bottom) / 2;
      if (!cur || cur.box !== box || Math.abs(cy - cur.cy) > r.height / 2 || r.left < cur.right - r.height) {
        cur = { box, el, text: '', cy, left: r.left, top: r.top, right: r.right, bottom: r.bottom, bad: false, pts: [] };
        lines.push(cur);
      }
      cur.text += ch;
      cur.right = Math.max(cur.right, r.right); cur.top = Math.min(cur.top, r.top); cur.bottom = Math.max(cur.bottom, r.bottom);
      if (small || r.left < c[0] - 1 || r.top < c[1] - 1 || r.right > c[2] + 1 || r.bottom > c[3] + 1) cur.bad = true;
      cur.pts.push([el, (r.left + r.right) / 2, cy]);
    }
  }
  const out = [];
  for (const l of lines) {
    if (l.bad) continue;
    const text = l.text.normalize('NFC').replace(/[\p{Extended_Pictographic}\u{E000}-\u{F8FF}\u{FE0F}\u200B-\u200D\u2060]/gu, '').replace(/\s+/g, ' ').trim();
    if ((text.match(/[\p{L}\p{N}\p{P}\p{S}]/gu) || []).length < 3) continue;
    const p = l.pts, picks = [p[0], p[p.length >> 1], p[p.length - 1]];
    if (picks.some(([e, x, y]) => covered(e, x, y))) continue;
    out.push(text);
  }
  return out;
})()"""


class CDP:
    """Just enough of a WebSocket client to talk to the DevTools protocol."""

    def __init__(self, url):
        u = urllib.parse.urlparse(url)
        self.sock = socket.create_connection((u.hostname, u.port), timeout=60)
        key = base64.b64encode(os.urandom(16)).decode()
        self.sock.sendall(f"GET {u.path} HTTP/1.1\r\nHost: {u.netloc}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                          f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n".encode())
        self.buf = bytearray()
        while b"\r\n\r\n" not in self.buf:
            self._fill()
        head, _, rest = bytes(self.buf).partition(b"\r\n\r\n")
        if b" 101 " not in head.split(b"\r\n")[0]:
            raise RuntimeError("DevTools refused the WebSocket: " + head.decode(errors="replace"))
        self.buf = bytearray(rest)
        self.next_id = 0
        self.events = []

    def _fill(self):
        d = self.sock.recv(1 << 20)
        if not d:
            raise EOFError("DevTools closed the connection")
        self.buf += d

    def _take(self, n):
        while len(self.buf) < n:
            self._fill()
        out = bytes(self.buf[:n])
        del self.buf[:n]
        return out

    def _send(self, op, data):
        mask = os.urandom(4)
        n = len(data)
        head = bytes([0x80 | op]) + (bytes([0x80 | n]) if n < 126 else bytes([0xFE]) + struct.pack(">H", n) if n < 65536 else bytes([0xFF]) + struct.pack(">Q", n))
        self.sock.sendall(head + mask + bytes(b ^ mask[i & 3] for i, b in enumerate(data)))

    def _recv(self):
        msg = b""
        while True:
            b0, b1 = self._take(2)
            n = b1 & 127
            if n == 126:
                n = struct.unpack(">H", self._take(2))[0]
            elif n == 127:
                n = struct.unpack(">Q", self._take(8))[0]
            data = self._take(n)
            op = b0 & 15
            if op == 8:
                raise EOFError("DevTools closed the connection")
            if op == 9:
                self._send(10, data)
                continue
            msg += data
            if b0 & 0x80:
                return json.loads(msg)

    def call(self, method, timeout=60, **params):
        self.next_id += 1
        self._send(1, json.dumps({"id": self.next_id, "method": method, "params": params}).encode())
        self.sock.settimeout(timeout)
        while True:
            m = self._recv()
            if m.get("id") == self.next_id:
                if "error" in m:
                    raise RuntimeError(f"{method}: {m['error']}")
                return m.get("result", {})
            if "method" in m:
                self.events.append(m["method"])

    def wait_event(self, method, timeout):
        end = time.time() + timeout
        while method not in self.events:
            left = end - time.time()
            if left <= 0:
                return False
            self.sock.settimeout(left)
            try:
                m = self._recv()
            except socket.timeout:
                return False
            if "method" in m:
                self.events.append(m["method"])
        return True

    def eval(self, js, timeout=60):
        r = self.call("Runtime.evaluate", timeout=timeout, expression=js, awaitPromise=True, returnByValue=True)
        if "exceptionDetails" in r:
            raise RuntimeError("page script failed: " + json.dumps(r["exceptionDetails"])[:500])
        return r["result"].get("value")


def png_size(data):
    return struct.unpack(">II", data[16:24])


def shoot(page):
    """Screenshots the page to OUT/<id>.png and returns its phrases."""
    w, h, zoom = page["width"], page["height"], page.get("zoom", 1)
    prof = tempfile.mkdtemp(prefix="quickcopy-web-")
    proc = subprocess.Popen([EDGE, "--headless=new", "--user-data-dir=" + prof, "--remote-debugging-port=0", "--no-first-run",
                             "--no-default-browser-check", "--disable-extensions", "--disable-gpu", "--hide-scrollbars",
                             "--mute-audio", "--lang=vi-VN", f"--window-size={w},{h}", "about:blank"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        port_file = os.path.join(prof, "DevToolsActivePort")
        for _ in range(100):
            if os.path.exists(port_file) and open(port_file).read().strip():
                break
            time.sleep(0.2)
        port = open(port_file).read().split()[0]
        targets = json.load(urllib.request.urlopen(f"http://127.0.0.1:{port}/json/list", timeout=10))
        cdp = CDP(next(t for t in targets if t["type"] == "page")["webSocketDebuggerUrl"])
        mobile = w < 600
        cdp.call("Emulation.setDeviceMetricsOverride", width=round(w / zoom), height=round(h / zoom), deviceScaleFactor=zoom, mobile=mobile)
        if mobile:
            ua = cdp.call("Browser.getVersion")["userAgent"]
            ua = ua.replace("(Windows NT 10.0; Win64; x64)", "(Linux; Android 14; Pixel 8)").replace("Safari/537.36", "Mobile Safari/537.36")
            cdp.call("Emulation.setUserAgentOverride", userAgent=ua, acceptLanguage="vi-VN,vi", platform="Android")
            cdp.call("Emulation.setTouchEmulationEnabled", enabled=True, maxTouchPoints=5)
        cdp.call("Emulation.setEmulatedMedia", features=[{"name": "prefers-color-scheme", "value": "dark" if page.get("dark") else "light"},
                                                         {"name": "prefers-reduced-motion", "value": "reduce"}])
        cdp.call("Emulation.setLocaleOverride", locale="vi-VN")
        cdp.call("Emulation.setTimezoneOverride", timezoneId="Asia/Ho_Chi_Minh")
        cdp.call("Page.setBypassCSP", enabled=True)
        cdp.call("Page.enable")
        cdp.call("Page.navigate", url=page["url"])
        if not cdp.wait_event("Page.loadEventFired", 40):
            print(f"  {page['id']}: no load event after 40 s, shooting anyway")
        time.sleep(SETTLE)
        cdp.eval(FREEZE)
        time.sleep(1.5)
        before = cdp.eval(PROBE, timeout=120)
        data = base64.b64decode(cdp.call("Page.captureScreenshot", timeout=60, format="png")["data"])
        after = set(cdp.eval(PROBE, timeout=120))
        final_url = cdp.eval("location.href")
        try:
            cdp.call("Browser.close", timeout=10)
        except (EOFError, OSError):
            pass
    finally:
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
        shutil.rmtree(prof, ignore_errors=True)
    if png_size(data) != (w, h):
        raise RuntimeError(f"screenshot is {png_size(data)}, not {(w, h)}")
    open(os.path.join(OUT, page["id"] + ".png"), "wb").write(data)
    drop = set(page.get("drop", []))
    seen, expect = set(), []
    for p in before:
        p = unicodedata.normalize("NFC", p)
        if p in after and p not in seen and p not in drop:
            seen.add(p)
            expect.append(p)
    if final_url.split("#")[0] != page["url"].split("#")[0]:
        print(f"  {page['id']}: landed on {final_url}")
    return expect, len(before) - len(after & set(before))


def main():
    os.makedirs(OUT, exist_ok=True)
    only = set(sys.argv[1:])
    pages = json.load(open(PAGES, encoding="utf-8"))
    old = {e["id"]: e for e in json.load(open(MANIFEST, encoding="utf-8"))} if os.path.exists(MANIFEST) else {}
    entries = []
    for page in pages:
        pid = page["id"]
        if only and pid not in only:
            if pid in old:
                entries.append(old[pid])
            continue
        try:
            expect, unstable = shoot(page)
        except Exception as ex:
            print(f"{pid}: FAILED {ex}")
            if pid in old:
                entries.append(old[pid])
            continue
        entries.append({"id": pid, "file": f"web/vi-web/{pid}.png", "source": page["url"],
                        "license": "screenshot of a public page, (c) its publisher; not redistributed",
                        "category": page.get("category", "web"), "lang": "vi", "expect": expect})
        view = f"{page['width']}x{page['height']}" + (f" zoom {page['zoom']}" if page.get("zoom") else "") + (" dark" if page.get("dark") else "")
        print(f"{pid}: {view}, {len(expect)} phrases" + (f" ({unstable} changed between probes)" if unstable else ""))
    json.dump(entries, open(MANIFEST, "w", encoding="utf-8", newline="\n"), ensure_ascii=False, indent=1)
    open(MANIFEST, "a", encoding="utf-8", newline="\n").write("\n")
    print(f"{len(entries)} pages, {sum(len(e['expect']) for e in entries)} phrases -> {os.path.relpath(MANIFEST, HERE)}")


if __name__ == "__main__":
    main()
