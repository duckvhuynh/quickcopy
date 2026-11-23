"""Renders text layouts with headless Edge and writes selection cases.

Every element with data-sel is one case: drag from the left edge of its
first line to the right edge of its last line; the selection must be the
element's text. Output: <out>/<name>.png and <out>/cases.json.
"""
import base64
import json
import re
import os
import subprocess
import sys
import time

OUT = sys.argv[1]
EDGE = next(p for p in [r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe", r"C:\Program Files\Microsoft\Edge\Application\msedge.exe"] if os.path.exists(p))

BASE = """<!doctype html><html><head><meta charset="utf-8"><style>
*{box-sizing:border-box} body{margin:0;font-family:'Segoe UI',Arial,sans-serif;font-size:14px;color:#222;background:#fff}
%s</style></head><body><div class="page" style="position:relative;overflow:hidden;width:%dpx;height:%dpx">%s</div>
<script>
const out=[];for(const e of document.querySelectorAll('[data-sel]')){const g=document.createRange();g.selectNodeContents(e);const r=[...g.getClientRects()].filter(r=>r.width>1&&r.height>1).sort((p,q)=>p.top-q.top||p.left-q.left);
const a=r[0],lr=r.filter(q=>Math.abs(q.top-r[r.length-1].top)<q.height/2),b=lr.reduce((p,q)=>q.right>p.right?q:p);out.push({name:e.dataset.sel,x0:Math.round(a.left)+1,y0:Math.round((a.top+a.bottom)/2),x1:Math.round(b.right)-1,y1:Math.round((b.top+b.bottom)/2),expect:e.innerText.replace(/\\s+/g,' ').trim()});}
console.log('CASES:'+btoa(unescape(encodeURIComponent(JSON.stringify(out)))));
</script></body></html>"""

P1 = "Our new release focuses on speed and reliability across every platform. Startup time is now forty percent faster on older hardware, and the app uses less memory when many documents are open at once."
P2 = "We also rewrote the sync engine, so changes made offline merge safely when you reconnect. Shared folders now show who edited a file and when."
P3 = "The settings page has been reorganized so the options you use most are easy to find. Thank you for all the feedback that made these changes possible."
P4 = "Version history keeps thirty days of changes for every document. You can restore any earlier version with a single click, or compare two versions side by side."
V1 = "Thành phố vừa công bố kế hoạch mở rộng tuyến xe buýt nhanh trong năm tới. Theo đó, mười hai tuyến mới sẽ kết nối các khu dân cư phía đông với trung tâm."
V2 = "Người dân có thể mua vé tháng qua ứng dụng điện thoại. Giá vé dành cho học sinh và sinh viên được giảm một nửa so với mức thông thường."
V3 = "Các chuyên gia giao thông cho rằng việc ưu tiên làn đường riêng sẽ rút ngắn đáng kể thời gian di chuyển vào giờ cao điểm buổi sáng và buổi chiều."

LAYOUTS = {}

LAYOUTS["article-2col"] = (1100, 560, """
h1{font-size:28px;text-align:center;margin:24px 0 6px} .by{text-align:center;color:#777;margin-bottom:18px}
.cols{columns:2;column-gap:48px;padding:0 60px;line-height:1.55;text-align:justify} p{margin:0 0 12px}""",
    f"""<h1 data-sel="headline">Release notes for the spring update</h1><div class="by">Published by the product team</div>
<div class="cols"><p data-sel="left-para">{P1}</p><p>{P2}</p><p>{P4}</p><p data-sel="right-para">{P3}</p><p>{P2} <span data-sel="right-sentence">Shared folders now show who edited a file and when.</span></p><p>{P4}</p></div>""")

LAYOUTS["table"] = (1000, 420, """
table{border-collapse:collapse;margin:30px;width:940px} th,td{border-bottom:1px solid #e3e3e3;padding:10px 14px;text-align:left} th{background:#f6f6f6;font-weight:600} td.n{text-align:right}""",
    """<table><tr><th>Order</th><th>Customer</th><th>Status</th><th>Created</th><th style="text-align:right">Total</th></tr>
<tr data-sel="row-1"><td>INV-1042</td><td>Northwind Traders</td><td>Paid</td><td>12 March 2024</td><td class="n">1,250.00</td></tr>
<tr><td>INV-1043</td><td data-sel="cell-customer">Contoso Pharmaceuticals</td><td>Pending review</td><td>13 March 2024</td><td class="n">830.50</td></tr>
<tr><td>INV-1044</td><td>Fabrikam Residences</td><td data-sel="cell-status">Awaiting payment</td><td>15 March 2024</td><td class="n" data-sel="cell-total">12,400.00</td></tr>
<tr data-sel="row-4"><td>INV-1045</td><td>Adventure Works</td><td>Refunded</td><td>18 March 2024</td><td class="n">96.20</td></tr>
<tr><td>INV-1046</td><td>Wide World Importers</td><td>Paid</td><td>21 March 2024</td><td class="n">4,075.00</td></tr></table>""")

LAYOUTS["form-2col"] = (1240, 420, """
.g{display:grid;grid-template-columns:1fr 1fr;gap:0 24px;padding:24px 36px;color:#333} .h{margin:14px 0 8px} .r{margin:9px 0;display:flex;align-items:center} .b{width:18px;height:18px;border:1px solid #bbb;border-radius:4px;margin-right:10px;flex:none} .o{border-radius:50%} .hint{color:#999;font-size:12px;margin:4px 0 0 4px} .ind{margin-left:18px;color:#999} b{font-weight:600}""",
    """<div class="g"><div><div class="h">Ask client to pay by card</div>
<div class="r"><span class="b o"></span><span data-sel="left-radio-1">When booking is created</span><span class="b o" style="margin-left:28px"></span><span data-sel="left-radio-2">After driver is assigned</span></div>
<div class="r"><span class="b o"></span><span>Never. Client will pay by account</span></div>
<div class="r"><span class="b"></span><b data-sel="left-bold">Overwrite email settings - When driver is assigned:</b></div>
<div class="r ind"><span class="b"></span><span>Automatically send driver details to the client by Email</span></div>
<div class="r ind"><span class="b"></span><span>Automatically send driver details to the client by SMS</span></div></div>
<div><div class="h" data-sel="right-heading">Booking Price</div>
<div class="r"><span class="b"></span><span data-sel="right-line-1">Show instant price when client is booking.</span></div>
<div class="r"><span class="b"></span><span data-sel="right-line-2">Never show booking price to the customer in booking detail page and emails</span></div>
<div class="r"><span class="b"></span><b>Overwrite email settings - When driver arrived at pickup location:</b></div>
<div class="r ind"><span class="b"></span><span data-sel="right-indent">Automatically send driver arrived at location to the client by Email</span></div>
<div class="r ind"><span class="b"></span><span>Automatically send driver arrived at location to the client by SMS</span></div></div></div>""")

LAYOUTS["chat"] = (760, 520, """
body{background:#eef1f5} .m{max-width:420px;padding:9px 13px;border-radius:14px;margin:10px 18px;line-height:1.4;width:fit-content} .a{background:#fff} .b{background:#0b84ff;color:#fff;margin-left:auto} .t{text-align:center;color:#8a8f98;font-size:12px;margin:14px}""",
    """<div class="t">Today 09:41</div>
<div class="m a" data-sel="in-1">Hi Linh, are we still meeting at the office on Friday morning?</div>
<div class="m b" data-sel="out-1">Yes, ten o'clock works for me. I will bring the printed contracts and the updated price list.</div>
<div class="m a">Great. Could you also send me the address again? I lost the last message when I changed phones.</div>
<div class="m b" data-sel="out-2">Sure: 27 Nguyen Hue Boulevard, fourth floor.</div>
<div class="m a" data-sel="in-3">Thanks, see you then!</div>""")

LAYOUTS["dashboard"] = (1200, 520, """
body{background:#f4f5f7;display:flex} nav{width:210px;background:#1f2430;color:#cfd3dc;padding:20px 0;height:520px} nav div{padding:10px 22px} nav .on{background:#2c3342;color:#fff}
main{flex:1;padding:22px} .cards{display:grid;grid-template-columns:repeat(3,1fr);gap:16px} .c{background:#fff;border-radius:8px;padding:16px 18px} .k{color:#7a808c;font-size:13px} .v{font-size:26px;font-weight:600;margin:6px 0} .d{font-size:12px;color:#1a9a55}
.list{background:#fff;border-radius:8px;margin-top:16px;padding:6px 18px} .li{display:flex;justify-content:space-between;padding:11px 0;border-bottom:1px solid #eee}""",
    """<nav><div class="on">Overview</div><div data-sel="nav-item">Bookings and trips</div><div>Drivers</div><div>Clients</div><div>Invoices</div><div>Settings</div></nav>
<main><div class="cards"><div class="c"><div class="k" data-sel="card-label">Revenue this month</div><div class="v" data-sel="card-value">$48,920</div><div class="d">up 12% from last month</div></div>
<div class="c"><div class="k">Completed trips</div><div class="v">1,284</div><div class="d" data-sel="card-delta">up 4% from last month</div></div>
<div class="c"><div class="k">Average rating</div><div class="v">4.87</div><div class="d">based on 962 reviews</div></div></div>
<div class="list"><div class="li"><span data-sel="list-name">Airport transfer to city center</span><span>$42.00</span></div><div class="li" data-sel="list-row"><span>Hourly chauffeur service</span><span>$65.00</span></div><div class="li"><span>Intercity ride to Da Nang</span><span data-sel="list-price">$180.00</span></div></div></main>""")

LAYOUTS["receipt"] = (520, 560, """
body{font-family:Consolas,'Courier New',monospace;font-size:15px;padding:24px 30px} .c{text-align:center} .r{display:flex;justify-content:space-between;margin:7px 0} hr{border:0;border-top:1px dashed #999;margin:12px 0}""",
    """<div class="c" data-sel="shop">GREEN LEAF COFFEE</div><div class="c">12 Tran Phu Street</div><hr>
<div class="r"><span data-sel="item-1">Iced latte large</span><span>4.50</span></div>
<div class="r" data-sel="row-2"><span>Butter croissant</span><span>3.20</span></div>
<div class="r"><span>Matcha cheesecake slice</span><span data-sel="price-3">5.80</span></div>
<div class="r"><span>Sparkling water</span><span>1.90</span></div><hr>
<div class="r"><span>Subtotal</span><span>15.40</span></div><div class="r"><span>Service charge 5%</span><span>0.77</span></div>
<div class="r" data-sel="total-row"><span>TOTAL</span><span>16.17</span></div><hr><div class="c" data-sel="thanks">Thank you and see you again</div>""")

LAYOUTS["code"] = (900, 400, """
body{background:#1e1e1e;color:#d4d4d4;font-family:Consolas,monospace;font-size:14px;padding:16px} .l{display:flex;line-height:22px} .n{width:40px;color:#6e7681;text-align:right;margin-right:18px;flex:none} pre{margin:0;font:inherit}""",
    "".join(f'<div class="l"><span class="n">{i+1}</span><pre{" data-sel=" + chr(34) + "line-" + str(i+1) + chr(34) if i in (1, 5) else ""}>{t}</pre></div>' for i, t in enumerate([
        "fn main() {", "    let settings = Settings::load_from_disk();", "    let mut total = 0;", "    for order in settings.orders.iter() {", "        total += order.price * order.quantity;", "        println!(\"order {} costs {}\", order.id, order.price);", "    }", "    println!(\"total: {}\", total);", "}"])))

LAYOUTS["vi-news-3col"] = (1200, 520, """
h1{font-size:26px;margin:22px 40px 14px} .cols{columns:3;column-gap:36px;padding:0 40px;line-height:1.6} p{margin:0 0 12px}""",
    f"""<h1 data-sel="headline">Thành phố mở rộng mạng lưới xe buýt nhanh</h1><div class="cols"><p data-sel="col1-para">{V1}</p><p>{V2}</p><p>{V3}</p><p data-sel="col2-para">{V2}</p><p>{V1}</p><p>{V3}</p><p data-sel="col3-para">{V3}</p><p>{V2}</p></div>""")

LAYOUTS["slide"] = (1100, 560, """
body{background:#0f2a43;color:#fff;padding:54px 70px} h1{font-size:44px;margin:0 0 30px;font-weight:600} li{font-size:24px;margin:16px 0} .f{position:absolute;bottom:26px;left:70px;font-size:13px;color:#9fb3c8} .pg{position:absolute;bottom:26px;right:70px;font-size:13px;color:#9fb3c8}""",
    """<h1 data-sel="title">Quarterly results at a glance</h1><ul><li data-sel="bullet-1">Revenue grew eighteen percent year over year</li><li>Customer churn fell below two percent</li><li data-sel="bullet-3">Three new regional offices opened in the second quarter</li><li>Support response time is now under four hours</li></ul><div class="f" data-sel="footer">Confidential, for internal use only</div><div class="pg">Page 7 of 24</div>""")

LAYOUTS["settings-dark"] = (820, 440, """
body{background:#202020;color:#f0f0f0;padding:20px 28px} .r{display:flex;justify-content:space-between;align-items:center;padding:13px 0} .s{color:#a0a0a0;font-size:12px;margin-top:3px} .v{color:#9fd0ff}""",
    """<div class="r"><div><div data-sel="label-1">Start with Windows</div><div class="s">Launch the app when you sign in</div></div><div class="v">Enabled</div></div>
<div class="r"><div><div>Translate into</div><div class="s" data-sel="sub-2">Language used for translated text</div></div><div class="v" data-sel="value-2">Vietnamese</div></div>
<div class="r" data-sel="row-3"><div>Translate shortcut</div><div class="v">Ctrl+Shift+T</div></div>
<div class="r"><div><div>App language</div><div class="s">Menus and messages</div></div><div class="v" data-sel="value-4">English (United States)</div></div>
<div class="r"><div><div data-sel="label-5">Vietnamese text recognition</div><div class="s">Reads tone marks correctly</div></div><div class="v">On</div></div>""")

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import layout_tracker

LAYOUTS["tracker-3pane"] = layout_tracker.LAYOUT

cases = []
for name, (w, h, css, body) in LAYOUTS.items():
    html = os.path.join(OUT, name + ".html")
    open(html, "w", encoding="utf-8").write(BASE % (css.replace("body{", ".page{").replace("height:520px", "height:100%"), w, h, body))
    url = "file:///" + html.replace("\\", "/")
    flags = ["--headless=new", "--user-data-dir=" + os.path.join(OUT, "_profile"), "--no-first-run", "--disable-gpu", "--hide-scrollbars", "--force-device-scale-factor=1", f"--window-size={w},{h}"]
    log = os.path.join(OUT, name + ".log")
    text = ""
    for attempt in range(4):
        if os.path.exists(log):
            os.remove(log)
        subprocess.run([EDGE, *flags, "--enable-logging", f"--log-file={log}", "--v=0", "--virtual-time-budget=2000", f"--screenshot={os.path.join(OUT, name + '.png')}", url], capture_output=True, timeout=60)
        for _ in range(60):
            if os.path.exists(log):
                text = open(log, encoding="utf-8", errors="replace").read()
                if "CASES:" in text or "ProcessSingleton" in text:
                    break
            time.sleep(0.2)
        if "CASES:" in text:
            break
        time.sleep(1.5)
    data = base64.b64decode(re.search(r"CASES:([A-Za-z0-9+/=]+)", text).group(1)).decode("utf-8")
    for c in json.loads(data):
        cases.append({"image": name + ".png", **c})
    print(name, "ok", os.path.getsize(os.path.join(OUT, name + ".png")))
json.dump(cases, open(os.path.join(OUT, "cases.json"), "w", encoding="utf-8"), ensure_ascii=False, indent=1)
print(len(cases), "cases")
