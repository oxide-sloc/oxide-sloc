<#
.SYNOPSIS
    Native-PowerShell offline installer/build for oxide-sloc on Windows.

.DESCRIPTION
    A bash-free backup for `scripts/internal/install.sh`. Every step here runs in
    pure PowerShell using tools that ship with Windows 10/11 -- `tar.exe` (bsdtar
    in System32), `Get-FileHash` (SHA-256), and `Expand-Archive` -- so it works on
    a locked-down, air-gapped Windows host with NO Git Bash and NO admin.

    It reproduces the offline flow of install.sh:
      1. If oxide-sloc.exe is already built and current -> done.
      2. If a pre-built binary is committed in dist\ -> extract it (no build).
      3. Otherwise bootstrap the bundled Rust toolchain (checksum-verified,
         reassembled from split parts, extracted with tar.exe), reassemble +
         verify the vendor sources, write .cargo\config.toml, and
         `cargo build --release --offline`.

    THE ONE THING POWERSHELL CANNOT SUPPLY: a C linker. The bundled toolchain
    targets x86_64-pc-windows-gnu, which links with MinGW gcc/ld. This script
    auto-locates that linker from (in order): -MingwBin, $env:SLOC_MINGW_BIN, a
    PortableGit folder ($env:SLOC_PORTABLE_GIT or the same locations
    ci/jenkins/pipeline-helpers.groovy probes), or gcc already on PATH. Stage a
    PortableGit once (see ci\jenkins\stage-portable-git.ps1) and its
    mingw64\bin is used as the linker -- you do NOT also need Git Bash as a shell.

.PARAMETER Build
    Compile from the bundled toolchain + vendor sources even if a dist\ archive
    is present.

.PARAMETER Rebuild
    Discard any existing binary and force a fresh compile (implies -Build).

.PARAMETER MingwBin
    Explicit path to a MinGW '...\mingw64\bin' folder containing gcc.exe.

.PARAMETER SkipDist
    Ignore the pre-built dist\ archive and go straight to a source build.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\internal\install.ps1

.EXAMPLE
    # Force a from-source build, pointing at a staged PortableGit for the linker:
    $env:SLOC_PORTABLE_GIT = 'C:\Tools\PortableGit'
    powershell -ExecutionPolicy Bypass -File scripts\internal\install.ps1 -Rebuild
#>
[CmdletBinding()]
param(
    [switch] $Build,
    [switch] $Rebuild,
    [string] $MingwBin,
    [switch] $SkipDist
)

$ErrorActionPreference = 'Stop'
if ($Rebuild) { $Build = $true }

# ── Paths ────────────────────────────────────────────────────────────────────
$RepoRoot    = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$Exe         = Join-Path $RepoRoot 'oxide-sloc.exe'
$BuildOutput = Join-Path $RepoRoot 'target\release\oxide-sloc.exe'
$VendorDir   = Join-Path $RepoRoot 'vendor'
$VendorSums  = Join-Path $RepoRoot 'vendor.checksums.sha256'
$ToolsDir    = Join-Path $RepoRoot '.tools'
$TarExe      = Join-Path $env:SystemRoot 'System32\tar.exe'

function Info($m)  { Write-Host " $m" }
function Ok($m)    { Write-Host " [OK] $m" -ForegroundColor Green }
function Warn($m)  { Write-Host " [WARN] $m" -ForegroundColor Yellow }
function Die($m)   { Write-Host " [ERROR] $m" -ForegroundColor Red; exit 1 }

Write-Host ''
Write-Host ' oxide-sloc installer (PowerShell)'
Write-Host ' ================================='

if (-not (Test-Path $TarExe)) {
    # Fall back to any tar on PATH (Git bsdtar, etc.).
    $TarExe = (Get-Command tar -ErrorAction SilentlyContinue).Source
}

# ── Helpers ──────────────────────────────────────────────────────────────────

