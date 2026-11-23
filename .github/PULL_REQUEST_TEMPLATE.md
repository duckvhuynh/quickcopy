## What changes for the user

<!-- What QuickCopy does differently after this change. -->

## Cause and fix

<!-- For a bug: what went wrong and why. For a feature: how it works. -->

## Measurements

<!-- Numbers before and after, and how you got them. Delete lines that do
     not apply. -->

- Unit tests (`cargo test --release`):
- OCR corpus (`tools/corpus/run_corpus.py`), Vietnamese and `--english`:
- Selection cases (`tools/selection/run_cases.py`):
- Checked in the running app:

## Checklist

- [ ] One change in this pull request.
- [ ] Comments say why, not what.
- [ ] README, docs and CHANGELOG are updated if users see the change.
- [ ] `python tools/third_party.py` was run if `Cargo.lock` changed.
