import assert from 'node:assert/strict';
import { test } from 'node:test';

import { contentSecurityPolicy, escapeHtml, looksLikeSvg, MAX_SVG_BYTES, newNonce, renderMarkdown, renderPage, svgDataUri } from '../../src/html';

const NONCE = 'abcdefghijklmnopqrstuvwx';
const HOSTILE_NAME = `"><script>alert(1)</script><img src=x onerror=alert(2)>'&.vi`;
const HOSTILE_SVG = `<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)"><script>alert(2)</script><foreignObject><iframe src="https://example.com"></iframe></foreignObject><image href="https://example.com/x.png"/><text>"></img><script>alert(3)</script></text></svg>`;

/** Every tag of a document, by name. */
function tags(html: string): string[] {
  return [...html.matchAll(/<([a-zA-Z][a-zA-Z0-9]*)/g)].map((match) => (match[1] ?? '').toLowerCase());
}

const SAFE_TAGS = new Set([
  'html', 'head', 'meta', 'title', 'style', 'body', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'p', 'section', 'div', 'img',
  'table', 'thead', 'tbody', 'tr', 'th', 'td', 'ul', 'ol', 'li', 'code', 'pre', 'strong', 'em', 'blockquote', 'hr', 'br', 's',
]);

function assertInert(html: string): void {
  for (const tag of tags(html)) {
    assert.ok(SAFE_TAGS.has(tag), `unexpected <${tag}>`);
  }
  // No attribute can start an event handler or a navigation: outside the
  // data URI, every quote that came from the input is escaped.
  // Quoted attribute values are text; what is left of each tag is its
  // attribute names.
  for (const tag of html.matchAll(/<[a-zA-Z](?:"[^"]*"|[^>"])*>/g)) {
    const names = tag[0].replace(/"[^"]*"/g, '""');
    assert.ok(!/\son[a-z]+\s*=/i.test(names), `event handler attribute in ${tag[0].slice(0, 80)}`);
    assert.ok(!/\sstyle\s*=/i.test(names), 'inline style attribute');
  }
  assert.ok(!/<a[\s>]/i.test(html), 'link');
  assert.ok(!/(src|href)\s*=\s*["']?\s*(https?|javascript|file|vscode)/i.test(html), 'outside reference');
  const images = [...html.matchAll(/<img\b[^>]*>/g)].map((match) => match[0]);
  for (const image of images) {
    assert.match(image, /^<img alt="[^"<>]*" src="data:image\/svg\+xml;base64,[A-Za-z0-9+/=]*">$/);
  }
}

test('escapeHtml escapes the five special characters', () => {
  assert.equal(escapeHtml(`<a href="x" title='y'>&</a>`), '&lt;a href=&quot;x&quot; title=&#39;y&#39;&gt;&amp;&lt;/a&gt;');
  assert.equal(escapeHtml(''), '');
});

test('nonces are long, URL-safe and differ from each other', () => {
  const nonces = new Set(Array.from({ length: 50 }, newNonce));
  assert.equal(nonces.size, 50);
  for (const nonce of nonces) {
    assert.match(nonce, /^[A-Za-z0-9_-]{24}$/);
  }
});

test('the policy allows no script, no network and no inline style without the nonce', () => {
  const policy = contentSecurityPolicy(NONCE);
  assert.equal(
    policy,
    `default-src 'none'; img-src data:; style-src 'nonce-${NONCE}'; base-uri 'none'; form-action 'none'`,
  );
  assert.ok(!policy.includes('unsafe'));
  assert.ok(!policy.includes('script-src'));
  assert.ok(!/https?:|\*/.test(policy));
});

test('a nonce that could break out of the policy is refused', () => {
  for (const bad of ['', 'short', `abc' 'unsafe-inline' x1234567890`, 'abcdefghijklmnop; script-src *', 'abcdefghijklmnop"']) {
    assert.throws(() => contentSecurityPolicy(bad));
    assert.throws(() => renderPage({ title: 't', sections: [] }, bad));
  }
});

test('Markdown is rendered: headings, tables, lists, emphasis, code', () => {
  const html = renderMarkdown('### Main.vi\n\n| | |\n| --- | --- |\n| Kind | VI (`LVIN`) |\n\n**Dependencies (1)**\n\n- `Sub.vi`\n');
  assert.ok(html.includes('<h3>Main.vi</h3>'));
  assert.ok(html.includes('<td>Kind</td>'));
  assert.ok(html.includes('<code>LVIN</code>'));
  assert.ok(html.includes('<strong>Dependencies (1)</strong>'));
  assert.ok(html.includes('<li><code>Sub.vi</code></li>'));
});

test('Markdown cannot carry HTML, links or images', () => {
  const hostile = [
    '<script>alert(1)</script>',
    '<img src=x onerror=alert(1)>',
    '[click](https://example.com)',
    '[click](javascript:alert(1))',
    '![pixel](https://example.com/p.png)',
    '<https://example.com>',
    'https://example.com',
    '[ref]\n\n[ref]: https://example.com',
    '| a |\n| --- |\n| <b onmouseover=alert(1)>x</b> |',
    '```<script>\n</pre><script>alert(1)</script>\n```',
    '`</code><script>alert(1)</script>`',
  ].join('\n\n');
  const html = renderMarkdown(hostile);
  assertInert(html);
  assert.ok(!html.includes('<script'));
  assert.ok(!html.includes('<a '));
  assert.ok(!html.includes('<img'));
  assert.ok(html.includes('&lt;script&gt;'));
});

test('looksLikeSvg accepts SVG documents only', () => {
  assert.equal(looksLikeSvg('<svg xmlns="http://www.w3.org/2000/svg"></svg>'), true);
  assert.equal(looksLikeSvg('\n  <?xml version="1.0"?>\n<!-- c -->\n<svg>'), true);
  assert.equal(looksLikeSvg(`${String.fromCharCode(0xfeff)}<svg>`), true);
  for (const text of ['', 'error: no', '<html><svg>', '<svgx>', '<script>', 'x<svg>']) {
    assert.equal(looksLikeSvg(text), false, text);
  }
});

test('an SVG becomes a base64 data URI that decodes to the same bytes', () => {
  const svg = '<svg xmlns="http://www.w3.org/2000/svg"><text>Größe "→" \'</text></svg>';
  const uri = svgDataUri(svg);
  assert.match(uri, /^data:image\/svg\+xml;base64,[A-Za-z0-9+/=]+$/);
  assert.equal(Buffer.from(uri.slice(uri.indexOf(',') + 1), 'base64').toString('utf8'), svg);
});

test('a page has the policy, the nonce on its one stylesheet, and no script', () => {
  const html = renderPage({ title: 'Main.vi', subtitle: '/w/Main.vi', sections: [{ kind: 'note', text: 'Loading' }] }, NONCE);
  assert.ok(html.startsWith('<!DOCTYPE html>'));
  assert.ok(
    html.includes(
      `<meta http-equiv="Content-Security-Policy" content="default-src &#39;none&#39;; img-src data:; style-src &#39;nonce-${NONCE}&#39;; base-uri &#39;none&#39;; form-action &#39;none&#39;">`,
    ),
  );
  assert.equal([...html.matchAll(/<style/g)].length, 1);
  assert.ok(html.includes(`<style nonce="${NONCE}">`));
  assert.ok(!html.includes('<script'));
  assert.ok(!/style\s*=/.test(html.replace(/<style[\s\S]*<\/style>/, '')), 'inline style attribute');
  assert.ok(html.includes('<h1>Main.vi</h1>'));
  assert.ok(html.includes('<p class="subtitle">/w/Main.vi</p>'));
  assertInert(html);
});

test('the same input and nonce give the same page', () => {
  const page = { title: 'a', sections: [{ kind: 'markdown', markdown: '# x' }] } as const;
  assert.equal(renderPage(page, NONCE), renderPage(page, NONCE));
  assert.notEqual(renderPage(page), renderPage(page));
});

test('a hostile file name is escaped everywhere it appears', () => {
  const html = renderPage(
    {
      title: HOSTILE_NAME,
      subtitle: `/w/${HOSTILE_NAME}`,
      sections: [
        { kind: 'error', text: `No summary: ${HOSTILE_NAME}: bad` },
        { kind: 'note', text: HOSTILE_NAME },
        { kind: 'markdown', markdown: `### ${HOSTILE_NAME}\n\n| Kind | ${HOSTILE_NAME} |\n| --- | --- |\n` },
        { kind: 'svg', svg: '<svg xmlns="http://www.w3.org/2000/svg"/>', alt: `Block diagram of ${HOSTILE_NAME}` },
      ],
    },
    NONCE,
  );
  assertInert(html);
  assert.ok(!html.includes('<script'));
  assert.ok(html.includes('&lt;script&gt;alert(1)&lt;/script&gt;'));
  assert.ok(html.includes('alt="Block diagram of &quot;&gt;&lt;script&gt;'));
});

test('hostile SVG content never becomes markup of the page', () => {
  const html = renderPage({ title: 'x.vi', sections: [{ kind: 'svg', svg: HOSTILE_SVG, alt: 'd' }] }, NONCE);
  assertInert(html);
  for (const fragment of ['<svg', '<script', 'onload', '<iframe', 'foreignObject', 'example.com']) {
    assert.ok(!html.includes(fragment), fragment);
  }
  const encoded = /src="data:image\/svg\+xml;base64,([A-Za-z0-9+/=]+)"/.exec(html)?.[1];
  assert.ok(encoded !== undefined);
  assert.equal(Buffer.from(encoded, 'base64').toString('utf8'), HOSTILE_SVG);
});

test('output that is not an SVG, or is too large, is not embedded', () => {
  const notSvg = renderPage({ title: 'x', sections: [{ kind: 'svg', svg: '<html><script>alert(1)</script>', alt: 'd' }] }, NONCE);
  assert.ok(!notSvg.includes('<img'));
  assert.ok(notSvg.includes('not an SVG picture'));
  assertInert(notSvg);

  const huge = `<svg>${' '.repeat(MAX_SVG_BYTES)}</svg>`;
  const tooLarge = renderPage({ title: 'x', sections: [{ kind: 'svg', svg: huge, alt: 'd' }] }, NONCE);
  assert.ok(!tooLarge.includes('<img'));
  assert.ok(tooLarge.includes('too large to preview'));
  assert.ok(tooLarge.length < 10_000);
});