# Reassemble split parts (…​.aa, .ab, …) into one file via streamed copy (no
# whole-file buffering, so multi-hundred-MB toolchain parts stay memory-safe).
function Join-Parts([string[]] $Parts, [string] $Dest) {
    $out = [System.IO.File]::Create($Dest)
    try {
        foreach ($p in $Parts) {
            $in = [System.IO.File]::OpenRead($p)
            try { $in.CopyTo($out) } finally { $in.Dispose() }
        }
    } finally { $out.Dispose() }
}

# Verify each part against a `<sha256> *<name>` checksums file. Missing entries
# are skipped (mirrors install.sh, which only checks names it finds).
function Test-PartChecksums([string[]] $Parts, [string] $SumsFile) {
    if (-not (Test-Path $SumsFile)) { return }
    $map = @{}
    foreach ($line in Get-Content $SumsFile) {
        $t = $line.Trim()
        if (-not $t -or $t.StartsWith('#')) { continue }
        $cols = $t -split '\s+'
        if ($cols.Count -lt 2) { continue }
        $name = ($cols[-1] -replace '^\*', '')
        $map[$name] = $cols[0].ToLower()
    }
    foreach ($p in $Parts) {
        $name = Split-Path $p -Leaf
        if (-not $map.ContainsKey($name)) { continue }
        $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $p).Hash.ToLower()
        if ($actual -ne $map[$name]) {
            Die "Checksum mismatch for $name (expected $($map[$name]), got $actual). Corrupt or truncated part."
        }
    }
    Ok 'Checksum verified.'
}

# Locate a MinGW '...\bin' folder that contains gcc.exe (the windows-gnu linker).
function Resolve-MingwBin {
    $tryDirs = @()
    if ($MingwBin)            { $tryDirs += $MingwBin }
    if ($env:SLOC_MINGW_BIN)  { $tryDirs += $env:SLOC_MINGW_BIN }
    # PortableGit / Git roots -> <root>\mingw64\bin (mirrors resolveBash order).
    $roots = @()
    if ($env:SLOC_PORTABLE_GIT) { $roots += $env:SLOC_PORTABLE_GIT }
    $roots += 'C:\Program Files\Git'
    $roots += 'C:\Program Files (x86)\Git'
    if ($env:LOCALAPPDATA) { $roots += (Join-Path $env:LOCALAPPDATA 'Programs\Git') }
    if ($env:WORKSPACE)    { $roots += (Join-Path $env:WORKSPACE '.tools\PortableGit') }
    if ($env:USERPROFILE)  { $roots += (Join-Path $env:USERPROFILE 'PortableGit') }
    $roots += 'C:\Tools\PortableGit'
    $roots += 'C:\PortableGit'
    foreach ($r in $roots) { $tryDirs += (Join-Path $r 'mingw64\bin') }

    foreach ($d in $tryDirs) {
        if ($d -and (Test-Path (Join-Path $d 'gcc.exe'))) { return (Resolve-Path $d).Path }
    }
    # Already on PATH?
    $gcc = Get-Command gcc -ErrorAction SilentlyContinue
    if ($gcc) { return (Split-Path $gcc.Source -Parent) }
    return $null
}

function Get-WorkspaceVersion {
    $line = Select-String -Path (Join-Path $RepoRoot 'Cargo.toml') -Pattern '^version\s*=' |
        Select-Object -First 1
    if ($line -and $line.Line -match '"([^"]+)"') { return $Matches[1] }
    return $null
}

# ── 1. Already installed & current? ──────────────────────────────────────────
if ((Test-Path $Exe) -and -not $Rebuild) {
    $wsVer = Get-WorkspaceVersion
    $instVer = $null
    try { $instVer = ((& $Exe --version 2>$null) -join ' ' -replace '.*?(\d+\.\d+\.\d+).*', '$1') } catch {}
    if ($wsVer -and $instVer -and $instVer -ne $wsVer) {
        Warn "Installed oxide-sloc.exe is v$instVer but this checkout is v$wsVer -- rebuilding."
        Remove-Item -LiteralPath $Exe -Force
    } else {
        Ok "oxide-sloc.exe already present$(if($instVer){" (v$instVer)"})."
        Info 'Run:  .\oxide-sloc.exe serve'
        exit 0
    }
}
if ((Test-Path $Exe) -and $Rebuild) {
    Info '[-Rebuild] Removing existing binary...'
    Remove-Item -LiteralPath $Exe -Force
}

