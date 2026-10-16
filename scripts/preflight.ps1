<#
.SYNOPSIS
  Verify a Windows host can install, build, and/or run oxide-sloc, and that it is
  compatible with a target server, BEFORE wiring oxide-sloc into a workflow.

.DESCRIPTION
  The PowerShell twin of scripts/preflight.sh for Windows hosts where bash is not
  available (Constrained Language Mode is tolerated). Standalone and dependency-free
  (Windows PowerShell 5.1+). Copy this ONE file into any project to confirm the
  prerequisites are present and fail fast with actionable guidance.

.PARAMETER Mode
  prebuilt | source | run — what the host is expected to do. Defaults to 'run' when
  an oxide-sloc.exe is already on PATH, otherwise 'prebuilt'.

.PARAMETER ServerUrl
  Probe a running `oxide-sloc serve` at this URL (GET /healthz and /api/version).

.PARAMETER RequireVersion
  Assert the resolved binary (and the server, if probed) report this version. A
  hard failure on mismatch — use it to pin a reproducible tool. Leading 'v' tolerated.

.PARAMETER Json
  Emit a machine-readable JSON summary on the last line.

.PARAMETER Quiet
  Suppress per-check lines; only the summary is printed.

.OUTPUTS
  Exit code 0 when all hard checks pass, 1 on a hard failure, 2 on bad usage.
#>
[CmdletBinding()]
param(
    [ValidateSet('prebuilt', 'source', 'run', '')]
    [string]$Mode = '',
    [string]$ServerUrl = '',
    [string]$RequireVersion = '',
    [switch]$Json,
    [switch]$Quiet
)

$ErrorActionPreference = 'Continue'
$script:Fails = 0; $script:Warns = 0; $script:Passes = 0

function Write-Ok   ($m) { $script:Passes++; if (-not $Quiet) { Write-Host "[ok]   $m" } }
function Write-Warn ($m) { $script:Warns++;  if (-not $Quiet) { Write-Host "[warn] $m" -ForegroundColor Yellow } }
function Write-Fail ($m) { $script:Fails++;  if (-not $Quiet) { Write-Host "[fail] $m" -ForegroundColor Red } }
function Write-Note ($m) { if (-not $Quiet) { Write-Host "[info] $m" } }
function Test-Have  ($n) { [bool](Get-Command $n -ErrorAction SilentlyContinue) }
function ConvertTo-NormVer ($v) { if ($v) { $v.TrimStart('v', 'V') } else { '' } }

$RequireVersion = ConvertTo-NormVer $RequireVersion
$verRegex = '[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.\-]+)?'

# ── Platform / repo root ──────────────────────────────────────────────────────
$arch = if ([Environment]::Is64BitOperatingSystem) {
    if ($env:PROCESSOR_ARCHITECTURE -match 'ARM') { 'arm64' } else { 'x86_64' }
} else { 'x86' }
Write-Note "Host: platform=windows arch=$arch"

$RepoRoot = Split-Path -Parent $PSScriptRoot

# ── Resolve an oxide-sloc binary, if reachable ────────────────────────────────
$Binary = ''
$onPath = Get-Command 'oxide-sloc.exe' -ErrorAction SilentlyContinue
if ($onPath) {
    $Binary = $onPath.Source
} elseif (Test-Path (Join-Path $RepoRoot 'target\release\oxide-sloc.exe')) {
    $Binary = Join-Path $RepoRoot 'target\release\oxide-sloc.exe'
}

if (-not $Mode) { $Mode = if ($Binary) { 'run' } else { 'prebuilt' } }
Write-Note "Mode: $Mode"

# ── git / curl ────────────────────────────────────────────────────────────────
if (Test-Have git) {
    Write-Ok ("git present ({0})" -f ((git --version 2>$null) | Select-Object -First 1))
} else {
    Write-Warn 'git not found — cloning remote repos and git hotspots/attribution are unavailable (analyzing a local path still works)'
}

# ── Mode-specific prerequisites ───────────────────────────────────────────────
switch ($Mode) {
    'prebuilt' {
        # tar.exe (bsdtar) ships with Windows 10/11; Expand-Archive is built in.
        if ((Test-Have tar) -or (Get-Command Expand-Archive -ErrorAction SilentlyContinue)) {
            Write-Ok 'archive extraction available (tar.exe / Expand-Archive)'
        } else {
            Write-Fail 'no tar.exe or Expand-Archive available to extract the dist/ archive'
        }
        $distDir = Join-Path $RepoRoot 'dist'
        if (Test-Path $distDir) {
            $cands = @('oxide-sloc-windows-x64.tar.gz', 'oxide-sloc-windows-x64.zip') |
                ForEach-Object { Join-Path $distDir $_ } | Where-Object { Test-Path $_ }
            if ($cands) {
                Write-Ok ("prebuilt dist archive present ({0})" -f (Split-Path -Leaf $cands[0]))
            } else {
                Write-Warn 'no dist/ archive for windows-x64 — a prebuilt run will have to fall back to a source build'
            }
        }
    }
    'source' {
        if (Test-Have cargo) {
            Write-Ok ("cargo present ({0})" -f ((cargo --version 2>$null) | Select-Object -First 1))
        } elseif ((Test-Path (Join-Path $RepoRoot 'toolchain')) -and
                  (Get-ChildItem (Join-Path $RepoRoot 'toolchain') -Filter 'rust-toolchain-*.tar.gz*' -ErrorAction SilentlyContinue)) {
            Write-Ok 'no cargo on PATH, but a bundled offline Rust toolchain is present under toolchain/'
        } elseif (Test-Have rustup) {
            Write-Warn 'no cargo on PATH but rustup is present — the toolchain will be installed on first build'
        } else {
            Write-Fail 'source build requires a Rust toolchain: install rustc+cargo, run inside the oxide-sloc repo with the bundled toolchain/, or use -Mode prebuilt'
        }
        if ((Get-ChildItem $RepoRoot -Filter 'vendor.tar.gz.*' -ErrorAction SilentlyContinue) -or
            (Test-Path (Join-Path $RepoRoot 'vendor'))) {
            Write-Ok 'vendored crate sources present (offline build supported)'
        } else {
            Write-Warn 'no vendored crates — the build will need network access to crates.io'
        }
        try {
            $free = (Get-PSDrive -Name ((Get-Item $RepoRoot).PSDrive.Name) -ErrorAction Stop).Free
            $freeGb = [math]::Floor($free / 1GB)
            if ($freeGb -lt 3) {
                Write-Warn "only $freeGb GB free on the build volume — a source build may run out of space"
            } else {
                Write-Ok "disk space OK ($freeGb GB free)"
            }
        } catch {}
    }
}

