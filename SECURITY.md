# Security policy

## Supported versions

Security fixes go into the latest release only. QuickCopy updates itself
from GitHub Releases, so update to the latest version before you report a
problem.

| Version | Supported |
| --- | --- |
| Latest release | Yes |
| Older releases | No |

## Report a vulnerability

Do not open a public issue for a security problem.

Report it privately through GitHub:

1. Go to the [Security tab](https://github.com/duckvhuynh/quickcopy/security)
   of this repository.
2. Select **Report a vulnerability**.
3. Describe the problem, the version of QuickCopy and of Windows, and the
   steps to reproduce it.

You get an answer in the advisory. When a fix is released, the advisory is
published and credits you, unless you ask not to be named.

## Scope

These are in scope:

- `quickcopy.exe` and the release files on GitHub Releases.
- The update check and the installation of updates.
- The handling of text that QuickCopy sends for translation.
- The scripts in `tools/` and the GitHub Actions workflows.

These are out of scope:

- The VieSVTR model and ONNX Runtime. Report problems in them to their
  authors.
- The Google Translate endpoint and Windows components.
