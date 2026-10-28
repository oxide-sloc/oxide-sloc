<#
.SYNOPSIS
  Install oxide-sloc as a persistent Windows service (Task Scheduler), the Windows
  analogue of scripts/internal/install-systemd.sh.

.DESCRIPTION
  Registers a scheduled task named "OxideSLOC" that runs `oxide-sloc serve --server`
  at system startup (LogonType ServiceAccount = SYSTEM), so the server survives
  reboots without an interactive login. On first install it generates an API key
  (fail-closed parity with the systemd installer), stores runtime settings in an
  ACL-restricted install directory, and opens the LAN firewall port for all
  profiles. Mirrors install-systemd.sh behavior and output.

  Registering a SYSTEM task and adding a firewall rule triggers a one-time UAC
  elevation (the script self-elevates), mirroring the `sudo` step on Linux.

.PARAMETER Port
  TCP port to bind (default 4317). Ignored if -Bind is given.

.PARAMETER Bind
  Full bind address ip:port (default 0.0.0.0:<Port>).

.PARAMETER ApiKey
  Pre-set API key. Generated automatically when omitted.

.PARAMETER TlsCert / .PARAMETER TlsKey
  PEM cert/key paths for HTTPS (sets SLOC_TLS_CERT / SLOC_TLS_KEY).

.PARAMETER AllowedRoots
  Semicolon-separated absolute paths the server may scan (SLOC_ALLOWED_ROOTS).

.PARAMETER InstallDir
  Where the binary + runtime env live (default C:\ProgramData\oxide-sloc).

.PARAMETER NoFirewall
  Skip the inbound firewall rule.

.PARAMETER Uninstall
  Stop + remove the task, firewall rule, and install directory (incl. the secret).

.PARAMETER DryRun
  Print the planned actions without elevation, registering the task, or touching
  the firewall. Used by the Pester tests. Still writes the wrapper/env into
  -InstallDir so it can be inspected.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\internal\install-service.ps1

.EXAMPLE
  .\install-service.ps1 -TlsCert C:\certs\server.crt -TlsKey C:\certs\server.key

.EXAMPLE
  .\install-service.ps1 -Uninstall
#>
[CmdletBinding()]
param(
    [int]    $Port = 4317,
    [string] $Bind,
    [string] $ApiKey,
    [string] $TlsCert,
    [string] $TlsKey,
    [string] $AllowedRoots,
    [string] $InstallDir = (Join-Path $env:ProgramData 'oxide-sloc'),
    [switch] $NoFirewall,
    [switch] $Uninstall,
    [switch] $DryRun
)

$ErrorActionPreference = 'Stop'
$TaskName = 'OxideSLOC'
if (-not $Bind) { $Bind = "0.0.0.0:$Port" }
# Derive the port from the bind address so the firewall rule always matches.
$BindPort = $Bind.Split(':')[-1]
if (-not ($BindPort -match '^\d+$')) { $BindPort = "$Port" }

function Test-Admin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    (New-Object Security.Principal.WindowsPrincipal $id).IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator)
}

