# Editor and Build Integrations

oxide-sloc is a single self-contained binary, so integrating it into an editor or build system is a
matter of shelling out to `oxide-sloc analyze` and reading its output. This page covers the two
integrations that ship in this repository:

- [CMake](#cmake) - a reusable module that adds a report target to any CMake project.
- [VS Code](#vs-code) - an extension that runs analyses and shows live SLOC in the editor.

> The Visual Studio (VSIX) extension lives in its own repository,
> [oxide-sloc-visualstudio](https://github.com/oxide-sloc/oxide-sloc-visualstudio) - the .NET/VSSDK
> toolchain does not fit a Rust build. Visual Studio users can also point the CMake integration at
> "Open Folder"/CMake projects.

## How the binary is located

Every integration resolves the `oxide-sloc` executable in the same order:

1. an explicit path (the `oxideSloc.binaryPath` VS Code setting, or `-D OXIDE_SLOC=<path>` in CMake),
2. the `SLOC_BIN` / `OXIDE_SLOC` environment variables,
3. `oxide-sloc` on `PATH`.

This mirrors the convention used by the MCP server, so a single `SLOC_BIN` export works everywhere.

## Exit codes

Both integrations surface oxide-sloc's exit codes so a tripped gate becomes an editor notification or
a failed build step:

| Exit | Meaning | Triggered by |
|------|---------|--------------|
| 0 | Success | - |
| 1 | Generic error | Bad path, binary not found, etc. |
| 2 | Warnings gate | `--fail-on-warnings` |
| 3 | Code lines below threshold | `--fail-below <n>` |
| 4 | SLOC budget exceeded | `--fail-on-budget` (budget defined in `.oxide-sloc.toml`) |
| 5 | Growth exceeded baseline | `--fail-above-baseline <name>` |
| 6 | Cyclomatic complexity exceeded | `--max-complexity <n>` |

## CMake

The [`cmake/OxideSloc.cmake`](../cmake/OxideSloc.cmake) module adds an `oxide_sloc_add_report()`
function that registers a custom target (never part of `ALL`) invoking `oxide-sloc analyze`.

```cmake
list(APPEND CMAKE_MODULE_PATH "${CMAKE_SOURCE_DIR}/cmake")
include(OxideSloc)

oxide_sloc_add_report(
  NAME sloc
  PATHS ${CMAKE_SOURCE_DIR}/src ${CMAKE_SOURCE_DIR}/include
  HTML JSON
  FAIL_BELOW 100)
```

Then:

```sh
cmake -S . -B build
cmake --build build --target sloc
```

The report target writes `build/sloc.html` and `build/sloc.json` and prints a machine-readable
summary. Gating options map to the exit codes above (`FAIL_BELOW`, `FAIL_ON_BUDGET`,
`FAIL_ON_WARNINGS`); a non-zero exit fails the build step. Use the `REPORT_ONLY` option for a
non-gating target that always succeeds.

A complete, runnable example lives in [`examples/cmake/`](../examples/cmake/). See the header comment
in [`OxideSloc.cmake`](../cmake/OxideSloc.cmake) for every option.

## VS Code

The [`editors/vscode/`](../editors/vscode/) extension adds command-palette entries, Explorer context
menus, and a status-bar code-line count.

- **Oxide SLOC: Analyze Workspace** / **Analyze Current File/Folder** - scan and report.
- **Oxide SLOC: Open HTML Report** - view the latest report (external browser or webview).
- **Oxide SLOC: Start/Stop Web UI** - manage the oxide-sloc dashboard on http://127.0.0.1:4317.
- A status-bar item shows the workspace code-line count; click it to re-analyze.

Reports are written to the extension's storage directory, never into the workspace. Key settings:
`oxideSloc.binaryPath`, `oxideSloc.analyzeFlags`, `oxideSloc.configPath`, `oxideSloc.report.viewer`,
`oxideSloc.statusBar.enabled`. See the extension [README](../editors/vscode/README.md) for the full
list and build instructions (`npm install && npm run compile && npx @vscode/vsce package`).