# ── Optional: a browser for PDF export ────────────────────────────────────────
$browser = ''
foreach ($b in @($env:SLOC_BROWSER,
                 "$env:ProgramFiles\Google\Chrome\Application\chrome.exe",
                 "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe",
                 "$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe")) {
    if ($b -and (Test-Path $b)) { $browser = $b; break }
}
if ($browser) {
    Write-Ok "Chromium-based browser found for PDF export ($browser)"
} else {
    Write-Warn 'no Chromium-based browser found — HTML/CSV/JSON still work; PDF export falls back to the pure-Rust writer (set SLOC_BROWSER to override)'
}

# ── Binary presence + version ─────────────────────────────────────────────────
$ResolvedVersion = ''
if ($Binary) {
    $raw = (& $Binary --version 2>$null | Select-Object -Last 1)
    if ($raw -match $verRegex) { $ResolvedVersion = $Matches[0] }
    if ($ResolvedVersion) {
        Write-Ok "oxide-sloc binary resolved: $Binary (v$ResolvedVersion)"
    } else {
        Write-Ok "oxide-sloc binary resolved: $Binary"
    }
} elseif ($Mode -eq 'run') {
    Write-Fail 'run mode requires an oxide-sloc.exe on PATH (or at target\release\) — none found'
} else {
    Write-Note "no oxide-sloc binary resolvable yet (produced by the $Mode step; the version pin is checked after build)"
}

# ── Version pin ───────────────────────────────────────────────────────────────
if ($RequireVersion) {
    if ($ResolvedVersion) {
        if ($ResolvedVersion -eq $RequireVersion) {
            Write-Ok "version pin satisfied: binary is $ResolvedVersion"
        } else {
            Write-Fail "version pin mismatch: required $RequireVersion but binary is $ResolvedVersion"
        }
    } else {
        Write-Warn "version pin $RequireVersion requested but no binary is resolvable yet to verify"
    }
}

# ── Optional: probe a running server ──────────────────────────────────────────
$ServerVersion = ''
if ($ServerUrl) {
    $base = $ServerUrl.TrimEnd('/')
    try {
        $h = Invoke-WebRequest -Uri "$base/healthz" -TimeoutSec 8 -UseBasicParsing -ErrorAction Stop
        if ($h.StatusCode -eq 200) { Write-Ok "server /healthz reachable at $base (HTTP 200)" }
        else { Write-Fail "server /healthz at $base returned HTTP $($h.StatusCode)" }
    } catch {
        Write-Fail "server /healthz at $base unreachable — is 'oxide-sloc serve' running there? ($($_.Exception.Message))"
    }
    try {
        $v = Invoke-WebRequest -Uri "$base/api/version" -TimeoutSec 8 -UseBasicParsing -ErrorAction Stop
        if ($v.Content -match "`"version`"\s*:\s*`"($verRegex)`"") { $ServerVersion = $Matches[1] }
        if ($ServerVersion) {
            Write-Ok "server reports oxide-sloc v$ServerVersion"
            if ($ResolvedVersion -and (($ResolvedVersion -split '\.')[0] -ne ($ServerVersion -split '\.')[0])) {
                Write-Warn "major-version skew: local v$ResolvedVersion vs server v$ServerVersion — API payloads may differ"
            }
            if ($RequireVersion -and $ServerVersion -ne $RequireVersion) {
                Write-Fail "server version $ServerVersion does not match the required pin $RequireVersion"
            }
        } else {
            Write-Warn "could not read a version from $base/api/version"
        }
    } catch {
        Write-Warn "could not query $base/api/version ($($_.Exception.Message))"
    }
}

# ── Summary ───────────────────────────────────────────────────────────────────
$status = if ($script:Fails -gt 0) { 'fail' } else { 'pass' }
if ($Json) {
    [pscustomobject]@{
        status = $status; mode = $Mode; platform = 'windows'; arch = $arch
        binary = $Binary; binary_version = $ResolvedVersion; server_version = $ServerVersion
        passes = $script:Passes; warnings = $script:Warns; failures = $script:Fails
    } | ConvertTo-Json -Compress | Write-Output
}
if ($script:Fails -gt 0) {
    if (-not $Quiet) { Write-Host "`nPreflight FAILED — $($script:Fails) hard check(s) failed, $($script:Warns) warning(s)." -ForegroundColor Red }
    exit 1
}
if (-not $Quiet) { Write-Host "`nPreflight PASSED — $($script:Passes) check(s) OK, $($script:Warns) warning(s)." -ForegroundColor Green }
exit 0
