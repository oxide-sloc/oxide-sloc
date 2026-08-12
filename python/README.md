# Portable CPython (offline / air-gap)

This directory ships a self-contained, portable CPython interpreter so that
air-gapped Jenkins agents can run the pipeline's Python helper steps without a
system Python — or with one too old to rely on. Like `toolchain/` (Rust) and the
`vendor.tar.gz.*` crate sources, these archives are committed directly to git so
a plain `git clone` is enough to build and run oxide-sloc with no network.

## What these are

Prebuilt, relocatable **CPython 3.14.7** builds from
[astral-sh/python-build-standalone](https://github.com/astral-sh/python-build-standalone),
release `20260807`, `install_only_stripped` variant (the smallest self-contained
layout — no build tooling, debug symbols stripped). No installer, no compiler,
and no system dependencies are required to use them; extract and run.

## Platforms

| Platform | Archive | python-build-standalone target |
|---|---|---|
| Windows x64 | `cpython-3.14-windows-x64.tar.gz` | `x86_64-pc-windows-msvc` |
| Linux x86_64 | `cpython-3.14-linux-x86_64.tar.gz` | `x86_64-unknown-linux-gnu` |

Both archives are under 45 MB (21 MB and 34 MB respectively), so — unlike the
Rust toolchain and vendor archives — they are **not** split into parts.

`python/checksums.sha256` carries the SHA-256 of each archive; the extractor
verifies the relevant line before unpacking. Verify manually with:

```bash
sha256sum -c python/checksums.sha256
```

## Extracted interpreter paths

Each archive unpacks a single top-level `python/` directory. When extracted into
`.tools/` by `ci/jenkins/setup-python.sh`, the interpreter lands at:

| Platform | Interpreter path (relative to repo root) |
|---|---|
| Windows x64 | `.tools/python/python.exe` |
| Linux x86_64 | `.tools/python/bin/python3` |

`ci/jenkins/setup-python.sh` prints exactly this path on the last line of stdout
so the pipeline can capture it (see `pyBin()` in
`ci/jenkins/pipeline-helpers.groovy`). Python is treated as optional — if no
bundle is present for the platform, the setup script exits non-zero and the
caller continues without it.

## How to regenerate

Run once on any machine with internet access, then commit the results:

```bash
bash ci/jenkins/bundle-python.sh          # defaults: release 20260807, CPython 3.14.7
git add python/
git commit -m "chore: update bundled Python"
```

The release date and version are variables at the top of `bundle-python.sh` —
edit them (or pass them as arguments) to pin a different build. The script
downloads both platform archives with the canonical names above and rewrites
`python/checksums.sha256`.
