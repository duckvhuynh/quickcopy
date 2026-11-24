# Releasing QuickCopy

The version lives in one place, `version` in `Cargo.toml`. Tags are `vX.Y.Z`,
and the release workflow stops if the tag and `Cargo.toml` disagree.

Each release has three assets:

| Asset | What it is |
| --- | --- |
| `QuickCopy-X.Y.Z-windows-x64.zip` | Folder `QuickCopy/` with `quickcopy.exe`, `models/viesvtr/vi_syllables.bin`, `README.txt` (the short user guide in `packaging/README.txt`) and the license files |
| `quickcopy-X.Y.Z-windows-x64.exe` | The bare exe, which the in-app updater downloads |
| `SHA256SUMS.txt` | One `<sha256>  <asset name>` line per asset |

The VieSVTR model (`viesvtr_v7.onnx`) is never shipped. It is licensed
CC BY-NC 4.0, so QuickCopy downloads it at its first start from its
author's Hugging Face page, at the revision and SHA-256 pinned in
`src/model.rs`. `README.txt` in the zip names the model and its license.

## Cut a release

1. Set `version = "X.Y.Z"` in `Cargo.toml` and run `cargo build` so
   `Cargo.lock` picks it up.
2. In `CHANGELOG.md`, move the `Unreleased` entries under a new
   `## [X.Y.Z] - YYYY-MM-DD` heading. That section becomes the release notes.
   Without it, GitHub generates notes from the merged PRs.
3. Dry run the packaging locally:

   ```powershell
   ./scripts/package.ps1
   ```

   It builds with `--locked`, rebuilds `dist/` from scratch and prints the
   assets and their hashes.
4. Commit, then tag and push:

   ```powershell
   git commit -am "Release X.Y.Z"
   git tag vX.Y.Z
   git push origin main vX.Y.Z
   ```

5. The `Release` workflow builds, tests, signs (when signing is set up),
   packages, writes a build provenance attestation for every asset and
   creates a **draft** release. Open it on GitHub, check the notes and
   assets, and publish it. Re-running the workflow on the same tag replaces
   the draft's assets.
6. Update winget and Scoop (below).

If the workflow fails on the version check, delete the tag
(`git push --delete origin vX.Y.Z` and `git tag -d vX.Y.Z`), fix
`Cargo.toml`, and tag again.

## Verify a download

Hashes. Compare with the matching line of `SHA256SUMS.txt`:

```powershell
Get-FileHash .\QuickCopy-X.Y.Z-windows-x64.zip -Algorithm SHA256
```

Provenance. This proves the file was built by this repository's release
workflow from the tagged commit, not uploaded by hand:

```powershell
gh attestation verify .\QuickCopy-X.Y.Z-windows-x64.zip --repo duckvhuynh/quickcopy
```

Signature, once releases are signed:

```powershell
Get-AuthenticodeSignature .\QuickCopy\quickcopy.exe
```

## Code signing

Unsigned, the exe triggers SmartScreen's "Windows protected your PC" until
it builds download reputation. Users click **More info**, then **Run
anyway**. Signing does not skip that step on day one either: since 2024
even EV certificates start without reputation. What a trusted signature
buys is that reputation accrues to the publisher across releases instead
of resetting with every new exe, and that the publisher name is shown
instead of "Unknown publisher".

| Option | Cost | Publisher shown | Fit |
| --- | --- | --- | --- |
| [SignPath Foundation](https://signpath.org) | Free for open source projects they accept | SignPath Foundation, with the project named in the signature | Signs builds from public CI only, which this workflow already is. They review the project and its release process before accepting it |
| [Azure Artifact Signing](https://learn.microsoft.com/azure/trusted-signing/) (formerly Trusted Signing) | About 10 USD a month | Your validated name | Microsoft-run, reputation builds quickly. Identity validation is limited to some countries for individuals; check the current list before relying on it |
| OV or EV certificate from a CA | Roughly 200 to 600 USD a year | Your validated name | Keys must live on a hardware token or cloud HSM since 2023, so a PFX in a CI secret is no longer how these are issued |
| Self-signed certificate | Free | Nothing users trust | Windows does not chain it to a trusted root, so SmartScreen treats the exe as unsigned. It only helps on machines where you install the certificate yourself |

**Recommendation:** apply to SignPath Foundation. It costs nothing, accepts individual
maintainers, and its model (sign only what public CI built from a
tagged commit) matches this workflow and the provenance attestations. Use
Azure Artifact Signing instead if your country is eligible and you want
"duckvhuynh" as the publisher. Ship the first release unsigned rather than
wait; SignPath may want to see an existing release.

### Turn signing on

The workflow signs `target/release/quickcopy.exe` before packaging, so the
zip, the bare exe and `SHA256SUMS.txt` all carry the signed exe. With no
signing secrets, those steps are skipped. Azure wins if both are set.

SignPath: create project `quickcopy` with signing policy `release-signing`
and an artifact configuration that signs `quickcopy.exe` inside the uploaded
zip, link the GitHub repository as a trusted build system, then add:

- secret `SIGNPATH_API_TOKEN`
- variable `SIGNPATH_ORGANIZATION_ID`

Azure Artifact Signing: create a signing account and certificate profile,
give an app registration the Certificate Profile Signer role on it, then
add these secrets:

- `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`
- `AZURE_SIGNING_ENDPOINT` (for example `https://weu.codesigning.azure.net/`)
- `AZURE_SIGNING_ACCOUNT`, `AZURE_CERTIFICATE_PROFILE`

A signed run ends with a "Check signature" step that fails the release if
the exe's signature is not valid.

## winget

`packaging/winget/` holds templates for `duckvhuynh.QuickCopy` with
`__VERSION__` and `__ZIP_SHA256__` placeholders. For the first submission,
fill them from the published release's hashes and validate:

```powershell
$v = 'X.Y.Z'
gh release download "v$v" --repo duckvhuynh/quickcopy --pattern SHA256SUMS.txt --dir $env:TEMP\qc --clobber
$sha = ((Select-String "  QuickCopy-$v-windows-x64.zip$" $env:TEMP\qc\SHA256SUMS.txt).Line -split ' ')[0].ToUpper()
$out = "manifests\d\duckvhuynh\QuickCopy\$v"
New-Item -ItemType Directory -Force $out | Out-Null
Get-ChildItem packaging\winget\*.yaml | ForEach-Object {
  (Get-Content $_ -Raw).Replace('__VERSION__', $v).Replace('__ZIP_SHA256__', $sha) |
    Set-Content -NoNewline (Join-Path $out $_.Name)
}
winget validate --manifest $out
```

Open a PR adding that folder to
[microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs). Later
versions are one command with
[wingetcreate](https://github.com/microsoft/winget-create):

```powershell
wingetcreate update duckvhuynh.QuickCopy --version X.Y.Z --urls https://github.com/duckvhuynh/quickcopy/releases/download/vX.Y.Z/QuickCopy-X.Y.Z-windows-x64.zip --submit
```

The installer manifest sets `ArchiveBinariesDependOnPath`, so winget puts
the install folder on `PATH` instead of linking the exe. QuickCopy looks for
`models\` next to the exe, which a link would hide.

## Scoop

`packaging/scoop/quickcopy.json` is a manifest for a Scoop bucket. Its
`checkver` follows GitHub releases and `autoupdate` reads the hash from
`SHA256SUMS.txt`. Before the first release is published its `hash` is a
placeholder; in the bucket, run Scoop's `checkver.ps1 quickcopy -u` to fill
the version and hash from the latest release.
