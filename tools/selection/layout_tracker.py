"""An issue tracker in three panes: a list on the left, comments with inline
code chips in the middle, properties on the right. Imported by gen_layouts."""

CSS = """
body{display:flex;font-size:13.5px;color:#1f2328}
.rail{width:42px;border-right:1px solid #eee;padding-top:14px;text-align:center;font-size:10px;color:#666;flex:none} .rail div{margin:0 0 26px}
.side{width:286px;border-right:1px solid #eee;padding:16px 14px 0 26px;flex:none} .side h3{font-size:16px;margin:2px 0 22px} .sec{color:#666;font-size:12px;margin:0 0 12px}
.it{margin:0 0 14px;display:flex;justify-content:space-between} .it .t{color:#555} .it .s{font-size:12px;color:#777;margin-top:3px} .it .m{font-size:12px;color:#777}
.v{margin:0 0 10px 26px;color:#333}
.mid{flex:1;padding:0 20px} .bar{height:56px;display:flex;align-items:center;font-size:14px;font-weight:600} .bar .c{color:#666;font-weight:400;margin-right:12px} .pill{margin-left:12px;border:1px solid #f0c9a8;color:#4b6fd6;font-weight:400;font-size:12px;border-radius:6px;padding:3px 8px}
.card{border:1px solid #ececec;border-radius:10px;padding:14px 18px;margin-bottom:0} .who{font-size:14px;margin:0 0 10px} .who small{color:#777;font-size:12.5px;margin-left:8px}
.card p{margin:0 0 10px;line-height:27px} .card li{line-height:27px;margin:4px 0} ul{margin:4px 0 8px;padding-left:20px}
code{font-family:Consolas,monospace;font-size:13px;background:#f6f6f6;border:1px solid #e3e3e3;border-radius:4px;padding:2px 6px}
.right{width:340px;border-left:1px solid #eee;padding:18px 24px;flex:none} .right h4{font-size:13.5px;font-weight:600;margin:0 0 16px} .p{display:flex;margin:0 0 19px} .p .k{width:86px;color:#666} .right .g{margin-top:30px}"""

BODY = """
<div class="rail"><div>Work</div><div>Team</div><div>Chat</div><div>Auto</div></div>
<div class="side"><h3>Work</h3><div class="sec">Inbox</div>
<div class="it"><div><div class="t" data-sel="inbox-title-1">devenv: thêm Cloudflare ...</div><div class="s"><span data-sel="inbox-who-1">Phương-CTO</span> Dạ, em rev...</div></div><div class="m" data-sel="inbox-time-1">23m</div></div>
<div class="it"><div><div class="t">devenv: thêm AWS CLI v...</div><div class="s" data-sel="inbox-sub-2">Phương-CTO Triage xong. Phạ...</div></div><div class="m">43m</div></div>
<div class="it"><div><div class="t" data-sel="inbox-title-3">devenv: phân tích repo ...</div><div class="s">Đà-DevOps Stage 1 đã hoàn tấ...</div></div><div class="m" data-sel="inbox-time-3">45m</div></div>
<div class="it"><div><div class="t">Lona Delivery: thêm bướ...</div><div class="s">Phương-CTO New issue</div></div><div class="m">10h</div></div>
<div class="sec" style="margin-top:26px">Views</div><div class="v" data-sel="view-all">All issues</div><div class="v">devenv: thêm AWS CLI v2 op...</div><div class="v" data-sel="view-3">devenv: phân tích repo và re...</div><div class="v">Lona Delivery: thêm bước sp...</div><div class="v">My work</div></div>
<div class="mid"><div class="bar"><span class="c" data-sel="crumb">DUTH-11</span><span data-sel="title">DUTH-14 devenv: thêm Cloudflare CLIs wrangler và cloudflared</span><span class="pill" data-sel="pill">Phương-CTO is working</span></div>
<div class="card"><div class="who"><b data-sel="author-1">Phương-CTO</b><small data-sel="ago-1">24m ago</small></div>
<p data-sel="para-1"><b>@Đà-DevOps</b> em là tác giả PR #8 nên em sửa theo review REQUEST CHANGES ở trên, trên cùng branch đó. Bắt buộc sửa mục Major (README và các thông báo dùng <code data-sel="chip-1">--force --only 35-cloud-cli</code> / <code data-sel="chip-2">-Force -Only 35-cloud-cli</code>, <span data-sel="frag-1">kèm một test Bats chạy bootstrap hai lần với cache tạm</span>). Nên sửa luôn nit <code>gpg --dearmor --yes</code>. <span data-sel="frag-2">Không đụng</span> <code data-sel="chip-3">lib/markers.*</code> hay <code>65-ai-cli</code>. <span data-sel="frag-3">Khi xong, chạy lại lint + Bats/Pester local, post comment kết quả rồi chuyển</span> <code>code_review</code>.</p></div>
<div class="card"><div class="who"><b data-sel="author-2">Đà-DevOps</b><small>21m ago</small></div>
<p><span data-sel="line-fixed">Đã sửa review Major trên cùng PR #8, commit</span> <code data-sel="chip-commit">208dbfc</code>.</p>
<ul><li data-sel="bullet-1">README và cảnh báo thiếu Node/npm nay hướng dẫn <code>devenv up --force --only 35-cloud-cli</code> / <code>devenv up -Force -Only 35-cloud-cli</code> để vượt marker cache khi đổi biến chọn tool.</li>
<li>Thêm Bats test bootstrap hai lần với cache tạm, <span data-sel="frag-4">xác nhận lần rerun</span> <code>--force --only 35-cloud-cli</code> nhận lựa chọn Wrangler mới; thêm <code data-sel="chip-gpg">gpg --dearmor --yes</code>.</li>
<li data-sel="bullet-3">Không thay đổi <code>lib/markers.*</code> hoặc <code>65-ai-cli</code>.</li></ul>
<p>Đã chạy lại: <code data-sel="chip-run">./tests/run.sh</code> <span data-sel="frag-pass">153/153 pass</span>; <code>pwsh -NoProfile -File ./tests/run.ps1</code> 48 pass, 2 skip Windows-only trên Linux.</p></div></div>
<div class="right"><h4 data-sel="props">Properties</h4>
<div class="p"><span class="k" data-sel="k-status">Status</span><span data-sel="v-status">Code Review</span></div>
<div class="p"><span class="k">Assignee</span><span data-sel="v-assignee">Phương-CTO</span></div>
<div class="p"><span class="k" data-sel="k-project">Project</span><span>devenv</span></div>
<div class="p" data-sel="row-workflow"><span class="k">Workflow</span><span>Lona Delivery</span></div>
<div class="p"><span class="k">Priority</span><span data-sel="v-priority">Medium</span></div>
<div class="p"><span class="k">Stage</span><span data-sel="v-stage">Stage 2</span></div>
<h4 class="g" data-sel="h-parent">Parent issue</h4><div class="p"><span data-sel="parent-line">DUTH-11 devenv: phân tích repo và resea...</span></div>
<h4 class="g">Pull requests</h4><div class="p"><span data-sel="pr-title">Add opt-in Cloudflare CLIs</span></div><div class="p"><span style="color:#666">#8</span>&nbsp;&nbsp;<span data-sel="pr-plus" style="color:#1a7f37">+444</span>&nbsp;<span style="color:#cf222e">-5</span></div></div>"""

LAYOUT = (1577, 700, CSS, BODY)
