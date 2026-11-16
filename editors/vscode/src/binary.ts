// Finding the `buzzard` executable. Pure: the file system and the
// environment are passed in, so this is tested without touching either.

import * as path from 'node:path';

/** The command shown to a user whose `buzzard` executable is missing. */
export const INSTALL_COMMAND = 'cargo install --path crates/buzzard-cli';

/** Where the sources for the install command come from. */
export const REPOSITORY_URL = 'https://github.com/jpoehnelt/buzzard';

export const DEFAULT_BINARY = 'buzzard';

export interface ResolveEnvironment {
  platform: NodeJS.Platform;
  /** Environment variables; names are matched without regard to case on Windows. */
  env: Readonly<Record<string, string | undefined>>;
  /** The user's home directory, for a leading `~`. */
  homeDir?: string | undefined;
  /** The first workspace folder, for relative paths with a directory part. */
  workspaceDir?: string | undefined;
  /** True if `candidate` is an existing regular file. Must not throw. */
  isFile: (candidate: string) => boolean;
}

export type Resolution =
  | { found: true; path: string }
  | { found: false; configured: string; searched: string[] };

function pathApi(platform: NodeJS.Platform): path.PlatformPath {
  return platform === 'win32' ? path.win32 : path.posix;
}

function envValue(environment: ResolveEnvironment, name: string): string | undefined {
  const direct = environment.env[name];
  if (direct !== undefined || environment.platform !== 'win32') {
    return direct;
  }
  const wanted = name.toLowerCase();
  // Sorted, so that the answer does not depend on insertion order.
  for (const key of Object.keys(environment.env).sort()) {
    if (key.toLowerCase() === wanted) {
      return environment.env[key];
    }
  }
  return undefined;
}

/** The file names to try for one configured name: itself, then PATHEXT on Windows. */
function withExtensions(name: string, environment: ResolveEnvironment): string[] {
  if (environment.platform !== 'win32') {
    return [name];
  }
  const names = [name];
  if (path.win32.extname(name) === '') {
    const pathExt = envValue(environment, 'PATHEXT') ?? '.COM;.EXE;.BAT;.CMD';
    for (const extension of pathExt.split(';')) {
      const trimmed = extension.trim();
      if (trimmed.startsWith('.') && trimmed.length > 1) {
        names.push(name + trimmed.toLowerCase());
      }
    }
  }
  return [...new Set(names)];
}

/** A `.bat` or `.cmd` cannot be started without a shell, and no shell is ever used. */
function isRunnable(candidate: string, environment: ResolveEnvironment): boolean {
  if (environment.platform !== 'win32') {
    return true;
  }
  const extension = path.win32.extname(candidate).toLowerCase();
  return extension !== '.bat' && extension !== '.cmd';
}

/**
 * Resolve the `buzzard.path` setting to an existing file.
 *
 * - A bare name (`buzzard`) is looked up in every `PATH` directory, in order.
 * - `~` or `~/...` is taken from the home directory.
 * - An absolute path is used as it is.
 * - A relative path with a directory part is taken from the workspace folder.
 *
 * The current directory is never searched for a bare name.
 */
export function resolveBinary(setting: string | undefined, environment: ResolveEnvironment): Resolution {
  const api = pathApi(environment.platform);
  const trimmed = (setting ?? '').trim();
  const configured = trimmed === '' ? DEFAULT_BINARY : trimmed;
  const searched: string[] = [];

  const attempt = (base: string): string | undefined => {
    for (const candidate of withExtensions(base, environment)) {
      searched.push(candidate);
      if (isRunnable(candidate, environment) && environment.isFile(candidate)) {
        return candidate;
      }
    }
    return undefined;
  };
  const answer = (found: string | undefined): Resolution =>
    found === undefined ? { found: false, configured, searched } : { found: true, path: found };

  if (configured.includes('\0')) {
    return { found: false, configured, searched };
  }

  const hasSeparator = configured.includes('/') || (environment.platform === 'win32' && configured.includes('\\'));

  if (configured === '~' || configured.startsWith('~/') || (environment.platform === 'win32' && configured.startsWith('~\\'))) {
    if (environment.homeDir === undefined || environment.homeDir === '') {
      return { found: false, configured, searched };
    }
    return answer(attempt(api.join(environment.homeDir, configured.slice(1))));
  }
  if (api.isAbsolute(configured)) {
    return answer(attempt(api.normalize(configured)));
  }
  if (hasSeparator) {
    if (environment.workspaceDir === undefined || environment.workspaceDir === '') {
      return { found: false, configured, searched };
    }
    return answer(attempt(api.resolve(environment.workspaceDir, configured)));
  }

  const pathVariable = envValue(environment, 'PATH') ?? '';
  for (const raw of pathVariable.split(api.delimiter)) {
    // Windows allows quoted entries. An empty or relative entry would mean
    // "relative to the current directory" and is skipped on purpose.
    const directory = environment.platform === 'win32' ? raw.replace(/^"(.*)"$/, '$1') : raw;
    if (directory === '' || !api.isAbsolute(directory)) {
      continue;
    }
    const found = attempt(api.join(directory, configured));
    if (found !== undefined) {
      return { found: true, path: found };
    }
  }
  return { found: false, configured, searched };
}

/** The message shown when the executable cannot be found. One line, plain text. */
export function missingBinaryMessage(configured: string): string {
  const shown = Array.from(configured)
    .map((character) => ((character.codePointAt(0) ?? 0) < 0x20 || character === '\x7f' ? ' ' : character))
    .join('')
    .slice(0, 200);
  const where =
    shown === DEFAULT_BINARY
      ? `The "${DEFAULT_BINARY}" executable was not found on PATH.`
      : `The buzzard executable "${shown}" (setting buzzard.path) was not found.`;
  return `${where} Install it from a checkout of ${REPOSITORY_URL} with: ${INSTALL_COMMAND}`;
}
