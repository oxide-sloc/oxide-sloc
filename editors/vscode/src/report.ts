// Open a generated HTML report, either externally or in a webview panel.

import * as vscode from 'vscode';
import * as fs from 'fs';
import * as path from 'path';

/** Open an HTML report file according to the `oxideSloc.report.viewer` setting. */
export async function openReport(htmlPath: string): Promise<void> {
  const viewer = vscode.workspace
    .getConfiguration('oxideSloc')
    .get<string>('report.viewer', 'external');

  if (viewer === 'webview') {
    openInWebview(htmlPath);
  } else {
    await vscode.env.openExternal(vscode.Uri.file(htmlPath));
  }
}

function openInWebview(htmlPath: string): void {
  const panel = vscode.window.createWebviewPanel(
    'oxideSlocReport',
    'Oxide SLOC Report',
    vscode.ViewColumn.Active,
    {
      enableScripts: true,
      localResourceRoots: [vscode.Uri.file(path.dirname(htmlPath))],
    },
  );
  // The oxide-sloc HTML report is fully self-contained (inline CSS/JS), so we can
  // load its markup directly into the webview.
  panel.webview.html = fs.readFileSync(htmlPath, 'utf8');
}
