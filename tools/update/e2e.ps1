param([string]$Old, [string]$New, [string]$Version, [switch]$BadHash, [switch]$Fails, [int]$Port = 8765)
# Runs a real update against a fake GitHub release served from this PC:
#   e2e.ps1 -Old target\release\quickcopy.exe -New <exe built as X.Y.Z> -Version X.Y.Z
#   e2e.ps1 ... -BadHash    (SHA256SUMS.txt lists a wrong hash: nothing installs)
#   e2e.ps1 ... -Fails      (-New quits at once, e.g. whoami.exe: the old exe comes back)
# Copies -Old into a temp app folder, serves latest.json, the -New exe and
# SHA256SUMS.txt with python's http.server, starts the copy with
# QC_UPDATE_API pointing there (QC_INSTANCE keeps it apart from an installed
# QuickCopy, APPDATA from its settings), waits for its first check, picks
# "Update to X.Y.Z" the way the tray menu does (WM_COMMAND 103 to its main
# window) and reports what happened to the folder and the processes.
$ErrorActionPreference = 'Stop'
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class U {
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowExW(IntPtr parent, IntPtr after, string cls, string title);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  // The message-only main window of process `pid`.
  public static IntPtr MainOf(uint pid) {
    IntPtr h = IntPtr.Zero;
    while ((h = FindWindowExW(new IntPtr(-3), h, "QuickCopyMain", null)) != IntPtr.Zero) {
      uint p; GetWindowThreadProcessId(h, out p); if (p == pid) return h;
    }
    return IntPtr.Zero;
  }
}
'@
function Wait-For([scriptblock]$Done, [int]$Seconds) {
  $until = (Get-Date).AddSeconds($Seconds)
  while (-not (& $Done)) { if ((Get-Date) -gt $until) { return $false }; Start-Sleep -Milliseconds 250 }
  $true
}
function Hash([string]$Path) { (Get-FileHash -Algorithm SHA256 $Path).Hash.ToLower() }

$root = Join-Path ([IO.Path]::GetTempPath()) "qc-update-e2e-$PID"
$app = "$root\app"; $srv = "$root\srv"; $log = "$root\quickcopy.log"
Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $app, $srv, "$root\appdata" | Out-Null
$exe = "$app\quickcopy.exe"
Copy-Item $Old $exe
$asset = "quickcopy-$Version-windows-x64.exe"
Copy-Item $New "$srv\$asset"
Set-Content "$srv\QuickCopy-$Version-windows-x64.zip" 'not used by the updater'
$sum = Hash "$srv\$asset"
if ($BadHash) { $sum = ($sum.Substring(0, 63) + $(if ($sum[63] -eq '0') { '1' } else { '0' })) }
$zipSum = Hash "$srv\QuickCopy-$Version-windows-x64.zip"
[IO.File]::WriteAllText("$srv\SHA256SUMS.txt", "$zipSum  QuickCopy-$Version-windows-x64.zip`n$sum  $asset`n")
$base = "http://127.0.0.1:$Port"
$assets = @("QuickCopy-$Version-windows-x64.zip", $asset, 'SHA256SUMS.txt') | ForEach-Object { @{ name = $_; browser_download_url = "$base/$_" } }
@{ tag_name = "v$Version"; html_url = "https://github.com/duckvhuynh/quickcopy/releases/tag/v$Version"; draft = $false; prerelease = $false; assets = $assets } |
  ConvertTo-Json -Depth 4 | Set-Content "$srv\latest.json"

$server = Start-Process python -ArgumentList '-I', '-m', 'http.server', $Port, '--bind', '127.0.0.1', '--directory', $srv -PassThru -WindowStyle Hidden
try {
  $env:QC_UPDATE_API = "$base/latest.json"; $env:QC_INSTANCE = "e2e-$PID"; $env:APPDATA = "$root\appdata"; $env:QC_LOG = '1'
  Start-Sleep -Milliseconds 800
  Write-Output "--- quickcopy --check-update (before)"
  & $exe --check-update | Write-Output
  $oldHash = Hash $exe
  $qc = Start-Process $exe -PassThru -RedirectStandardError $log
  Write-Output "--- started $exe (pid $($qc.Id)); waiting for its first check"
  if (-not (Wait-For { (Test-Path $log) -and (Select-String -Quiet -Path $log -Pattern 'update: latest') } 30)) { throw 'no update check within 30 s' }
  $main = [U]::MainOf([uint32]$qc.Id)
  [U]::PostMessageW($main, 0x111, [IntPtr]103, [IntPtr]::Zero) | Out-Null
  Write-Output "--- picked 'Update to $Version' (WM_COMMAND 103 to $main)"
  if ($BadHash -or $Fails) {
    Wait-For { Select-String -Quiet -Path $log -Pattern "update: (HashMismatch|Launch)" } 30 | Out-Null
    Start-Sleep -Seconds 1
    Write-Output "old process still running: $(-not $qc.HasExited)"
  } else {
    if (-not (Wait-For { $qc.HasExited } 30)) { throw 'the old process did not quit' }
    Wait-For { -not (Test-Path "$exe.old") } 15 | Out-Null
  }
  Start-Sleep -Seconds 1
  Write-Output '--- app folder'
  Get-ChildItem $app | ForEach-Object { "{0,-22} {1}" -f $_.Name, (Hash $_.FullName) } | Write-Output
  Write-Output "old exe sha256: $oldHash"
  Write-Output "new exe sha256: $(Hash $New)"
  Write-Output '--- quickcopy processes running from the app folder'
  Get-CimInstance Win32_Process -Filter "Name = 'quickcopy.exe'" | Where-Object { $_.ExecutablePath -eq $exe } |
    ForEach-Object { "pid $($_.ProcessId)  $($_.CommandLine)" } | Write-Output
  Write-Output '--- quickcopy --check-update (after)'
  & $exe --check-update | Write-Output
  Write-Output '--- log (old process, then the new one: both write to it)'
  Get-Content $log | Select-String -Pattern 'update|running' | ForEach-Object { $_.Line } | Write-Output
} finally {
  Get-CimInstance Win32_Process -Filter "Name = 'quickcopy.exe'" | Where-Object { $_.ExecutablePath -like "$app\*" } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force }
  Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue
}
