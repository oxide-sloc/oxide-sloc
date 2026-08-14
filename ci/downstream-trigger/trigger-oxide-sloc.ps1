# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>
#
# trigger-oxide-sloc.ps1 — portable downstream trigger for oxide-sloc (Windows).
#
# The PowerShell twin of trigger-oxide-sloc.sh, for upstream builds running on
# Windows agents. Compatible with Windows PowerShell 5.1 (the version shipped
# with Windows and used by most on-prem Jenkins Windows nodes) as well as
# PowerShell 7+. No modules, no admin, no modern-CI features required.
#
# Same two modes and the same guards as the shell version:
#   success gate, HMAC signing (server mode), idempotency key, retry/backoff,
#   fail-closed configuration validation.
#
# Exit codes: 0 = triggered OR intentionally skipped; 2 = misconfiguration;
#             3 = trigger failed after all retries.
[CmdletBinding()]
param(
    [ValidateSet('server', 'dispatch')]
    [string]$Mode      = $(if ($env:OXIDE_SLOC_MODE) { $env:OXIDE_SLOC_MODE } else { 'server' }),
    [string]$Url       = $env:OXIDE_SLOC_URL,
    [string]$Secret    = $env:OXIDE_SLOC_SECRET,
    [string]$Repo      = $env:OXIDE_SLOC_REPO,
    [string]$Branch    = $env:OXIDE_SLOC_BRANCH,
    [string]$Commit    = $env:OXIDE_SLOC_COMMIT,
    [string]$Status    = $(if ($env:OXIDE_SLOC_STATUS) { $env:OXIDE_SLOC_STATUS } else { 'success' }),
    [string]$System    = $env:OXIDE_SLOC_SYSTEM,
    [string]$Job       = $env:OXIDE_SLOC_JOB,
    [string]$BuildId   = $env:OXIDE_SLOC_BUILD_ID,
    [string]$BuildUrl  = $env:OXIDE_SLOC_BUILD_URL,
    [ValidateSet('', 'github', 'gitlab', 'jenkins', 'bitbucket')]
    [string]$Dispatch  = $env:OXIDE_SLOC_DISPATCH,
    [string]$Token     = $env:OXIDE_SLOC_TOKEN,
    [string]$EventType = $(if ($env:OXIDE_SLOC_EVENT_TYPE) { $env:OXIDE_SLOC_EVENT_TYPE } else { 'oxide-sloc-scan' }),
    [int]$Retries      = $(if ($env:OXIDE_SLOC_RETRIES) { [int]$env:OXIDE_SLOC_RETRIES } else { 4 }),
    [int]$TimeoutSec   = $(if ($env:OXIDE_SLOC_TIMEOUT) { [int]$env:OXIDE_SLOC_TIMEOUT } else { 30 }),
    [switch]$Insecure,
    [switch]$Always,
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
function Log($m) { Write-Host "trigger-oxide-sloc: $m" }
function Die($m) { Write-Error "trigger-oxide-sloc: $m"; exit 2 }

# ── success gate (guard 1) ────────────────────────────────────────────────────
$statusLc = ($Status -replace '\s', '').ToLowerInvariant()
$successSet = @('success', 'succeeded', 'passed', 'pass', 'ok', 'green', 'completed', '0', 'true')
if ($successSet -notcontains $statusLc -and -not $Always) {
    Log "upstream status '$Status' is not a success - skipping downstream scan"
    exit 0
}

# ── required-field validation (fail closed) ───────────────────────────────────
if (-not $Repo)   { Die '-Repo is required' }
if (-not $Branch) { Die '-Branch is required' }
if (-not $BuildId) { Log 'warning: no -BuildId given; de-duplication is disabled' }

# Allow untrusted TLS only when explicitly asked (last resort).
if ($Insecure -and $PSVersionTable.PSVersion.Major -lt 6) {
    Add-Type @"
using System.Net;using System.Security.Cryptography.X509Certificates;
public class OxSlocCertPolicy : ICertificatePolicy {
  public bool CheckValidationResult(ServicePoint s, X509Certificate c, WebRequest r, int p) { return true; }
}
"@
    [System.Net.ServicePointManager]::CertificatePolicy = New-Object OxSlocCertPolicy
}
[System.Net.ServicePointManager]::SecurityProtocol = [System.Net.SecurityProtocolType]::Tls12

function Get-HmacSha256Hex([string]$key, [string]$msg) {
    $h = New-Object System.Security.Cryptography.HMACSHA256
    $h.Key = [Text.Encoding]::UTF8.GetBytes($key)
    $bytes = $h.ComputeHash([Text.Encoding]::UTF8.GetBytes($msg))
    -join ($bytes | ForEach-Object { $_.ToString('x2') })
}

# POST with retry + exponential backoff. Returns the response body string.
function Invoke-WithRetry([string]$Uri, [hashtable]$Headers, $Body, [string]$ContentType) {
    $attempt = 1; $delay = 2
    $irmArgs = @{ Uri = $Uri; Method = 'Post'; Headers = $Headers; TimeoutSec = $TimeoutSec }
    if ($Body) { $irmArgs.Body = $Body }
    if ($ContentType) { $irmArgs.ContentType = $ContentType }
    if ($Insecure -and $PSVersionTable.PSVersion.Major -ge 6) { $irmArgs.SkipCertificateCheck = $true }
    while ($true) {
        try {
            return (Invoke-RestMethod @irmArgs | Out-String).Trim()
        } catch {
            $code = 0
            if ($_.Exception.Response -and $_.Exception.Response.StatusCode) {
                $code = [int]$_.Exception.Response.StatusCode
            }
            # 4xx (except 429) is a caller error — do not retry.
            if ($code -ge 400 -and $code -lt 500 -and $code -ne 429) {
                Log "downstream rejected the trigger (HTTP $code): $($_.Exception.Message)"
                exit 3
            }
            if ($attempt -ge $Retries) {
                Log "trigger failed after $Retries attempt(s): $($_.Exception.Message)"
                exit 3
            }
            Log "attempt $attempt failed (HTTP $code); retrying in ${delay}s"
            Start-Sleep -Seconds $delay
            $attempt++; $delay *= 2
        }
    }
}

function Invoke-ServerMode {
    if (-not $Url)    { Die 'server mode requires -Url' }
    if (-not $Secret) { Die 'server mode requires -Secret' }

    $sys = if ($System) { $System } else { 'ci' }
    # Build an ordered object so the serialised bytes are deterministic, then
    # sign those exact bytes.
    $payload = [ordered]@{
        repo_url   = $Repo
        branch     = $Branch
        commit_sha = $Commit
        status     = 'success'
        upstream   = [ordered]@{
            system   = $sys
            job      = $Job
            build_id = $BuildId
            url      = $BuildUrl
        }
    }
    $body = ($payload | ConvertTo-Json -Compress -Depth 5)
    $sig = 'sha256=' + (Get-HmacSha256Hex $Secret $body)
    $endpoint = ($Url.TrimEnd('/')) + '/webhooks/ci'

    if ($DryRun) {
        Log "[dry-run] POST $endpoint"
        Log "[dry-run] X-Sloc-Signature: $sig"
        Log "[dry-run] body: $body"
        return
    }
    Log "triggering scan of $Repo@$Branch via $endpoint"
    $headers = @{ 'X-Sloc-Signature' = $sig }
    $resp = Invoke-WithRetry -Uri $endpoint -Headers $headers -Body $body -ContentType 'application/json'
    Log "downstream response: $resp"
}

function Invoke-DispatchMode {
    if (-not $Dispatch) { Die 'dispatch mode requires -Dispatch KIND' }
    if (-not $Url)      { Die 'dispatch mode requires -Url' }
    switch ($Dispatch) {
        'github' {
            if (-not $Token) { Die 'github dispatch requires -Token (PAT with repo scope)' }
            $body = @{ event_type = $EventType; client_payload = @{
                repo_url = $Repo; branch = $Branch; commit_sha = $Commit; build_id = $BuildId } } |
                ConvertTo-Json -Compress -Depth 5
            if ($DryRun) { Log "[dry-run] POST $Url body: $body"; return }
            $headers = @{ Authorization = "Bearer $Token"; Accept = 'application/vnd.github+json' }
            $resp = Invoke-WithRetry -Uri $Url -Headers $headers -Body $body -ContentType 'application/json'
        }
        'gitlab' {
            if (-not $Token) { Die 'gitlab dispatch requires -Token (pipeline trigger token)' }
            $form = "token=$Token&ref=$Branch"
            if ($DryRun) { Log "[dry-run] POST $Url ($form)"; return }
            $resp = Invoke-WithRetry -Uri $Url -Headers @{} -Body $form -ContentType 'application/x-www-form-urlencoded'
        }
        'jenkins' {
            if (-not $Token) { Die 'jenkins dispatch requires -Token (job remote trigger token)' }
            $q = "token=$Token&OXIDE_SLOC_REPO=$([uri]::EscapeDataString($Repo))&OXIDE_SLOC_BRANCH=$([uri]::EscapeDataString($Branch))&OXIDE_SLOC_COMMIT=$([uri]::EscapeDataString($Commit))&OXIDE_SLOC_BUILD_ID=$([uri]::EscapeDataString($BuildId))"
            if ($DryRun) { Log "[dry-run] POST $Url ($q)"; return }
            $resp = Invoke-WithRetry -Uri $Url -Headers @{} -Body $q -ContentType 'application/x-www-form-urlencoded'
        }
        'bitbucket' {
            if (-not $Token) { Die 'bitbucket dispatch requires -Token (app password / access token)' }
            $body = @{ target = @{ type = 'pipeline_ref_target'; ref_type = 'branch'; ref_name = $Branch;
                selector = @{ type = 'custom'; pattern = 'oxide-sloc-scan' } } } | ConvertTo-Json -Compress -Depth 5
            if ($DryRun) { Log "[dry-run] POST $Url body: $body"; return }
            $headers = @{ Authorization = "Bearer $Token" }
            $resp = Invoke-WithRetry -Uri $Url -Headers $headers -Body $body -ContentType 'application/json'
        }
        default { Die "unknown -Dispatch kind '$Dispatch'" }
    }
    Log "downstream response: $resp"
}

switch ($Mode) {
    'server'   { Invoke-ServerMode }
    'dispatch' { Invoke-DispatchMode }
    default    { Die "unknown -Mode '$Mode'" }
}
