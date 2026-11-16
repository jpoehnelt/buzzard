// Path and URI helpers. Pure; no `vscode` import.

import * as path from 'node:path';

export type FileKind = 'vi' | 'ctl' | 'lvproj' | 'lvlib' | 'lvclass';

const KINDS: readonly FileKind[] = ['vi', 'ctl', 'lvproj', 'lvlib', 'lvclass'];

/** The last component of a path written with `/` or `\`. */
export function baseName(file: string): string {
  const trimmed = file.replace(/[\\/]+$/, '');
  const cut = Math.max(trimmed.lastIndexOf('/'), trimmed.lastIndexOf('\\'));
  return trimmed.slice(cut + 1);
}

/** The kind of a supported file by its extension (any letter case), else `undefined`. */
export function fileKind(file: string): FileKind | undefined {
  const name = baseName(file);
  const dot = name.lastIndexOf('.');
  if (dot <= 0) {
    return undefined;
  }
  const extension = name.slice(dot + 1).toLowerCase();
  return KINDS.find((kind) => kind === extension);
}

export function isSupported(file: string): boolean {
  return fileKind(file) !== undefined;
}

/** Only files with a binary resource container can have a block diagram. */
export function hasDiagram(file: string): boolean {
  const kind = fileKind(file);
  return kind === 'vi' || kind === 'ctl';
}

/**
 * How a file is shown in a list: relative to the workspace folder that
 * holds it (the deepest one, if folders are nested), with `/` separators,
 * else the path as it is.
 */
export function relativeLabel(file: string, roots: readonly string[], platform: NodeJS.Platform = process.platform): string {
  const api = platform === 'win32' ? path.win32 : path.posix;
  let best: string | undefined;
  let bestRootLength = -1;
  for (const root of roots) {
    const relative = api.relative(root, file);
    if (relative === '' || relative.startsWith('..') || api.isAbsolute(relative)) {
      continue;
    }
    if (root.length > bestRootLength) {
      best = relative;
      bestRootLength = root.length;
    }
  }
  return (best ?? file).split(api.sep).join('/');
}

/**
 * The `file:` URIs of an LSP `Location[]` answer, without duplicates and
 * sorted. Anything that is not a location with a string `uri` is dropped,
 * so a malformed answer never throws.
 */
export function locationUris(answer: unknown): string[] {
  if (!Array.isArray(answer)) {
    return [];
  }
  const uris = new Set<string>();
  for (const item of answer as unknown[]) {
    if (typeof item !== 'object' || item === null) {
      continue;
    }
    const uri = (item as { uri?: unknown }).uri;
    if (typeof uri === 'string' && /^file:/i.test(uri)) {
      uris.add(uri);
    }
  }
  return [...uris].sort();
}

/** The Markdown text of an LSP `Hover` answer, or `undefined`. */
export function hoverMarkdown(answer: unknown): string | undefined {
  if (typeof answer !== 'object' || answer === null) {
    return undefined;
  }
  const contents = (answer as { contents?: unknown }).contents;
  const one = (value: unknown): string | undefined => {
    if (typeof value === 'string') {
      return value;
    }
    if (typeof value === 'object' && value !== null) {
      const text = (value as { value?: unknown }).value;
      return typeof text === 'string' ? text : undefined;
    }
    return undefined;
  };
  const texts = (Array.isArray(contents) ? (contents as unknown[]) : [contents])
    .map(one)
    .filter((text): text is string => text !== undefined && text.trim() !== '');
  return texts.length === 0 ? undefined : texts.join('\n\n');
}
