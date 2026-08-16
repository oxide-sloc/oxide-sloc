// Build oxide-sloc argument vectors and interpret exit codes.

import * as vscode from 'vscode';

export interface AnalyzeOutputs {
  jsonOut?: string;
  htmlOut?: string;
  /** Append --open so oxide-sloc opens the report itself. */
  open?: boolean;
}

/** User-configured extra flags and config path, shared by analyze invocations. */
function commonAnalyzeFlags(): string[] {
  const cfg = vscode.workspace.getConfiguration('oxideSloc');
  const args: string[] = [];
  const configPath = cfg.get<string>('configPath', '').trim();
  if (configPath) {
    args.push('--config', configPath);
  }
  const extra = cfg.get<string[]>('analyzeFlags', []);
  for (const flag of extra) {
    if (flag && flag.length > 0) {
      args.push(flag);
    }
  }
  return args;
}

/** `oxide-sloc analyze <paths...> --plain [outputs] [user flags]` */
export function analyzeArgs(paths: string[], outputs: AnalyzeOutputs = {}): string[] {
  const args = ['analyze', ...paths, '--plain'];
  if (outputs.jsonOut) {
    args.push('--json-out', outputs.jsonOut);
  }
  if (outputs.htmlOut) {
    args.push('--html-out', outputs.htmlOut);
  }
  if (outputs.open) {
    args.push('--open');
  }
  args.push(...commonAnalyzeFlags());
  return args;
}

/** `oxide-sloc report <json> --html-out <html> [--open]` */
export function reportArgs(jsonPath: string, htmlOut: string, open = false): string[] {
  const args = ['report', jsonPath, '--html-out', htmlOut];
  if (open) {
    args.push('--open');
  }
  return args;
}

/** `oxide-sloc serve` on the configured port (via SLOC_BIND semantics passed as --bind). */
export function serveArgs(port: number): string[] {
  return ['serve', '--bind', `127.0.0.1:${port}`];
}

export interface ExitMeaning {
  ok: boolean;
  severity: 'info' | 'warning' | 'error';
  message: string;
}

/** Map an oxide-sloc exit code to a human-facing meaning. */
export function mapExit(code: number | null): ExitMeaning {
  switch (code) {
    case 0:
      return { ok: true, severity: 'info', message: 'Analysis complete.' };
    case 2:
      return { ok: false, severity: 'warning', message: 'Warnings gate failed (--fail-on-warnings).' };
    case 3:
      return { ok: false, severity: 'warning', message: 'Code lines below threshold (--fail-below).' };
    case 4:
      return { ok: false, severity: 'error', message: 'SLOC budget exceeded (--fail-on-budget).' };
    case 5:
      return { ok: false, severity: 'error', message: 'Growth exceeded baseline (--fail-above-baseline).' };
    case 6:
      return { ok: false, severity: 'error', message: 'Cyclomatic complexity exceeded (--max-complexity).' };
    default:
      return {
        ok: false,
        severity: 'error',
        message: `oxide-sloc exited with code ${code ?? 'unknown'}.`,
      };
  }
}
