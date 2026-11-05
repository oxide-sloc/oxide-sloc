# Generates static, offline-safe fallback SVG badges under docs/badges/.
#
# These are the local fallbacks referenced by the README's <img onerror=...>
# handlers: online (GitHub/crates.io) the live badge services render as usual;
# on an air-gapped machine the live fetch fails and the browser swaps in the
# committed SVG below instead of showing a broken-image icon.
#
# The SVG template + glyph-width table mirror sloc-web::render_badge_svg so the
# fallbacks look identical to the app's own /badge endpoint. Re-run after a
# version bump to refresh version.svg / crates.svg:
#
#   pwsh scripts/internal/gen-offline-badges.ps1
#
# Status badges (CI, coverage, scorecard, ...) render a neutral gray "offline"
# rather than asserting a pass/fail state that cannot be known without network.
#
# -Version overrides the version stamped into version.svg / crates.svg. Omit it
# (the normal local case, e.g. the /release flow) to read the current workspace
# version from Cargo.toml. CI passes the release tag's version explicitly so the
# badges are deterministic even if main has already advanced past the bump.

param(
    [string]$Version
)

$ErrorActionPreference = 'Stop'
$outDir = Join-Path $PSScriptRoot '..\..\docs\badges'
$outDir = [System.IO.Path]::GetFullPath($outDir)
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

# Pull the workspace version (or use the explicit -Version) so version/crates
# badges stay in sync.
if ($Version) {
    $ver = $Version.TrimStart('v')
    if ($ver -notmatch '^[0-9]+\.[0-9]+\.[0-9]+') {
        throw "Invalid -Version '$Version' — expected MAJOR.MINOR.PATCH (optionally v-prefixed)"
    }
} else {
    $cargo = Join-Path $PSScriptRoot '..\..\Cargo.toml'
    $ver = (Select-String -Path $cargo -Pattern '^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"' |
        Select-Object -First 1).Matches[0].Groups[1].Value
    if (-not $ver) { throw "Could not read version from $cargo" }
}

function Get-CharWidth([char]$c) {
    switch ($c) {
        { $_ -in 'f', 'i', 'j', 'l', 'r', 't' } { return 5.0 }
        { $_ -in 'm', 'w' } { return 9.0 }
        ' ' { return 4.0 }
        default { return 6.5 }
    }
}
function Get-TextPx([string]$s) {
    $sum = 0.0
    foreach ($ch in $s.ToCharArray()) { $sum += Get-CharWidth $ch }
    return [int][math]::Ceiling($sum)
}
function Esc([string]$s) {
    return ($s -replace '&', '&amp;' -replace '<', '&lt;' -replace '>', '&gt;' -replace '"', '&quot;')
}
function New-Badge([string]$file, [string]$label, [string]$value, [string]$color) {
    $lw = (Get-TextPx $label) + 20
    $rw = (Get-TextPx $value) + 20
    $total = $lw + $rw
    $lx = [int]($lw / 2)
    $rx = [int]($lw + $rw / 2)
    $le = Esc $label; $ve = Esc $value; $ce = Esc $color
    $svg = @"
<svg xmlns="http://www.w3.org/2000/svg" width="$total" height="20" role="img" aria-label="$le : $ve">
  <title>$le : $ve</title>
  <rect width="$total" height="20" fill="#555"/>
  <rect x="$lw" width="$rw" height="20" fill="$ce"/>
  <g fill="#fff" text-anchor="middle" font-family="DejaVu Sans,Verdana,Geneva,sans-serif" font-size="11">
    <text x="$lx" y="14" fill="#010101" fill-opacity=".3">$le</text>
    <text x="$lx" y="13">$le</text>
    <text x="$rx" y="14" fill="#010101" fill-opacity=".3">$ve</text>
    <text x="$rx" y="13">$ve</text>
  </g>
</svg>
"@
    $path = Join-Path $outDir $file
    [System.IO.File]::WriteAllText($path, $svg.Trim() + "`n")
    Write-Host "  $file"
}

$GRAY = '#9f9f9f'; $BLUE = '#007ec6'; $ORANGE = '#fe7d37'

Write-Host "Writing offline badge fallbacks to $outDir (v$ver):"
# Live-status badges: value is unknowable offline -> honest neutral snapshot.
New-Badge 'ci.svg'                'CI'                'offline' $GRAY
New-Badge 'release.svg'           'Release'           'offline' $GRAY
New-Badge 'docker.svg'            'Docker'            'offline' $GRAY
New-Badge 'codecov.svg'           'codecov'           'offline' $GRAY
New-Badge 'ossf-bestpractices.svg' 'best practices'   'offline' $GRAY
New-Badge 'ossf-scorecard.svg'    'openssf scorecard' 'offline' $GRAY
New-Badge 'docs.svg'              'docs.rs'           'offline' $GRAY
# Static facts: known without network.
New-Badge 'version.svg'           'release'           "v$ver"   $BLUE
New-Badge 'crates.svg'            'crates.io'         "v$ver"   $ORANGE
New-Badge 'license.svg'           'license'           'AGPL-3.0-or-later' $BLUE
New-Badge 'mcp.svg'               'MCP'               'server'  $ORANGE
Write-Host "Done."
