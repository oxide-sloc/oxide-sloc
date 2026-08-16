// Status-bar item showing the workspace code-line count.

import * as vscode from 'vscode';
import { spawnSloc } from './binary';
import { analyzeArgs } from './runner';
import { parsePlain, codeLines, filesAnalyzed, warningCount, fmt } from './plain';

export class SlocStatusBar {
  private item: vscode.StatusBarItem;
  private refreshing = false;

  constructor() {
    this.item = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
    this.item.command = 'oxideSloc.analyzeWorkspace';
    this.item.name = 'Oxide SLOC';
  }

  dispose(): void {
    this.item.dispose();
  }

  /** Show/hide based on the `oxideSloc.statusBar.enabled` setting. */
  applyVisibility(): void {
    const enabled = vscode.workspace
      .getConfiguration('oxideSloc')
      .get<boolean>('statusBar.enabled', true);
    if (enabled) {
      this.item.show();
    } else {
      this.item.hide();
    }
  }

  /** Re-run analyze across workspace folders and update the item text. */
  async refresh(): Promise<void> {
    if (this.refreshing) {
      return;
    }
    const enabled = vscode.workspace
      .getConfiguration('oxideSloc')
      .get<boolean>('statusBar.enabled', true);
    if (!enabled) {
      this.item.hide();
      return;
    }

    const folders = vscode.workspace.workspaceFolders;
    if (!folders || folders.length === 0) {
      this.item.hide();
      return;
    }

    this.refreshing = true;
    this.item.text = '$(sync~spin) SLOC';
    this.item.tooltip = 'Oxide SLOC: analyzing...';
    this.item.show();

    try {
      const paths = folders.map((f) => f.uri.fsPath);
      const result = await spawnSloc(analyzeArgs(paths), paths[0]);
      if (result.spawnError) {
        this.item.text = '$(error) SLOC';
        this.item.tooltip = `Oxide SLOC: ${result.spawnError.message}`;
        return;
      }
      const metrics = parsePlain(result.stdout);
      const code = codeLines(metrics);
      if (code === undefined) {
        this.item.text = '$(question) SLOC';
        this.item.tooltip = 'Oxide SLOC: no code_lines in output';
        return;
      }
      this.item.text = `$(code) ${fmt(code)} SLOC`;
      const files = filesAnalyzed(metrics);
      const warns = warningCount(metrics);
      const lines = [
        `Code lines: ${code.toLocaleString('en-US')}`,
        files !== undefined ? `Files analyzed: ${files.toLocaleString('en-US')}` : undefined,
        warns > 0 ? `Warnings: ${warns}` : undefined,
        'Click to analyze the workspace.',
      ].filter(Boolean);
      this.item.tooltip = lines.join('\n');
    } finally {
      this.refreshing = false;
    }
  }
}
