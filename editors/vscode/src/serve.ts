// Manage the oxide-sloc web UI (`serve`) as a child process.

import * as vscode from 'vscode';
import { ChildProcess, spawn } from 'child_process';
import { resolveBinary } from './binary';
import { serveArgs } from './runner';

let serveProcess: ChildProcess | undefined;

function setRunning(running: boolean): void {
  void vscode.commands.executeCommand('setContext', 'oxideSloc.serveRunning', running);
}

export function isServeRunning(): boolean {
  return serveProcess !== undefined;
}

/** Start the web UI and open it in a browser. No-op with a notice if already running. */
export async function startServe(output: vscode.OutputChannel): Promise<void> {
  if (serveProcess) {
    void vscode.window.showInformationMessage('Oxide SLOC web UI is already running.');
    return;
  }
  const port = vscode.workspace.getConfiguration('oxideSloc').get<number>('serve.port', 4317);
  const bin = resolveBinary();
  const cwd = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;

  const child = spawn(bin, serveArgs(port), { cwd, shell: false });
  serveProcess = child;
  setRunning(true);

  child.stdout?.on('data', (d) => output.append(d.toString()));
  child.stderr?.on('data', (d) => output.append(d.toString()));

  child.on('error', (err) => {
    output.appendLine(`\n[serve] failed to start: ${err.message}`);
    void vscode.window.showErrorMessage(`Oxide SLOC: failed to start web UI: ${err.message}`);
    serveProcess = undefined;
    setRunning(false);
  });

  child.on('close', (code) => {
    output.appendLine(`\n[serve] exited with code ${code ?? 'unknown'}`);
    serveProcess = undefined;
    setRunning(false);
  });

  // Give the server a moment to bind before opening the browser.
  await new Promise((r) => setTimeout(r, 800));
  if (serveProcess) {
    void vscode.env.openExternal(vscode.Uri.parse(`http://127.0.0.1:${port}`));
    void vscode.window.showInformationMessage(`Oxide SLOC web UI started on http://127.0.0.1:${port}`);
  }
}

/** Stop the web UI child process, if running. */
export function stopServe(): void {
  if (!serveProcess) {
    void vscode.window.showInformationMessage('Oxide SLOC web UI is not running.');
    return;
  }
  serveProcess.kill();
  serveProcess = undefined;
  setRunning(false);
}
