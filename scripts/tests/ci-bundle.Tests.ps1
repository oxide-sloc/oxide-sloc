<#
  Windows integration test for the standalone CI bundle
  (`oxide-sloc analyze --ci-bundle`).

  Scenario mirrors a real adopter build: a C++ project carrying GoogleTest /
  CTest-style source tests, plus an imported LCOV coverage file. The test then
  asserts that the full standalone artifact set is produced — HTML, PDF, XLSX,
  JSON, the machine-readable CI summary, and the CI dashboard — and that the
  run clearly separates lexical test detections from imported coverage.

  PDF export here uses oxide-sloc's pure-Rust PDF writer (write_pdf_from_run),
  so NO Chromium/Edge install is required on the runner.

  Run: Invoke-Pester scripts/tests/ci-bundle.Tests.ps1
#>

BeforeAll {
    $script:RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path

    # Locate a built binary; prefer release, fall back to debug, else build one.
    function Resolve-OxideBinary {
        foreach ($rel in @('target\release\oxide-sloc.exe', 'target\debug\oxide-sloc.exe')) {
            $p = Join-Path $script:RepoRoot $rel
            if (Test-Path $p) { return $p }
        }
        # Build (unset CC to avoid airgap-devkit gcc conflict on Windows).
        $env:CC = $null
        Push-Location $script:RepoRoot
        try {
            & cargo build -p oxide-sloc 2>&1 | Out-Null
        } finally {
            Pop-Location
        }
        $p = Join-Path $script:RepoRoot 'target\debug\oxide-sloc.exe'
        if (Test-Path $p) { return $p }
        throw "could not find or build oxide-sloc.exe"
    }

    $script:Bin = Resolve-OxideBinary

    # ── Build a throwaway C++ project with GoogleTest/CTest-style tests ──────────
    $script:Work = Join-Path $env:TEMP ("sloc-cibundle-" + [guid]::NewGuid().ToString('N'))
    $script:Proj = Join-Path $script:Work 'proj'
    New-Item -ItemType Directory -Force -Path (Join-Path $script:Proj 'src')   | Out-Null
    New-Item -ItemType Directory -Force -Path (Join-Path $script:Proj 'tests') | Out-Null

    Set-Content -Path (Join-Path $script:Proj 'src\calc.cpp') -NoNewline -Value @'
// Small library under test.
#include "calc.h"

int add(int a, int b) {
    return a + b;
}

int mul(int a, int b) {
    int result = 0;
    for (int i = 0; i < b; ++i) {
        result = add(result, a);
    }
    return result;
}
'@

    Set-Content -Path (Join-Path $script:Proj 'src\calc.h') -NoNewline -Value @'
#pragma once
int add(int a, int b);
int mul(int a, int b);
'@

    Set-Content -Path (Join-Path $script:Proj 'tests\calc_test.cpp') -NoNewline -Value @'
#include <gtest/gtest.h>
#include "calc.h"

TEST(CalcSuite, Adds) {
    EXPECT_EQ(add(2, 3), 5);
    EXPECT_EQ(add(-1, 1), 0);
}

TEST(CalcSuite, Multiplies) {
    EXPECT_EQ(mul(3, 4), 12);
    ASSERT_NE(mul(0, 9), 1);
}
'@

    # ── LCOV coverage file whose SF paths match the scanned relative paths ───────
    $script:Lcov = Join-Path $script:Work 'coverage.info'
    Set-Content -Path $script:Lcov -NoNewline -Value @'
TN:
SF:src/calc.cpp
FN:4,add
FN:8,mul
FNDA:5,add
FNDA:2,mul
FNF:2
FNH:2
DA:4,5
DA:5,5
DA:8,2
DA:9,2
DA:10,6
DA:11,6
DA:13,2
LF:7
LH:7
end_of_record
'@

    # ── Run the standalone CI bundle ─────────────────────────────────────────────
    $script:Out = Join-Path $script:Work 'bundle'
    $env:CC = $null
    & $script:Bin analyze $script:Proj --coverage-file $script:Lcov `
        --ci-bundle --out-dir $script:Out --report-title 'CI Bundle Test' --quiet 2>&1 | Out-Null
    $script:ExitCode = $LASTEXITCODE

    function Read-Json([string] $path) {
        if (-not (Test-Path $path)) { return $null }
        Get-Content $path -Raw | ConvertFrom-Json
    }
    $script:Result  = Read-Json (Join-Path $script:Out 'result.json')
    $script:Summary = Read-Json (Join-Path $script:Out 'summary.json')
}

AfterAll {
    if ($script:Work -and (Test-Path $script:Work)) {
        Remove-Item -Recurse -Force $script:Work -ErrorAction SilentlyContinue
    }
}

Describe 'oxide-sloc analyze --ci-bundle (C++ + LCOV)' {
    It 'exits successfully' {
        $script:ExitCode | Should -Be 0
    }

    Context 'emitted artifact set' {
        It 'writes the HTML report'           { Test-Path (Join-Path $script:Out 'report.html')      | Should -BeTrue }
        It 'writes the PDF report'            { Test-Path (Join-Path $script:Out 'report.pdf')       | Should -BeTrue }
        It 'writes the XLSX report'           { Test-Path (Join-Path $script:Out 'report.xlsx')      | Should -BeTrue }
        It 'writes the CSV report'            { Test-Path (Join-Path $script:Out 'report.csv')       | Should -BeTrue }
        It 'writes the result JSON'           { Test-Path (Join-Path $script:Out 'result.json')      | Should -BeTrue }
        It 'writes the scan-config JSON'      { Test-Path (Join-Path $script:Out 'scan-config.json') | Should -BeTrue }
        It 'writes the machine-readable summary' { Test-Path (Join-Path $script:Out 'summary.json')  | Should -BeTrue }
        It 'writes the CI dashboard index'    { Test-Path (Join-Path $script:Out 'index.html')       | Should -BeTrue }
        It 'writes the CSP-compatible report' { Test-Path (Join-Path $script:Out 'csp\report.html')  | Should -BeTrue }
        It 'writes the local-import bundle'   { Test-Path (Join-Path $script:Out 'import\run.json')  | Should -BeTrue }
    }

    It 'produces a non-trivial PDF' {
        (Get-Item (Join-Path $script:Out 'report.pdf')).Length | Should -BeGreaterThan 1000
    }

    Context 'PDF is linked from the HTML report' {
        It 'references the sibling PDF so the offline View-PDF button works' {
            (Get-Content (Join-Path $script:Out 'report.html') -Raw) |
                Should -Match 'data-standalone-pdf="report\.pdf"'
        }
    }

    Context 'CSP-compatible report has externalized assets' {
        It 'has no inline <style> blocks' {
            (Get-Content (Join-Path $script:Out 'csp\report.html') -Raw) |
                Should -Not -Match '<style'
        }
        It 'links an external stylesheet' {
            (Get-Content (Join-Path $script:Out 'csp\report.html') -Raw) |
                Should -Match 'assets/report\.css'
        }
        It 'materializes the external JS assets' {
            (Get-ChildItem (Join-Path $script:Out 'csp\assets') -Filter 'report-*.js').Count |
                Should -BeGreaterThan 0
        }
    }

    Context 'lexical test detection' {
        It 'detects GoogleTest test functions as lexical tests' {
            $script:Result.summary_totals.test_count | Should -BeGreaterThan 0
        }
        It 'detects test assertions' {
            $script:Result.summary_totals.test_assertion_count | Should -BeGreaterThan 0
        }
        It 'labels lexical tests distinctly in the summary payload' {
            $script:Summary.lexical_tests.note | Should -Match 'not executed test results'
        }
    }

    Context 'imported coverage' {
        It 'matches the LCOV file to scanned sources' {
            $script:Result.coverage_diagnostics.outcome        | Should -Be 'matched'
            $script:Result.coverage_diagnostics.files_matched  | Should -BeGreaterThan 0
        }
        It 'aggregates imported line coverage in the summary' {
            $script:Summary.coverage.source      | Should -Be 'imported'
            $script:Summary.coverage.lines_found | Should -BeGreaterThan 0
        }
    }

    Context 'machine-readable summary schema' {
        It 'stamps the stable schema id' {
            $script:Summary.schema | Should -Be 'oxide-sloc.ci-summary/v1'
        }
        It 'carries the CLI report title (overrides defaults)' {
            $script:Summary.report_title | Should -Be 'CI Bundle Test'
        }
        It 'includes the language breakdown' {
            ($script:Summary.languages | Where-Object { $_.language -match 'C' }).Count |
                Should -BeGreaterThan 0
        }
    }
}
