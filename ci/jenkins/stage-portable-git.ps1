<#
.SYNOPSIS
    Stage a no-install, no-admin portable Git Bash for the oxide-sloc Jenkins
    pipeline on a Windows agent.

.DESCRIPTION
    The pipeline runs its POSIX .sh scripts through a bash.exe on Windows (see
    ci/jenkins/pipeline-helpers.groovy :: resolveBash / shx). Installing Git for
    Windows system-wide requires admin and is often blocked on locked-down
    agents. Git for Windows also ships as PortableGit -- a self-contained folder
    that needs NO installer, NO admin, and NO registry: you just extract it.

    This script extracts a PortableGit archive into a folder the pipeline's
    resolveBash() auto-discovers by default:

        <Destination>\...   (default: <WORKSPACE-or-CWD>\.tools\PortableGit)

    It accepts, as -Source:
      * a PortableGit self-extracting archive  (PortableGit-*.7z.exe)
      * a plain .zip of a PortableGit folder
      * an already-extracted PortableGit folder (validated / copied)

    After staging, resolveBash() finds <dest>\bin\bash.exe with no further
    configuration (the workspace .tools\PortableGit path is on its probe list).
    If you stage it OUTSIDE the workspace, set SLOC_PORTABLE_GIT to the folder.

    Everything here is native PowerShell -- it does NOT require bash to already
    be present, so it can bootstrap an agent from a truly bash-free state.

.PARAMETER Source
    Path to a PortableGit-*.7z.exe, a .zip, or an already-extracted folder.
    Optional when -AutoDiscover is used.

.PARAMETER AutoDiscover
    Search the agent for a PortableGit-*.7z.exe / .zip (in SLOC_PORTABLE_GIT_ARCHIVE,
    the job WORKSPACE, WORKSPACE\.tools, %USERPROFILE%\Downloads, %USERPROFILE%, and
    C:\Tools) and stage the first match. Exits 3 (no error thrown) when none is found,
    so the Jenkins pipeline can call this unconditionally as a bash-free last resort.

.PARAMETER Destination
    Target folder for the extracted PortableGit. Defaults to
    "$env:WORKSPACE\.tools\PortableGit" (or "<cwd>\.tools\PortableGit" when
    WORKSPACE is unset).

