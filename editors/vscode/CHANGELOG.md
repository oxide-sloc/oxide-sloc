# Changelog

All notable changes to the oxide-sloc VS Code extension are documented here.

## [0.1.0] - Initial release

- Analyze the workspace or the current file/folder via the command palette or context menus.
- Status-bar item showing the workspace code-line count, refreshed on demand (and on save when enabled).
- Open the generated HTML report in the default browser or a webview panel.
- Start/stop the oxide-sloc web UI from within VS Code.
- Configurable binary path (setting, `SLOC_BIN`/`OXIDE_SLOC`, or `PATH`), extra analyze flags, and config file.
- oxide-sloc exit-code gates (warnings, below-threshold, budget, baseline growth, complexity) surfaced as notifications.
