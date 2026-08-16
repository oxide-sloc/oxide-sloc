// Surface oxide-sloc warnings and exit-code gates to the user.

import * as vscode from 'vscode';
import { PlainMetrics } from './plain';
import { ExitMeaning } from './runner';

/**
 * Show the outcome of an analyze run: a toast whose severity matches the exit
 * code, plus a note when warnings were emitted. Offers to open the report when
 * one was produced.
 */
export async function surfaceOutcome(
  meaning: ExitMeaning,
  metrics: PlainMetrics,
  htmlPath: string | undefined,
  output: vscode.OutputChannel,
): Promise<void> {
  const warnCount = metrics.warnings.length;
  if (warnCount > 0) {
    output.appendLine(`\n[analyze] ${warnCount} warning(s):`);
    for (const w of metrics.warnings) {
      output.appendLine(`  - ${w}`);
    }
  }

  const openAction = htmlPath ? 'Open Report' : undefined;
  const parts = [meaning.message];
  if (warnCount > 0) {
    parts.push(`${warnCount} warning(s).`);
  }
  const text = `Oxide SLOC: ${parts.join(' ')}`;

  let choice: string | undefined;
  if (meaning.severity === 'error') {
    choice = await vscode.window.showErrorMessage(text, ...(openAction ? [openAction] : []));
  } else if (meaning.severity === 'warning' || warnCount > 0) {
    choice = await vscode.window.showWarningMessage(text, ...(openAction ? [openAction] : []));
  } else {
    // Success with no warnings: keep it quiet - the status bar already reflects it.
    return;
  }

  if (choice === openAction && htmlPath) {
    const { openReport } = await import('./report');
    await openReport(htmlPath);
  }
}
