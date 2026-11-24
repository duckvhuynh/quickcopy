<#
.SYNOPSIS
Builds QuickCopy and writes the release assets to dist/.

.DESCRIPTION
dist/ is wiped and rebuilt on every run, so the same commit gives the same
file list. Assets, for version X.Y.Z from Cargo.toml:

  QuickCopy-X.Y.Z-windows-x64.zip   QuickCopy/ with quickcopy.exe, the
                                    Vietnamese tone-mark table, README.txt and
                                    license files
  quickcopy-X.Y.Z-windows-x64.exe   the bare exe (the in-app updater uses it)
  SHA256SUMS.txt                    "<sha256>  <asset name>" per asset

The VieSVTR model (.onnx) is never packaged: it is CC BY-NC, and the app
downloads it at its first start (see packaging/README.txt).

.PARAMETER NoBuild
Package the existing target/release/quickcopy.exe instead of building. The
release workflow uses this to sign the exe between build and packaging.
#>
[CmdletBinding()]
param(
    [switch]$NoBuild
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$meta = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
$version = ($meta.packages | Where-Object name -eq 'quickcopy').version

if (-not $NoBuild) {
    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
}

$exe = Join-Path $root 'target/release/quickcopy.exe'
$syllables = Join-Path $root 'models/viesvtr/vi_syllables.bin'
foreach ($required in $exe, $syllables) {
    if (-not (Test-Path $required -PathType Leaf)) { throw "missing $required" }
}

# Zip entry name -> source file. README and licenses are optional so the
# script works before the docs land; the exe and tone-mark table are not.
$entries = [ordered]@{
    'QuickCopy/quickcopy.exe'                    = $exe
    'QuickCopy/models/viesvtr/vi_syllables.bin' = $syllables
}
$entries['QuickCopy/README.txt'] = Join-Path $root 'packaging/README.txt'
foreach ($doc in 'LICENSE-MIT', 'LICENSE-APACHE', 'THIRD-PARTY-NOTICES.md') {
    $path = Join-Path $root $doc
    if (Test-Path $path -PathType Leaf) { $entries["QuickCopy/$doc"] = $path }
}

$dist = Join-Path $root 'dist'
if (Test-Path $dist) { Remove-Item $dist -Recurse -Force }
New-Item -ItemType Directory $dist | Out-Null

# Entries are written in a fixed order with the commit's UTC time, not the
# build time, so two runs on one commit give byte-identical zips when the
# exe is identical.
$stamp = [DateTimeOffset]::FromUnixTimeSeconds([long](git log -1 --format=%ct))

$zipName = "QuickCopy-$version-windows-x64.zip"
$exeName = "quickcopy-$version-windows-x64.exe"
$zip = [System.IO.Compression.ZipFile]::Open((Join-Path $dist $zipName), 'Create')
try {
    foreach ($name in $entries.Keys) {
        $entry = $zip.CreateEntry($name, [System.IO.Compression.CompressionLevel]::Optimal)
        $entry.LastWriteTime = $stamp
        $out = $entry.Open()
        try {
            $in = [System.IO.File]::OpenRead($entries[$name])
            try { $in.CopyTo($out) } finally { $in.Dispose() }
        } finally { $out.Dispose() }
    }
} finally { $zip.Dispose() }

Copy-Item $exe (Join-Path $dist $exeName)

# LF endings and no BOM, so `sha256sum -c SHA256SUMS.txt` works too.
$sums = foreach ($name in $zipName, $exeName) {
    $hash = (Get-FileHash (Join-Path $dist $name) -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $name"
}
[System.IO.File]::WriteAllText((Join-Path $dist 'SHA256SUMS.txt'), (($sums -join "`n") + "`n"))

Write-Host "QuickCopy $version"
Get-ChildItem $dist | Format-Table Name, Length -AutoSize | Out-String | Write-Host
Write-Host ($sums -join "`n")
