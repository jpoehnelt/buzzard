// HTML for the webviews. This module does not import `vscode`.
//
// Everything shown comes from files that may be hostile (a file name, a
// name stored inside a VI, the SVG drawn from one). So the module follows
// three rules:
//
// - The webviews run no script, and the Content-Security-Policy
//   allows none. It allows one stylesheet (by nonce) and `data:` images.
// - Text is escaped. Markdown is rendered with raw HTML, links, and images
//   switched off.
// - The SVG is never put into the page as markup. It is shown through
//   `<img src="data:image/svg+xml;base64,...">`. An SVG loaded as an image
//   cannot run script or load anything from outside itself.

import { randomBytes } from 'node:crypto';
import MarkdownIt from 'markdown-it';

/** A larger picture is not embedded. The page says so instead. */
export const MAX_SVG_BYTES = 16 * 1024 * 1024;

export function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

export function newNonce(): string {
  return randomBytes(18).toString('base64url');
}

function checkedNonce(nonce: string): string {
  if (!/^[A-Za-z0-9_-]{16,}$/.test(nonce)) {
    throw new Error('invalid nonce');
  }
  return nonce;
}

export function contentSecurityPolicy(nonce: string): string {
  return [
    "default-src 'none'",
    'img-src data:',
    `style-src 'nonce-${checkedNonce(nonce)}'`,
    "base-uri 'none'",
    "form-action 'none'",
  ].join('; ');
}

const markdown = new MarkdownIt({ html: false, linkify: false, typographer: false }).disable([
  'link',
  'image',
  'autolink',
  'reference',
]);

/** Markdown to HTML with raw HTML escaped and no links or images. */
export function renderMarkdown(source: string): string {
  return markdown.render(source);
}

/** True if the text looks like the SVG document the tool prints. */
export function looksLikeSvg(text: string): boolean {
  const start = text.charCodeAt(0) === 0xfeff ? 1 : 0;
  return /^\s*(<\?xml[^>]*\?>\s*)?(<!--[\s\S]*?-->\s*)*<svg[\s>]/.test(text.slice(start, start + 4096));
}

/** A `data:` URI for an SVG document. The document is Base64-encoded, so no character of it is markup. */
export function svgDataUri(svg: string): string {
  return `data:image/svg+xml;base64,${Buffer.from(svg, 'utf8').toString('base64')}`;
}

export type Section =
  | { kind: 'markdown'; markdown: string }
  | { kind: 'svg'; svg: string; alt: string }
  | { kind: 'note'; text: string }
  | { kind: 'error'; text: string };

export interface Page {
  title: string;
  /** Shown under the title, for example the path of the file. */
  subtitle?: string;
  sections: readonly Section[];
}

const STYLE = `
body { padding: 0 1.5rem 2rem; line-height: 1.5; color: var(--vscode-foreground); font-family: var(--vscode-font-family); font-size: var(--vscode-font-size); }
h1 { font-size: 1.4em; margin: 1.2rem 0 0.2rem; overflow-wrap: anywhere; }
h2 { font-size: 1.15em; margin-top: 1.6rem; }
h3 { font-size: 1.05em; }
.subtitle { margin: 0 0 1rem; opacity: 0.75; overflow-wrap: anywhere; }
table { border-collapse: collapse; margin: 0.6rem 0; }
th, td { border: 1px solid var(--vscode-editorWidget-border, rgba(128, 128, 128, 0.4)); padding: 0.2rem 0.6rem; text-align: left; vertical-align: top; overflow-wrap: anywhere; }
code, pre { font-family: var(--vscode-editor-font-family, monospace); font-size: 0.95em; }
pre { overflow-x: auto; padding: 0.6rem; background: var(--vscode-textCodeBlock-background, rgba(128, 128, 128, 0.15)); }
.diagram { overflow: auto; max-height: 85vh; margin: 0.8rem 0; padding: 0.5rem; background: #ffffff; border: 1px solid var(--vscode-editorWidget-border, rgba(128, 128, 128, 0.4)); }
.diagram img { display: block; max-width: none; }
.note { opacity: 0.8; font-style: italic; }
.error { color: var(--vscode-errorForeground, #c00000); overflow-wrap: anywhere; }
`;

function renderSection(section: Section): string {
  switch (section.kind) {
    case 'markdown':
      return `<section class="markdown">${renderMarkdown(section.markdown)}</section>`;
    case 'svg': {
      if (Buffer.byteLength(section.svg, 'utf8') > MAX_SVG_BYTES) {
        return '<p class="note">The diagram is too large to preview.</p>';
      }
      if (!looksLikeSvg(section.svg)) {
        return '<p class="error">The diagram could not be shown: the output is not an SVG picture.</p>';
      }
      return `<div class="diagram"><img alt="${escapeHtml(section.alt)}" src="${svgDataUri(section.svg)}"></div>`;
    }
    case 'note':
      return `<p class="note">${escapeHtml(section.text)}</p>`;
    case 'error':
      return `<p class="error">${escapeHtml(section.text)}</p>`;
  }
}

/** A complete HTML document for a webview. */
export function renderPage(page: Page, nonce: string = newNonce()): string {
  const csp = contentSecurityPolicy(nonce);
  const subtitle = page.subtitle === undefined ? '' : `<p class="subtitle">${escapeHtml(page.subtitle)}</p>\n`;
  return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta http-equiv="Content-Security-Policy" content="${escapeHtml(csp)}">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>${escapeHtml(page.title)}</title>
<style nonce="${nonce}">${STYLE}</style>
</head>
<body>
<h1>${escapeHtml(page.title)}</h1>
${subtitle}${page.sections.map(renderSection).join('\n')}
</body>
</html>
`;
}