.PARAMETER Force
    Overwrite a non-empty destination.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File ci\jenkins\stage-portable-git.ps1 `
        C:\stage\PortableGit-2.47.0-64-bit.7z.exe

.EXAMPLE
    # Stage into a shared tools dir and expose it to every job on the agent:
    powershell -File ci\jenkins\stage-portable-git.ps1 `
        -Source C:\stage\PortableGit-2.47.0-64-bit.7z.exe `
        -Destination C:\Tools\PortableGit
    setx SLOC_PORTABLE_GIT C:\Tools\PortableGit
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $false, Position = 0)]
    [string] $Source,

    [Parameter(Position = 1)]
    [string] $Destination,

    [switch] $AutoDiscover,

    [switch] $Force
)

$ErrorActionPreference = 'Stop'

function Write-Step($msg) { Write-Host "[stage-portable-git] $msg" }

# ---- Auto-discover a PortableGit archive on the agent ---------------------
# Bash-free: pure PowerShell, so it can bootstrap a truly bash-free agent. When
# nothing is found we exit 3 (NOT throw) so the pipeline's fallback stays quiet.
if (-not $Source -and $AutoDiscover) {
    Write-Step "Auto-discovering a PortableGit archive on this agent..."
    $searchDirs = @()
    if ($env:WORKSPACE) {
        $searchDirs += $env:WORKSPACE
        $searchDirs += (Join-Path $env:WORKSPACE '.tools')
    }
    if ($env:USERPROFILE) {
        $searchDirs += (Join-Path $env:USERPROFILE 'Downloads')
        $searchDirs += $env:USERPROFILE
    }
    $searchDirs += 'C:\Tools'

    # Explicit override wins over any directory scan.
    if ($env:SLOC_PORTABLE_GIT_ARCHIVE -and (Test-Path $env:SLOC_PORTABLE_GIT_ARCHIVE)) {
        $Source = $env:SLOC_PORTABLE_GIT_ARCHIVE
        Write-Step "Using SLOC_PORTABLE_GIT_ARCHIVE: $Source"
    } else {
        foreach ($d in $searchDirs) {
            if (-not $d -or -not (Test-Path $d)) { continue }
            $hit = Get-ChildItem -Path $d -Filter 'PortableGit-*' -File -ErrorAction SilentlyContinue |
                Where-Object { $_.Extension -ieq '.exe' -or $_.Extension -ieq '.zip' } |
                Sort-Object Name -Descending | Select-Object -First 1
            if ($hit) { $Source = $hit.FullName; break }
        }
    }

    if (-not $Source) {
        Write-Step "No PortableGit-*.7z.exe or .zip found (searched: $($searchDirs -join '; '))."
        Write-Step "Drop a PortableGit archive in the workspace or set SLOC_PORTABLE_GIT_ARCHIVE, then re-run."
        exit 3
    }
    Write-Step "Discovered archive: $Source"
}

if (-not $Source) {
    throw "No -Source provided. Pass a PortableGit archive/folder, or use -AutoDiscover."
}

# ---- Resolve destination --------------------------------------------------
if (-not $Destination) {
    $root = if ($env:WORKSPACE) { $env:WORKSPACE } else { (Get-Location).Path }
    $Destination = Join-Path $root '.tools\PortableGit'
}
Write-Step "Destination: $Destination"

if (-not (Test-Path $Source)) {
    throw "Source not found: $Source"
}
$src = Get-Item -LiteralPath $Source

# ---- If destination already has a working bash, we're done (unless -Force) --
$existingBash = Join-Path $Destination 'bin\bash.exe'
if ((Test-Path $existingBash) -and -not $Force) {
    Write-Step "PortableGit already staged (bash.exe present). Use -Force to re-extract."
    Write-Step "bash.exe: $existingBash"
    exit 0
}

if ((Test-Path $Destination) -and $Force) {
    Write-Step "Removing existing destination (-Force)..."
    Remove-Item -LiteralPath $Destination -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $Destination | Out-Null

# ---- Extract per source type ----------------------------------------------
if ($src.PSIsContainer) {
    # Already-extracted folder: locate its bash and copy the tree in.
    $srcBash = @(
        (Join-Path $src.FullName 'bin\bash.exe'),
        (Join-Path $src.FullName 'usr\bin\bash.exe')
    ) | Where-Object { Test-Path $_ } | Select-Object -First 1
    if (-not $srcBash) {
        throw "Folder '$($src.FullName)' does not look like a Git/PortableGit root (no bin\bash.exe)."
    }
    Write-Step "Copying extracted PortableGit tree..."
    Copy-Item -Path (Join-Path $src.FullName '*') -Destination $Destination -Recurse -Force
}
elseif ($src.Extension -ieq '.exe') {
    # 7-Zip self-extracting archive (PortableGit-*.7z.exe). The SFX accepts
    # -o<dir> (no space) and -y. Run it and wait.
    Write-Step "Extracting self-extracting archive..."
    $args = @("-o`"$Destination`"", '-y')
    $p = Start-Process -FilePath $src.FullName -ArgumentList $args -Wait -PassThru -NoNewWindow
    if ($p.ExitCode -ne 0) {
        throw "PortableGit self-extractor exited with code $($p.ExitCode)."
    }
}
elseif ($src.Extension -ieq '.zip') {
    Write-Step "Expanding .zip archive..."
    Expand-Archive -LiteralPath $src.FullName -DestinationPath $Destination -Force
    # Some zips nest everything under a single top folder; flatten if so.
    $nestedBash = Get-ChildItem -Path $Destination -Recurse -Filter 'bash.exe' -File -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -like '*\bin\bash.exe' } | Select-Object -First 1
    if ($nestedBash) {
        $nestedRoot = Split-Path (Split-Path $nestedBash.FullName -Parent) -Parent
        if ($nestedRoot -ne (Get-Item $Destination).FullName) {
            Write-Step "Flattening nested top-level folder..."
            Move-Item -Path (Join-Path $nestedRoot '*') -Destination $Destination -Force
        }
    }
}
else {
    throw "Unsupported source '$($src.Name)'. Provide a PortableGit-*.7z.exe, a .zip, or an extracted folder."
}

# ---- Verify ----------------------------------------------------------------
$bash = Join-Path $Destination 'bin\bash.exe'
if (-not (Test-Path $bash)) {
    throw "Extraction completed but '$bash' is missing. Is this a real PortableGit archive?"
}
$mingw = Join-Path $Destination 'mingw64\bin\gcc.exe'
$hasGcc = Test-Path $mingw

Write-Host ''
Write-Step "PortableGit staged successfully."
Write-Step "  bash.exe : $bash"
Write-Step "  MinGW gcc: $(if ($hasGcc) { $mingw } else { '(not bundled in this edition; windows-gnu build may need a separate linker)' })"
Write-Host ''

# ---- Guidance --------------------------------------------------------------
$wsDefault = if ($env:WORKSPACE) { Join-Path $env:WORKSPACE '.tools\PortableGit' } else { $null }
if ($wsDefault -and ((Get-Item $Destination).FullName -ieq (Get-Item $wsDefault).FullName)) {
    Write-Step "This is the default workspace location -- resolveBash() auto-detects it. No env var needed."
} else {
    Write-Step "Staged outside the workspace default. Expose it to the pipeline with:"
    Write-Host  "    setx SLOC_PORTABLE_GIT `"$Destination`""
    Write-Host  "  (or set SLOC_PORTABLE_GIT in the Jenkins agent/node environment)."
}
exit 0
