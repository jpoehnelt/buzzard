// Running `buzzard inspect` for previews. The executable is started
// directly, never through a shell, so a file name is only ever one argument.
// `inspect` reads the file and prints; nothing is written.

import { execFile } from 'node:child_process';

export type InspectFormat = 'markdown' | 'svg';

export const DEFAULT_TIMEOUT_MS = 20_000;
/** Upper bound for the output kept from one run. */
export const MAX_OUTPUT_BYTES = 32 * 1024 * 1024;

/**
 * Arguments for `buzzard inspect`. The `--` keeps a file whose name starts
 * with `-` from being read as an option.
 */
export function inspectArgs(file: string, format: InspectFormat): string[] {
  return ['inspect', '--format', format, '--', file];
}

export type RunResult = { ok: true; stdout: string } | { ok: false; message: string };

export interface RunOptions {
  timeoutMs?: number;
  maxOutputBytes?: number;
}

/** The first lines of what the tool wrote to stderr, without its `error: ` prefix. */
export function errorText(stderr: string, fallback: string): string {
  const text = stderr
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line !== '')
    .slice(0, 3)
    .join(' ')
    .replace(/^error:\s*/i, '');
  return (text === '' ? fallback : text).slice(0, 500);
}

/** Run an executable and collect its standard output. Never rejects. */
export function run(binary: string, args: readonly string[], options: RunOptions = {}): Promise<RunResult> {
  return new Promise((resolve) => {
    try {
      execFile(
        binary,
        [...args],
        {
          encoding: 'utf8',
          timeout: options.timeoutMs ?? DEFAULT_TIMEOUT_MS,
          maxBuffer: options.maxOutputBytes ?? MAX_OUTPUT_BYTES,
          windowsHide: true,
          shell: false,
        },
        (error, stdout, stderr) => {
          if (error === null) {
            resolve({ ok: true, stdout });
            return;
          }
          const failure = error as NodeJS.ErrnoException & { killed?: boolean };
          let fallback = error.message;
          if (failure.code === 'ENOENT') {
            fallback = `cannot run ${binary}: not found`;
          } else if (failure.code === 'ERR_CHILD_PROCESS_STDIO_MAXBUFFER') {
            resolve({ ok: false, message: 'the output is too large to show' });
            return;
          } else if (failure.killed === true) {
            resolve({ ok: false, message: 'buzzard did not answer in time' });
            return;
          }
          resolve({ ok: false, message: errorText(stderr, fallback) });
        },
      );
    } catch (error) {
      resolve({ ok: false, message: error instanceof Error ? error.message : String(error) });
    }
  });
}

export function inspect(binary: string, file: string, format: InspectFormat, options: RunOptions = {}): Promise<RunResult> {
  return run(binary, inspectArgs(file, format), options);
}