# Self-elevate unless we're already elevated or just doing a dry run.
if (-not $DryRun -and -not (Test-Admin)) {
    Write-Host 'Registering a system service and opening the firewall needs elevation.'
    Write-Host 'Re-launching with an elevation (UAC) prompt...'
    $argList = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"") + $args
    foreach ($kv in $PSBoundParameters.GetEnumerator()) {
        if ($kv.Value -is [switch]) {
            if ($kv.Value.IsPresent) { $argList += "-$($kv.Key)" }
        } else {
            $argList += "-$($kv.Key)"; $argList += "`"$($kv.Value)`""
        }
    }
    Start-Process -FilePath 'powershell.exe' -Verb RunAs -ArgumentList $argList
    exit 0
}

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$WrapperCmd = Join-Path $InstallDir 'run-service.cmd'
$InstalledExe = Join-Path $InstallDir 'oxide-sloc.exe'
$LogDir = Join-Path $InstallDir 'logs'

# ── Uninstall ────────────────────────────────────────────────────────────────
if ($Uninstall) {
    Write-Host 'Uninstalling oxide-sloc Windows service...'
    if (-not $DryRun) {
        $existing = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
        if ($existing) {
            Write-Host "  Stopping and removing scheduled task '$TaskName'..."
            try { Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue } catch {}
            Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
        }
        if (-not $NoFirewall) {
            Write-Host "  Removing firewall rule 'oxide-sloc-$BindPort'..."
            & netsh advfirewall firewall delete rule name="oxide-sloc-$BindPort" | Out-Null
        }
        if (Test-Path $InstallDir) {
            Write-Host "  Removing $InstallDir (contains the generated API key)..."
            Remove-Item -Recurse -Force $InstallDir
        }
    } else {
        Write-Host "  [dry-run] Unregister-ScheduledTask -TaskName $TaskName"
        Write-Host "  [dry-run] netsh advfirewall firewall delete rule name=oxide-sloc-$BindPort"
        Write-Host "  [dry-run] Remove-Item -Recurse -Force $InstallDir"
    }
    Write-Host ''
    Write-Host 'oxide-sloc service uninstalled.'
    exit 0
}

# ── Locate the binary ──────────────────────────────────────────────────────────
$BinarySource = $null
foreach ($cand in @(
        (Join-Path $RepoRoot 'oxide-sloc.exe'),
        (Join-Path $RepoRoot 'target\release\oxide-sloc.exe'),
        (Join-Path $RepoRoot 'target\debug\oxide-sloc.exe'))) {
    if (Test-Path $cand) { $BinarySource = $cand; break }
}
if (-not $BinarySource) {
    $onPath = Get-Command 'oxide-sloc.exe' -ErrorAction SilentlyContinue
    if ($onPath) { $BinarySource = $onPath.Source }
}
if (-not $BinarySource) {
    Write-Error ("oxide-sloc.exe not found. Build it first: cargo build --release -p oxide-sloc`n" +
                 "  or run: bash scripts/run.sh")
    exit 1
}

# ── Create install dir and restrict its ACL to SYSTEM + Administrators ─────────
Write-Host "Installing oxide-sloc as a Windows service (Task Scheduler)..."
Write-Host "  [1/5] Preparing $InstallDir"
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
if (-not $DryRun) {
    # Remove inherited (user) access so the stored API key is not world-readable.
    & icacls "$InstallDir" /inheritance:r /grant:r 'SYSTEM:(OI)(CI)F' 'Administrators:(OI)(CI)F' | Out-Null
}
Copy-Item -Force $BinarySource $InstalledExe

