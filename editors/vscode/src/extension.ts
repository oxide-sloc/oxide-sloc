// oxide-sloc VS Code extension entry point.

import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';
import { spawnSloc } from './binary';
import { analyzeArgs, mapExit } from './runner';
import { parsePlain, codeLines } from './plain';
import { SlocStatusBar } from './statusBar';
import { surfaceOutcome } from './diagnostics';
import { openReport } from './report';
import { startServe, stopServe } from './serve';

let statusBar: SlocStatusBar;
let output: vscode.OutputChannel;
let reportDir: string;
let lastReportHtml: string | undefined;

export function activate(context: vscode.ExtensionContext): void {
  output = vscode.window.createOutputChannel('Oxide SLOC');
  statusBar = new SlocStatusBar();
  context.subscriptions.push(output, statusBar);

  // Reports go to extension storage, never the workspace tree.
  reportDir = path.join(context.globalStorageUri.fsPath, 'reports');
  fs.mkdirSync(reportDir, { recursive: true });

  context.subscriptions.push(
    vscode.commands.registerCommand('oxideSloc.analyzeWorkspace', analyzeWorkspace),
    vscode.commands.registerCommand('oxideSloc.analyzeCurrent', analyzeCurrent),
    vscode.commands.registerCommand('oxideSloc.openReport', openLastReport),
    vscode.commands.registerCommand('oxideSloc.startServe', () => startServe(output)),
    vscode.commands.registerCommand('oxideSloc.stopServe', () => stopServe()),
    vscode.commands.registerCommand('oxideSloc.refreshStatus', () => statusBar.refresh()),
  );

  // Refresh the status bar on save when the user opted in.
  context.subscriptions.push(
    vscode.workspace.onDidSaveTextDocument(() => {
      const auto = vscode.workspace
        .getConfiguration('oxideSloc')
        .get<boolean>('statusBar.autoRefresh', false);
      if (auto) {
        void statusBar.refresh();
      }
    }),
  );

  // React to relevant setting changes.
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration('oxideSloc.statusBar.enabled')) {
        statusBar.applyVisibility();
        void statusBar.refresh();
      }
    }),
  );

  statusBar.applyVisibility();
  void statusBar.refresh();
}

export function deactivate(): void {
  stopServe();
}

/** Timestamped report path pair under the extension storage dir. */
function reportPaths(label: string): { json: string; html: string } {
  const safe = label.replace(/[^a-zA-Z0-9._-]+/g, '_').slice(0, 60) || 'report';
  const stamp = Date.now();
  return {
    json: path.join(reportDir, `${safe}-${stamp}.json`),
    html: path.join(reportDir, `${safe}-${stamp}.html`),
  };
}

async function runAnalyze(paths: string[], label: string): Promise<void> {
  if (paths.length === 0) {
    void vscode.window.showWarningMessage('Oxide SLOC: nothing to analyze.');
    return;
  }
  const { json, html } = reportPaths(label);

  await vscode.window.withProgress(
    { location: vscode.ProgressLocation.Window, title: 'Oxide SLOC: analyzing...' },
    async () => {
      const result = await spawnSloc(
        analyzeArgs(paths, { jsonOut: json, htmlOut: html }),
        paths[0],
      );

      output.appendLine(`\n$ oxide-sloc analyze ${paths.join(' ')}`);
      if (result.stdout) {
        output.append(result.stdout);
      }

      if (result.spawnError) {
        output.appendLine(`[error] ${result.spawnError.message}`);
        void vscode.window.showErrorMessage(
          `Oxide SLOC: could not run the binary (${result.spawnError.message}). ` +
            'Set "oxideSloc.binaryPath" or add oxide-sloc to PATH.',
        );
        return;
      }
      if (result.stderr) {
        output.appendLine(result.stderr.trimEnd());
      }

      const metrics = parsePlain(result.stdout);
      const meaning = mapExit(result.code);
      lastReportHtml = fs.existsSync(html) ? html : undefined;

      // Update the status bar from this run's numbers if we scanned the workspace.
      const code = codeLines(metrics);
      if (code !== undefined) {
        void statusBar.refresh();
      }

      await surfaceOutcome(meaning, metrics, lastReportHtml, output);
    },
  );
}

async function analyzeWorkspace(): Promise<void> {
  const folders = vscode.workspace.workspaceFolders;
  if (!folders || folders.length === 0) {
    void vscode.window.showWarningMessage('Oxide SLOC: no workspace folder is open.');
    return;
  }
  await runAnalyze(folders.map((f) => f.uri.fsPath), 'workspace');
}

async function analyzeCurrent(target?: vscode.Uri): Promise<void> {
  const uri = target ?? vscode.window.activeTextEditor?.document.uri;
  if (!uri) {
    void vscode.window.showWarningMessage('Oxide SLOC: no file or folder selected.');
    return;
  }
  await runAnalyze([uri.fsPath], path.basename(uri.fsPath));
}

async function openLastReport(): Promise<void> {
  if (!lastReportHtml || !fs.existsSync(lastReportHtml)) {
    void vscode.window.showInformationMessage(
      'Oxide SLOC: no report yet. Run "Oxide SLOC: Analyze Workspace" first.',
    );
    return;
  }
  await openReport(lastReportHtml);
}