# ── 2. Pre-built binary from dist\ ───────────────────────────────────────────
if (-not $Build -and -not $SkipDist) {
    $distZip = Join-Path $RepoRoot 'dist\oxide-sloc-windows-x64.zip'
    $distTgz = Join-Path $RepoRoot 'dist\oxide-sloc-windows-x64.tar.gz'
    if ((Test-Path $distZip) -or (Test-Path $distTgz)) {
        Info 'Pre-built binary found in dist\ -- extracting (no build)...'
        $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("sloc-dist-" + [System.Guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Force -Path $tmp | Out-Null
        $got = $false
        try {
            if ((Test-Path $distTgz) -and $TarExe) {
                & $TarExe -xzf $distTgz -C $tmp 2>$null
                if (Test-Path (Join-Path $tmp 'oxide-sloc.exe')) { $got = $true }
            }
            if (-not $got -and (Test-Path $distZip)) {
                try { Expand-Archive -LiteralPath $distZip -DestinationPath $tmp -Force } catch {}
                if (Test-Path (Join-Path $tmp 'oxide-sloc.exe')) { $got = $true }
            }
            if (-not $got -and (Test-Path $distZip) -and $TarExe) {
                & $TarExe -xf $distZip -C $tmp 2>$null
                if (Test-Path (Join-Path $tmp 'oxide-sloc.exe')) { $got = $true }
            }
            if ($got) {
                Copy-Item (Join-Path $tmp 'oxide-sloc.exe') $Exe -Force
                Ok 'oxide-sloc.exe installed from dist\ (no build, no network).'
                Info 'Start the web UI:  .\oxide-sloc.exe serve'
                exit 0
            }
        } finally {
            Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
        }
        Warn 'dist\ extraction produced no binary -- falling back to a source build.'
    }
}

# ── 3. Bootstrap the bundled Rust toolchain (if cargo is absent) ─────────────
$haveCargo = [bool](Get-Command cargo -ErrorAction SilentlyContinue)
if (-not $haveCargo) {
    $tcBase  = Join-Path $RepoRoot 'toolchain\rust-toolchain-windows-x64.tar.gz'
    $tcParts = @(Get-ChildItem -Path (Split-Path $tcBase -Parent) `
        -Filter 'rust-toolchain-windows-x64.tar.gz.*' -File -ErrorAction SilentlyContinue |
        Sort-Object Name | Select-Object -ExpandProperty FullName)
    if ($tcParts.Count -eq 0 -and (Test-Path $tcBase)) { $tcParts = @($tcBase) }
    if ($tcParts.Count -eq 0) {
        Die "No Rust toolchain on PATH and no toolchain\rust-toolchain-windows-x64.tar.gz.* parts found. Ensure the full repo (with committed toolchain\) is present."
    }
    if (-not $TarExe) { Die "tar.exe not found (expected in System32 on Windows 10/11). Cannot extract the toolchain." }

    Info 'No Rust toolchain detected -- bootstrapping from the bundled archive...'
    Info '(one-time extract, ~300-500 MB on disk)'
    Test-PartChecksums -Parts $tcParts -SumsFile (Join-Path $RepoRoot 'toolchain\checksums.sha256')

    New-Item -ItemType Directory -Force -Path $ToolsDir | Out-Null
    $tcTar = Join-Path ([System.IO.Path]::GetTempPath()) ("sloc-tc-" + [System.Guid]::NewGuid().ToString('N') + '.tar.gz')
    try {
        if ($tcParts.Count -gt 1) {
            Info 'Reassembling toolchain parts...'
            Join-Parts -Parts $tcParts -Dest $tcTar
        } else {
            $tcTar = $tcParts[0]
        }
        Info 'Extracting toolchain archive...'
        # bsdtar (Windows tar.exe) cannot create the rustup proxy symlinks: in the
        # bundle cargo/bin/cargo.exe, rustc.exe, ... are POSIX symlinks to rustup.exe,
        # so tar emits "Can't create ... Invalid argument" per proxy and exits non-zero.
        # This is EXPECTED and benign — bsdtar still extracts every real file, and we
        # Let cmd.exe perform the stderr redirection so PowerShell never sees tar's
        # native stderr as ErrorRecords (pwsh 7.3+'s
        # $PSNativeCommandUseErrorActionPreference would otherwise make the non-zero
        # exit terminating AND serialize its own formatted error wrapper into any
        # `2>file` capture). With `cmd /c "... 2>file"` the file gets RAW tar text.
        # Then surface only errors that are NOT the expected proxy noise.
        $errFile = [System.IO.Path]::GetTempFileName()
        $tarCmd = '"{0}" -xzf "{1}" -C "{2}" 2>"{3}"' -f $TarExe, $tcTar, $ToolsDir, $errFile
        cmd /c $tarCmd | Out-Null
        $tarErr = (Get-Content -LiteralPath $errFile -ErrorAction SilentlyContinue) -join "`n"
        Remove-Item -LiteralPath $errFile -Force -ErrorAction SilentlyContinue
        $realErr = ($tarErr -split "`n") | Where-Object {
            $_.Trim() -and
            $_ -notmatch "Can't create" -and $_ -notmatch 'Invalid argument' -and
            $_ -notmatch 'symlink' -and $_ -notmatch 'Error exit delayed'
        }
        if ($realErr) { $realErr | ForEach-Object { Warn ("tar: " + $_.Trim()) } }
    } finally {
        if ($tcTar -ne $tcParts[0] -and (Test-Path $tcTar)) { Remove-Item -LiteralPath $tcTar -Force -ErrorAction SilentlyContinue }
    }

    $rustupHome = Join-Path $ToolsDir 'rustup'
    $cargoHome  = Join-Path $ToolsDir 'cargo'
    $cargoBin   = Join-Path $cargoHome 'bin'

    # Patch any missing rustup proxies (cargo.exe/rustc.exe/rustdoc.exe are
    # hardlink aliases of rustup.exe).
    $rustupProxy = Join-Path $cargoBin 'rustup.exe'
    if (Test-Path $rustupProxy) {
        foreach ($proxy in @('cargo.exe','rustc.exe','rustdoc.exe')) {
            $pp = Join-Path $cargoBin $proxy
            if (-not (Test-Path $pp)) { Copy-Item $rustupProxy $pp -Force }
        }
        Ok 'Toolchain proxy binaries verified.'
    }

    # Find …\rustup\toolchains\<name>\bin
    $tcBin = Get-ChildItem -Path (Join-Path $rustupHome 'toolchains') -Directory -ErrorAction SilentlyContinue |
        ForEach-Object { Join-Path $_.FullName 'bin' } | Where-Object { Test-Path $_ } | Select-Object -First 1

    $env:RUSTUP_HOME = $rustupHome
    $env:CARGO_HOME  = $cargoHome
    $env:PATH        = "$cargoBin;$(if($tcBin){"$tcBin;"})$env:PATH"
    if ($tcBin) {
        # Pin RUSTUP_TOOLCHAIN to the exact bundled name so the "1.97" shorthand in
        # rust-toolchain.toml never triggers an online channel sync (fatal offline).
        $env:RUSTUP_TOOLCHAIN = Split-Path (Split-Path $tcBin -Parent) -Leaf
    }

    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Die "cargo not found after toolchain extraction -- the archive may be incomplete."
    }
    Ok 'Rust toolchain bootstrapped at .tools\'
}

# ── 4. Ensure a C linker for the ACTIVE target ───────────────────────────────
# The bundled toolchain is windows-gnu (links with MinGW gcc/ld). But a Rust
# toolchain already on PATH may target MSVC instead, which links with link.exe
# from the MSVC Build Tools -- no MinGW needed. Decide from rustc's host triple
# (after any bootstrap above, `rustc` is the one we'll actually build with).
$rustHost = ''
try {
    $rustHost = ((& rustc -vV 2>$null) | Where-Object { $_ -match '^host:' } |
        Select-Object -First 1) -replace '^host:\s*', ''
} catch {}
if ($rustHost -match 'msvc') {
    Ok "Toolchain targets $rustHost -- using its native MSVC linker (no MinGW needed)."
} else {
    # GNU target (bundled toolchain, or a gnu host) -- require a MinGW linker.
    $mingw = Resolve-MingwBin
    if ($mingw) {
        if (($env:PATH -split ';') -notcontains $mingw) { $env:PATH = "$mingw;$env:PATH" }
        Ok "MinGW linker: $mingw"
    } else {
        Die @"
No C linker (MinGW gcc) found -- the $(if($rustHost){$rustHost}else{'x86_64-pc-windows-gnu'}) build cannot link without one.
This is the one requirement PowerShell cannot supply. Choose one:
  * Stage a PortableGit (also gives you the linker), then re-run:
        powershell -File ci\jenkins\stage-portable-git.ps1 <PortableGit-*.7z.exe>
        `$env:SLOC_PORTABLE_GIT = '<extracted folder>'   # if outside the workspace
  * Point at an existing MinGW bin dir:
        powershell -File scripts\internal\install.ps1 -MingwBin 'C:\path\to\mingw64\bin'
  * Or set `$env:SLOC_MINGW_BIN to that folder.
"@
    }
}

# ── 5. Reassemble + verify vendor sources, write .cargo\config.toml ──────────
if (-not (Test-Path $VendorDir)) {
    $vParts = @(Get-ChildItem -Path $RepoRoot -Filter 'vendor.tar.gz.*' -File -ErrorAction SilentlyContinue |
        Sort-Object Name | Select-Object -ExpandProperty FullName)
    if ($vParts.Count -eq 0) {
        Die "Neither vendor\ nor vendor.tar.gz.* parts found. Ensure the full repository is present."
    }
    Test-PartChecksums -Parts $vParts -SumsFile $VendorSums
    Info 'Reassembling and decompressing vendor sources (one-time)...'
    $vTar = Join-Path ([System.IO.Path]::GetTempPath()) ("sloc-vendor-" + [System.Guid]::NewGuid().ToString('N') + '.tar.gz')
    try {
        Join-Parts -Parts $vParts -Dest $vTar
        & $TarExe -xzf $vTar -C $RepoRoot
    } finally {
        if (Test-Path $vTar) { Remove-Item -LiteralPath $vTar -Force -ErrorAction SilentlyContinue }
    }
    Ok 'Vendor sources ready.'
}

$cargoDir = Join-Path $RepoRoot '.cargo'
New-Item -ItemType Directory -Force -Path $cargoDir | Out-Null
@'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
'@ | Set-Content -Path (Join-Path $cargoDir 'config.toml') -Encoding ascii

# ── 6. Build from vendored sources ───────────────────────────────────────────
Info 'Building from vendored sources (cargo build --release --offline)...'
# Unset CC/CXX so a stray gcc from another toolkit can't override the MinGW gcc
# the windows-gnu target links with.
$env:CC = $null
$env:CXX = $null
Push-Location $RepoRoot
try {
    & cargo build --release --offline -p oxide-sloc
    $code = $LASTEXITCODE
} finally {
    Pop-Location
}
if ($code -ne 0) { Die "cargo build failed (exit $code). See the output above." }

if (Test-Path $BuildOutput) {
    Copy-Item $BuildOutput $Exe -Force
    Ok "Built and installed oxide-sloc.exe"
    Write-Host ''
    Info 'Start the web UI:  .\oxide-sloc.exe serve'
    exit 0
}
Die 'Build reported success but target\release\oxide-sloc.exe is missing.'