# ── API key (generate on first install; never clobber an existing one) ─────────
$GeneratedKey = ''
if (-not $ApiKey) {
    if (Test-Path $WrapperCmd) {
        # Re-install: reuse the key already baked into the wrapper.
        $existingLine = Select-String -Path $WrapperCmd -Pattern '^\s*set\s+"SLOC_API_KEYS=(.+)"' -ErrorAction SilentlyContinue
        if ($existingLine) { $ApiKey = $existingLine.Matches[0].Groups[1].Value }
    }
    if (-not $ApiKey) {
        $bytes = New-Object byte[] 32
        [System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
        $ApiKey = -join ($bytes | ForEach-Object { $_.ToString('x2') })
        $GeneratedKey = $ApiKey
    }
}

# ── Write the wrapper .cmd the task runs (sets env, then execs the server) ─────
Write-Host "  [2/5] Writing runtime wrapper $WrapperCmd"
$lines = @('@echo off')
$lines += "set `"SLOC_API_KEYS=$ApiKey`""
$lines += "set `"SLOC_BIND=$Bind`""
$lines += 'set "SLOC_RATE_LIMIT=120"'
$lines += "set `"OXIDE_SLOC_ROOT=$InstallDir`""
if ($TlsCert) { $lines += "set `"SLOC_TLS_CERT=$TlsCert`"" }
if ($TlsKey)  { $lines += "set `"SLOC_TLS_KEY=$TlsKey`"" }
if ($AllowedRoots) { $lines += "set `"SLOC_ALLOWED_ROOTS=$AllowedRoots`"" }
$logFile = Join-Path $LogDir 'service.log'
$lines += "`"$InstalledExe`" serve --server --bind $Bind >> `"$logFile`" 2>&1"
Set-Content -Path $WrapperCmd -Value $lines -Encoding ASCII

# ── Register the scheduled task (AtStartup, SYSTEM) ────────────────────────────
Write-Host "  [3/5] Registering scheduled task '$TaskName' (runs at startup as SYSTEM)"
$taskCommand = "cmd.exe /c `"$WrapperCmd`""
if (-not $DryRun) {
    $action    = New-ScheduledTaskAction -Execute 'cmd.exe' -Argument "/c `"$WrapperCmd`"" -WorkingDirectory $InstallDir
    $trigger   = New-ScheduledTaskTrigger -AtStartup
    $principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
    $settings  = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
                    -StartWhenAvailable -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)
    Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger `
        -Principal $principal -Settings $settings -Force | Out-Null
} else {
    Write-Host "  [dry-run] task action: $taskCommand"
}

# ── Firewall ───────────────────────────────────────────────────────────────────
$fwCommand = "netsh advfirewall firewall add rule name=oxide-sloc-$BindPort dir=in action=allow protocol=TCP localport=$BindPort profile=any remoteip=any"
if (-not $NoFirewall) {
    Write-Host "  [4/5] Opening inbound TCP $BindPort (all profiles, all remote hosts)"
    if (-not $DryRun) {
        & netsh advfirewall firewall delete rule name="oxide-sloc-$BindPort" | Out-Null 2>&1
        Invoke-Expression "& $fwCommand" | Out-Null
    } else {
        Write-Host "  [dry-run] $fwCommand"
    }
} else {
    Write-Host "  [4/5] Firewall rule skipped (-NoFirewall)."
}

# ── Start the service now ──────────────────────────────────────────────────────
Write-Host "  [5/5] Starting '$TaskName'"
if (-not $DryRun) { Start-ScheduledTask -TaskName $TaskName }

# ── Summary (mirrors install-systemd.sh output) ────────────────────────────────
function Get-PrimaryLanIp {
    try {
        $r = Find-NetRoute -RemoteIPAddress 1.1.1.1 -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if ($r -and $r.IPAddress) { return $r.IPAddress }
    } catch {}
    (Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue |
        Where-Object { $_.IPAddress -notmatch '^(127\.|169\.254\.)' } |
        Select-Object -First 1 -ExpandProperty IPAddress)
}
$scheme = if ($TlsCert -and $TlsKey) { 'https' } else { 'http' }
$lanIp  = Get-PrimaryLanIp

Write-Host ''
Write-Host 'Installation complete.'
Write-Host ''
if ($GeneratedKey) {
    Write-Host "  API key (generated, stored in $WrapperCmd):"
    Write-Host "    $GeneratedKey"
    Write-Host '  Send requests with:  Authorization: Bearer <key>'
} else {
    Write-Host "  API key: kept existing key (in $WrapperCmd; readable by SYSTEM + Administrators only)."
}
Write-Host ''
if ($lanIp) {
    Write-Host '  Reach it from another machine on the LAN:'
    Write-Host "    $scheme`://$lanIp`:$BindPort/"
    Write-Host "    $scheme`://$lanIp`:$BindPort/auth/login    (sign in with the API key)"
} else {
    Write-Host "  This host binds all interfaces on port $BindPort; give clients its LAN IP."
}
Write-Host ''
Write-Host "  Status:  Get-ScheduledTask -TaskName $TaskName"
Write-Host "  Logs:    Get-Content `"$logFile`" -Tail 50 -Wait"
Write-Host "  Stop:    Stop-ScheduledTask -TaskName $TaskName"
Write-Host "  Remove:  powershell -File scripts\internal\install-service.ps1 -Uninstall"
Write-Host ''
