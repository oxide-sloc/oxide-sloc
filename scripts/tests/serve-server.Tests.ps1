<#
  Pester tests for scripts/internal/install-service.ps1 (Pester 5 syntax).

  These drive the installer in -DryRun mode: it writes the runtime wrapper and
  prints the planned scheduled-task + firewall commands WITHOUT requiring admin,
  registering a task, or touching the firewall. That lets CI validate the
  command construction, API-key generation, and env wiring on a hosted runner.

  Run: Invoke-Pester scripts/tests/serve-server.Tests.ps1
#>

BeforeAll {
    $script:InstallerPath = Join-Path $PSScriptRoot '..\internal\install-service.ps1'
    $script:InstallDir = Join-Path $env:TEMP ("sloc-pester-" + [guid]::NewGuid().ToString('N'))
    function Invoke-Installer {
        # NB: parameter must NOT be named $Args — that shadows PowerShell's
        # automatic $args and the @-splat would pass nothing to the child process.
        param([string[]] $InstallerArgs)
        & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $script:InstallerPath @InstallerArgs 2>&1 |
            Out-String
    }
}

AfterAll {
    if (Test-Path $script:InstallDir) { Remove-Item -Recurse -Force $script:InstallDir -ErrorAction SilentlyContinue }
}

Describe 'install-service.ps1 (dry-run install)' {
    BeforeAll {
        $script:Out = Invoke-Installer -InstallerArgs @(
            '-DryRun', '-InstallDir', $script:InstallDir,
            '-Bind', '0.0.0.0:4321',
            '-TlsCert', 'C:\certs\server.crt', '-TlsKey', 'C:\certs\server.key',
            '-AllowedRoots', 'C:\repos')
        $script:Wrapper = Join-Path $script:InstallDir 'run-service.cmd'
        $script:WrapperText = if (Test-Path $script:Wrapper) { Get-Content $script:Wrapper -Raw } else { '' }
    }

    It 'writes the runtime wrapper' {
        Test-Path $script:Wrapper | Should -BeTrue
    }
    It 'generates a 256-bit hex API key' {
        $script:WrapperText | Should -Match 'SLOC_API_KEYS=[0-9a-f]{64}'
    }
    It 'sets the requested bind address' {
        $script:WrapperText | Should -Match 'SLOC_BIND=0\.0\.0\.0:4321'
    }
    It 'wires the TLS cert and key' {
        $script:WrapperText | Should -Match 'SLOC_TLS_CERT=C:\\certs\\server\.crt'
        $script:WrapperText | Should -Match 'SLOC_TLS_KEY=C:\\certs\\server\.key'
    }
    It 'wires the allowed-roots scan allowlist' {
        $script:WrapperText | Should -Match 'SLOC_ALLOWED_ROOTS=C:\\repos'
    }
    It 'launches the server in --server mode on the bind address' {
        $script:WrapperText | Should -Match 'serve --server --bind 0\.0\.0\.0:4321'
    }
    It 'plans an all-profiles firewall rule on the derived port' {
        $script:Out | Should -Match 'netsh advfirewall firewall add rule name=oxide-sloc-4321'
        $script:Out | Should -Match 'localport=4321'
        $script:Out | Should -Match 'remoteip=any'
    }
    It 'plans a SYSTEM scheduled-task action' {
        $script:Out | Should -Match 'task action: cmd\.exe /c'
    }
    It 'does not attempt UAC elevation in dry-run' {
        $script:Out | Should -Not -Match 'Re-launching with an elevation'
    }
}

Describe 'install-service.ps1 (dry-run uninstall)' {
    BeforeAll {
        $script:UOut = Invoke-Installer -InstallerArgs @('-Uninstall', '-DryRun', '-InstallDir', $script:InstallDir)
    }
    It 'plans scheduled-task removal' {
        $script:UOut | Should -Match 'Unregister-ScheduledTask'
    }
    It 'plans firewall-rule removal' {
        $script:UOut | Should -Match 'delete rule name=oxide-sloc'
    }
    It 'plans install-directory removal (clears the stored secret)' {
        $script:UOut | Should -Match 'Remove-Item -Recurse -Force'
    }
}
