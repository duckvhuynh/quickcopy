param([string]$Exe, [string]$Title, [string]$Name, [string]$Dir = "$PSScriptRoot\out", [ValidateSet("media", "space")] [string]$PauseWith = "space", [int]$PauseAfter = 3000, [int]$Watch = 8000, [switch]$Resume)
# A desktop player already showing the test video (out\talk.mp4, from
# make_videos.ps1) and playing, e.g.:
#   desktop.ps1 -Exe quickcopy.exe -Title 'VLC media player' -Name vlc -PauseWith space -Resume
#   desktop.ps1 -Exe quickcopy.exe -Title 'Media Player' -Name mediaplayer -PauseWith media -Resume
# Then: start
# QuickCopy, rest the cursor on the video, pause it after a while without
# moving the mouse (media key, or space with the player focused), and record
# what QuickCopy does; with -Resume, play again and check the overlay goes.
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System; using System.Runtime.InteropServices; using System.Text;
public static class D {
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int cx, int cy, uint f);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr e);
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr l);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  public static IntPtr Find(string part) { IntPtr found = IntPtr.Zero; EnumWindows((h, l) => { var s = new StringBuilder(512); GetWindowTextW(h, s, 512); if (IsWindowVisible(h) && s.ToString().Contains(part)) { found = h; return false; } return true; }, IntPtr.Zero); return found; }
  public static void Key(byte vk, uint ext) { keybd_event(vk, 0, ext, UIntPtr.Zero); keybd_event(vk, 0, ext | 2, UIntPtr.Zero); }
}
'@
[D]::SetProcessDpiAwarenessContext([IntPtr]-4) | Out-Null
New-Item -ItemType Directory -Force $Dir | Out-Null; $S = $Dir
$log = "$S\desktop-$Name.log"
$h = [D]::Find($Title)
if ($h -eq [IntPtr]::Zero) { "no window '$Title'"; return }
[D]::SetWindowPos($h, [IntPtr]-1, 40, 30, 1280, 800, 0x0040) | Out-Null
function Toggle { if ($PauseWith -eq "media") { [D]::Key(0xB3, 1) } else { [D]::SetForegroundWindow($h) | Out-Null; Start-Sleep -Milliseconds 100; [D]::Key(0x20, 0) } }
Get-Process quickcopy -ErrorAction SilentlyContinue | Stop-Process -Force
[D]::SetCursorPos(5, 5) | Out-Null
$env:QC_LOG = "1"
$qc = Start-Process $Exe -PassThru -RedirectStandardError $log
Start-Sleep -Milliseconds 1500
function Lines { Get-Content $log -ErrorAction SilentlyContinue }
$res = @()
try {
  [D]::SetCursorPos(680, 400) | Out-Null
  $sw = [Diagnostics.Stopwatch]::StartNew(); $paused = $false
  while ($sw.ElapsedMilliseconds -lt $Watch) {
    if (-not $paused -and $sw.ElapsedMilliseconds -ge $PauseAfter) { Toggle; $paused = $true; $res += "paused ($PauseWith) at $($sw.ElapsedMilliseconds) ms" }
    if (Lines | ? { $_ -match 'overlay \d+x\d+' }) { break }
    Start-Sleep -Milliseconds 50
  }
  $shown = Lines | ? { $_ -match 'overlay \d+x\d+' } | select -First 1
  $res += "overlay: $(if ($shown) { "$shown after $($sw.ElapsedMilliseconds) ms" } else { 'none' })"
  $res += "playing reports before: " + ((Lines | ? { $_ -match 'video playing' } | % { ($_ -split 'playing ')[1] } | Group-Object | % { "$($_.Count)x $($_.Name)" }) -join ', ')
  Lines | ? { $_ -match "player's own text|windows ocr|never settled|surface:" } | select -Last 3 | % { $res += "  $_" }
  Start-Sleep -Milliseconds 500
  $b = New-Object System.Drawing.Bitmap 1280, 800; $g = [System.Drawing.Graphics]::FromImage($b); $g.CopyFromScreen(40, 30, 0, 0, $b.Size); $b.Save("$S\desktop-$Name.png"); $g.Dispose(); $b.Dispose()
  if ($Resume -and $shown) {
    Toggle; $res += "resumed"
    $t = [Diagnostics.Stopwatch]::StartNew()
    while ($t.ElapsedMilliseconds -lt 4000 -and -not (Lines | ? { $_ -match 'revalidate: (video playing|image changed)' })) { Start-Sleep -Milliseconds 50 }
    $res += "after resuming: " + ((Lines | ? { $_ -match 'revalidate' } | select -First 1)) + " after $($t.ElapsedMilliseconds) ms"
  }
} finally {
  Stop-Process -Id $qc.Id -ErrorAction SilentlyContinue
  [D]::SetWindowPos($h, [IntPtr]-2, 0, 0, 0, 0, 0x0013) | Out-Null
}
$res
