#!/usr/bin/env bash
# Rewrite the committed scoop / homebrew / chocolatey installer manifests to a
# given release version + asset hashes, so they stop drifting behind the
# workspace version. Invoked automatically by .github/workflows/update-dist.yml
# after each release (commit-installer-manifests job), and runnable by hand.
#
# winget and the nfpm deb/rpm are NOT handled here — they are already drift-proof
# (winget is auto-submitted per tag; installer/nfpm.yaml uses ${VERSION} templating).
#
# Usage:
#   update-installer-manifests.sh <version> <windows_zip_sha256> <source_tgz_sha256>
#
#   <version>             Release version, with or without a leading v (e.g. 1.6.27)
#   <windows_zip_sha256>  sha256 of oxide-sloc-windows-x64.zip  (scoop + chocolatey)
#   <source_tgz_sha256>   sha256 of the GitHub source tarball v<version>.tar.gz (homebrew)
#
# Obtain the hashes from a published release:
#   ZIP_SHA=$(gh release download v<version> --repo oxide-sloc/oxide-sloc \
#               --pattern SHA256SUMS-dist.txt --output - \
#               | awk '/oxide-sloc-windows-x64\.zip$/{print $1}')
#   SRC_SHA=$(curl -fsSL \
#               https://github.com/oxide-sloc/oxide-sloc/archive/refs/tags/v<version>.tar.gz \
#               | sha256sum | cut -d' ' -f1)

set -euo pipefail

VERSION="${1:-}"; ZIP_SHA="${2:-}"; SRC_SHA="${3:-}"
if [[ -z "$VERSION" || -z "$ZIP_SHA" || -z "$SRC_SHA" ]]; then
  echo "usage: $0 <version> <windows_zip_sha256> <source_tgz_sha256>" >&2
  exit 2
fi
VERSION="${VERSION#v}"

[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z]+)*$ ]] \
  || { echo "error: invalid version '$VERSION'" >&2; exit 2; }
[[ "$ZIP_SHA" =~ ^[a-fA-F0-9]{64}$ ]] \
  || { echo "error: invalid windows_zip_sha256 '$ZIP_SHA'" >&2; exit 2; }
[[ "$SRC_SHA" =~ ^[a-fA-F0-9]{64}$ ]] \
  || { echo "error: invalid source_tgz_sha256 '$SRC_SHA'" >&2; exit 2; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

PY="$(command -v python3 || command -v python || true)"
[[ -n "$PY" ]] || { echo "error: python3/python not found on PATH" >&2; exit 1; }

VERSION="$VERSION" ZIP_SHA="$ZIP_SHA" SRC_SHA="$SRC_SHA" "$PY" - <<'PY'
import json, os, re, pathlib

v       = os.environ["VERSION"]
zip_sha = os.environ["ZIP_SHA"].lower()
src_sha = os.environ["SRC_SHA"].lower()
base    = "https://github.com/oxide-sloc/oxide-sloc"

# ── scoop (JSON) ────────────────────────────────────────────────────────────
p = pathlib.Path("installer/scoop/oxide-sloc.json")
d = json.loads(p.read_text(encoding="utf-8"))
d["version"] = v
d["url"]     = f"{base}/releases/download/v{v}/oxide-sloc-windows-x64.zip"
d["hash"]    = f"sha256:{zip_sha}"
# autoupdate.url stays templated with scoop's $version placeholder. Fix the hash
# source: there is no per-asset .zip.sha256 sidecar on the release, only the
# aggregate SHA256SUMS-dist.txt — extract the hash from it by regex.
au = d.setdefault("autoupdate", {})
au["url"]  = f"{base}/releases/download/v$version/oxide-sloc-windows-x64.zip"
au["hash"] = {
    "url":   f"{base}/releases/download/v$version/SHA256SUMS-dist.txt",
    "regex": r"$sha256\s+$basename",
}
# ensure_ascii=False keeps the em-dash in the description literal (not —).
p.write_text(json.dumps(d, indent=4, ensure_ascii=False) + "\n", encoding="utf-8")

# ── homebrew (Ruby formula) — builds from the GitHub source tarball ─────────
p = pathlib.Path("installer/homebrew/oxide-sloc.rb")
t = p.read_text(encoding="utf-8")
t = re.sub(r'(^\s*url\s+")[^"]*(")',
           rf'\g<1>{base}/archive/refs/tags/v{v}.tar.gz\g<2>', t, count=1, flags=re.M)
t = re.sub(r'(^\s*sha256\s+")[0-9a-fA-F]*(")',
           rf'\g<1>{src_sha}\g<2>', t, count=1, flags=re.M)
p.write_text(t, encoding="utf-8")

# ── chocolatey nuspec ───────────────────────────────────────────────────────
p = pathlib.Path("installer/chocolatey/oxide-sloc.nuspec")
t = p.read_text(encoding="utf-8")
t = re.sub(r"<version>[^<]*</version>", f"<version>{v}</version>", t, count=1)
t = re.sub(r"(releases/tag/v)[0-9][^<]*", rf"\g<1>{v}", t)
p.write_text(t, encoding="utf-8")

# ── chocolatey install script — downloads the windows zip ───────────────────
p = pathlib.Path("installer/chocolatey/tools/chocolateyinstall.ps1")
t = p.read_text(encoding="utf-8")
t = re.sub(r"(\$version\s*=\s*')[^']*(')",  rf"\g<1>{v}\g<2>",      t, count=1)
t = re.sub(r"(\$checksum\s*=\s*')[^']*(')", rf"\g<1>{zip_sha}\g<2>", t, count=1)
p.write_text(t, encoding="utf-8")

print(f"Updated scoop/homebrew/chocolatey manifests to v{v}")
PY
