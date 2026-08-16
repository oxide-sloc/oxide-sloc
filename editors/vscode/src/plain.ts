// Parse oxide-sloc `--plain` output (one key=value per line) into a typed record.

export interface PlainMetrics {
  raw: Map<string, string>;
  warnings: string[];
}

/** Parse `--plain` stdout. Repeated `warning=` lines are collected separately. */
export function parsePlain(stdout: string): PlainMetrics {
  const raw = new Map<string, string>();
  const warnings: string[] = [];
  for (const line of stdout.split(/\r?\n/)) {
    const eq = line.indexOf('=');
    if (eq < 0) {
      continue;
    }
    const key = line.slice(0, eq);
    const value = line.slice(eq + 1);
    if (key === 'warning') {
      warnings.push(value);
    } else {
      raw.set(key, value);
    }
  }
  return { raw, warnings };
}

function num(metrics: PlainMetrics, key: string): number | undefined {
  const v = metrics.raw.get(key);
  if (v === undefined) {
    return undefined;
  }
  const n = Number(v);
  return Number.isFinite(n) ? n : undefined;
}

export const codeLines = (m: PlainMetrics): number | undefined => num(m, 'code_lines');
export const filesAnalyzed = (m: PlainMetrics): number | undefined => num(m, 'files_analyzed');
export const commentLines = (m: PlainMetrics): number | undefined => num(m, 'comment_lines');
export const blankLines = (m: PlainMetrics): number | undefined => num(m, 'blank_lines');
export const complexity = (m: PlainMetrics): number | undefined => num(m, 'cyclomatic_complexity');
export const unitTests = (m: PlainMetrics): number | undefined => num(m, 'unit_tests');
export const warningCount = (m: PlainMetrics): number => num(m, 'warning_count') ?? m.warnings.length;

/**
 * Compact number formatting matching the oxide-sloc UI convention:
 *   < 1,000        -> as-is
 *   1,000-9,999    -> comma-separated
 *   10,000-999,999 -> one-decimal K (trailing .0 stripped)
 *   >= 1,000,000   -> one-decimal M (trailing .0 stripped)
 */
export function fmt(n: number): string {
  const a = Math.abs(n);
  if (a >= 1e6) {
    return (n / 1e6).toFixed(1).replace(/\.0$/, '') + 'M';
  }
  if (a >= 1e4) {
    return (n / 1e3).toFixed(1).replace(/\.0$/, '') + 'K';
  }
  return n.toLocaleString('en-US');
}
