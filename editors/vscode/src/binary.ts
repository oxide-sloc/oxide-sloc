// Locate and run the oxide-sloc executable.

import * as vscode from 'vscode';
import { spawn } from 'child_process';

export interface SpawnResult {
  code: number | null;
  stdout: string;
  stderr: string;
  /** Set when the process could not be started at all (e.g. binary not found). */
  spawnError?: Error;
}

/**
 * Resolve the oxide-sloc binary, in order:
 *   1. the `oxideSloc.binaryPath` setting, if non-empty
 *   2. the `SLOC_BIN` / `OXIDE_SLOC` environment variables
 *   3. the bare name `oxide-sloc` (resolved via PATH by the OS)
 */
export function resolveBinary(): string {
  const configured = vscode.workspace
    .getConfiguration('oxideSloc')
    .get<string>('binaryPath', '')
    .trim();
  if (configured) {
    return configured;
  }
  const fromEnv = (process.env.SLOC_BIN || process.env.OXIDE_SLOC || '').trim();
  if (fromEnv) {
    return fromEnv;
  }
  return 'oxide-sloc';
}

/**
 * Run oxide-sloc with the given argv (no shell - args are passed as an array,
 * so paths with spaces need no quoting). Resolves with captured stdout/stderr
 * and the exit code; never rejects.
 */
export function spawnSloc(args: string[], cwd?: string): Promise<SpawnResult> {
  const bin = resolveBinary();
  return new Promise((resolve) => {
    const child = spawn(bin, args, { cwd, shell: false });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (d) => {
      stdout += d.toString();
    });
    child.stderr.on('data', (d) => {
      stderr += d.toString();
    });
    child.on('error', (err) => {
      resolve({ code: null, stdout, stderr, spawnError: err });
    });
    child.on('close', (code) => {
      resolve({ code, stdout, stderr });
    });
  });
}
