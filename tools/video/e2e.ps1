param([string]$Exe, [string]$Query, [string]$Name, [string]$Dir = "$PSScriptRoot\out", [int]$Watch = 9000, [switch]$ClickToPlay, [int]$AwayAfterPlay = 0, [int]$ClickAgainAfter = 0, [switch]$CopyLine)
# Usage (after make_videos.ps1), the four cases:
#   e2e.ps1 -Exe quickcopy.exe -Query '' -Name paused -CopyLine
#   e2e.ps1 -Exe quickcopy.exe -Query '?muted=1&play=1&pauseAfter=5000' -Name pauses
#   e2e.ps1 -Exe quickcopy.exe -Query '?muted=1&play=1' -Name plays -Watch 6000
#   e2e.ps1 -Exe quickcopy.exe -Query '?src=static.mp4' -Name still -ClickToPlay -AwayAfterPlay 2000 -ClickAgainAfter 4000
# Opens the test video page, starts QuickCopy, rests the cursor on the video
# and records when the app sees it playing, when the overlay appears, and
# (with -CopyLine) what dragging over the title line copies.
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class Z {
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int cx, int cy, uint f);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, int dx, int dy, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr e);
  [StructLayout(LayoutKind.Sequential)] public struct PT { public int X, Y; }
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref PT p);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowW(string c, string t);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  public static void Click() { mouse_event(2, 0, 0, 0, UIntPtr.Zero); mouse_event(4, 0, 0, 0, UIntPtr.Zero); }
}
'@
[Z]::SetProcessDpiAwarenessContext([IntPtr]-4) | Out-Null
$S = $Dir; $V = $Dir; $log = "$Dir\video-$Name.log"
$saved = Get-Clipboard -Raw
Get-Process quickcopy -ErrorAction SilentlyContinue | Stop-Process -Force
foreach ($title in 'video.html', 'play blocked') { $h = [Z]::FindWindowW('Chrome_WidgetWin_1', $title); if ($h -ne [IntPtr]::Zero) { [Z]::PostMessageW($h, 0x10, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null } }
Start-Sleep -Milliseconds 700
[Z]::SetCursorPos(5, 5) | Out-Null
Start-Process msedge -ArgumentList "--new-window", "--app=file:///$(($V + '\video.html') -replace '\\','/')$Query"
$pb = $null
for ($i = 0; $i -lt 50 -and -not $pb; $i++) { Start-Sleep -Milliseconds 150; foreach ($title in 'video.html', 'play blocked') { $h = [Z]::FindWindowW('Chrome_WidgetWin_1', $title); if ($h -ne [IntPtr]::Zero) { $pb = [pscustomobject]@{ MainWindowHandle = $h; MainWindowTitle = $title } } } }
[Z]::SetWindowPos($pb.MainWindowHandle, [IntPtr]-1, 40, 30, 1000, 640, 0x0040) | Out-Null
Start-Sleep -Milliseconds 1500
$o = New-Object Z+PT; [Z]::ClientToScreen($pb.MainWindowHandle, [ref]$o) | Out-Null
$env:QC_LOG = "1"; $env:QC_CAPTURABLE = "1"
$qc = Start-Process $Exe -PassThru -RedirectStandardError $log
Start-Sleep -Milliseconds 1500
$res = @("page $Query, window '$($pb.MainWindowTitle)'")
function Lines { Get-Content $log -ErrorAction SilentlyContinue }
function Shot($file) { $b = New-Object System.Drawing.Bitmap 960, 540; $g = [System.Drawing.Graphics]::FromImage($b); $g.CopyFromScreen($o.X, $o.Y, 0, 0, $b.Size); $b.Save($file, [System.Drawing.Imaging.ImageFormat]::Png); $g.Dispose(); $b.Dispose() }
try {
  # Rest the cursor on the video, away from the text and the controls.
  [Z]::SetCursorPos($o.X + 700, $o.Y + 420) | Out-Null
  if ($ClickToPlay) { Start-Sleep -Milliseconds 30; [Z]::Click(); $res += "clicked the video (play)"; if ($AwayAfterPlay -gt 0) { [Z]::SetCursorPos(5, 5) | Out-Null; Start-Sleep -Milliseconds $AwayAfterPlay; [Z]::SetCursorPos($o.X + 700, $o.Y + 420) | Out-Null; $res += "cursor away for $AwayAfterPlay ms, back on the video" } }
  $sw = [Diagnostics.Stopwatch]::StartNew()
  $clicked2 = $false
  while ($sw.ElapsedMilliseconds -lt $Watch) {
    if ($ClickAgainAfter -gt 0 -and -not $clicked2 -and $sw.ElapsedMilliseconds -ge $ClickAgainAfter) { [Z]::Click(); $clicked2 = $true; $res += "clicked the video again (pause) at $($sw.ElapsedMilliseconds) ms" }
    if (Lines | ? { $_ -match 'overlay \d+x\d+' }) { break }
    Start-Sleep -Milliseconds 50
  }
  $shown = Lines | ? { $_ -match 'overlay \d+x\d+' } | select -First 1
  $res += "overlay: $(if ($shown) { "$shown after $($sw.ElapsedMilliseconds) ms" } else { 'none' })"
  $res += "playing reports: $(@(Lines | ? { $_ -match 'video playing' }).Count) (" + ((Lines | ? { $_ -match 'video playing' } | % { ($_ -split 'playing ')[1] } | Group-Object | % { "$($_.Count)x $($_.Name)" }) -join ', ') + ")"
  Lines | ? { $_ -match "player's own text|windows ocr" } | % { $res += "  $_" }
  Start-Sleep -Milliseconds 600
  Shot "$S\video-$Name.png"
  if ($CopyLine -and $shown) {
    Set-Clipboard -Value "__none__"
    [Z]::SetCursorPos($o.X + 62, $o.Y + 108) | Out-Null; Start-Sleep -Milliseconds 60
    [Z]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
    foreach ($s in 1..10) { [Z]::SetCursorPos($o.X + 62 + 58 * $s, $o.Y + 108) | Out-Null; Start-Sleep -Milliseconds 25 }
    [Z]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 300
    [Z]::keybd_event(0x11, 0, 0, [UIntPtr]::Zero); [Z]::keybd_event(0x43, 0, 0, [UIntPtr]::Zero); [Z]::keybd_event(0x43, 0, 2, [UIntPtr]::Zero); [Z]::keybd_event(0x11, 0, 2, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 400
    $res += "drag over the title line, Ctrl+C -> [" + (Get-Clipboard -Raw) + "]"
  }
} finally {
  Stop-Process -Id $qc.Id -ErrorAction SilentlyContinue
  [Z]::SetWindowPos($pb.MainWindowHandle, [IntPtr]-2, 0, 0, 0, 0, 0x0013) | Out-Null
  [Z]::PostMessageW($pb.MainWindowHandle, 0x10, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
  if ($null -ne $saved) { Set-Clipboard -Value $saved } else { Set-Clipboard -Value $null }
}
$res
