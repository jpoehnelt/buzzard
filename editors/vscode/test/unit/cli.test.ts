import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { test } from 'node:test';

import { errorText, inspect, inspectArgs, run } from '../../src/cli';
import { looksLikeSvg, renderPage } from '../../src/html';

const NODE = process.execPath;

test('inspect arguments keep the file as one argument after `--`', () => {
  assert.deepEqual(inspectArgs('/w/Main.vi', 'svg'), ['inspect', '--format', 'svg', '--', '/w/Main.vi']);
  assert.deepEqual(inspectArgs('--format=json; rm -rf ~ $(x) `y`.vi', 'markdown'), [
    'inspect',
    '--format',
    'markdown',
    '--',
    '--format=json; rm -rf ~ $(x) `y`.vi',
  ]);
});

test('errorText takes the first lines of stderr without the prefix', () => {
  assert.equal(errorText('error: /x.vi: No such file or directory (os error 2)\n', 'f'), '/x.vi: No such file or directory (os error 2)');
  assert.equal(errorText('\n  \n', 'fallback'), 'fallback');
  assert.equal(errorText('a\nb\nc\nd\n', 'f'), 'a b c');
  assert.equal(errorText('x'.repeat(5000), 'f').length, 500);
});

test('run returns the standard output of a successful process', async () => {
  const result = await run(NODE, ['-e', 'process.stdout.write("<svg/>")']);
  assert.deepEqual(result, { ok: true, stdout: '<svg/>' });
});

test('run passes arguments through without a shell', async () => {
  const hostile = '$(echo pwned) `id` ; echo x | cat > /dev/null && "quoted" \'single\' *';
  const result = await run(NODE, ['-e', 'process.stdout.write(JSON.stringify(process.argv.slice(1)))', '--', hostile]);
  assert.deepEqual(result, { ok: true, stdout: JSON.stringify([hostile]) });
});

test('run reports a failing process with what it wrote to stderr', async () => {
  const result = await run(NODE, ['-e', 'process.stderr.write("error: broken file\\n"); process.exit(2)']);
  assert.deepEqual(result, { ok: false, message: 'broken file' });
});

test('run reports a missing executable instead of throwing', async () => {
  const result = await run(path.join(os.tmpdir(), 'buzzard-test-no-such-executable'), ['inspect']);
  assert.ok(!result.ok);
  assert.ok(result.message.includes('not found'));
});

test('run stops a process that does not answer in time', async () => {
  const result = await run(NODE, ['-e', 'setTimeout(() => {}, 60000)'], { timeoutMs: 200 });
  assert.deepEqual(result, { ok: false, message: 'buzzard did not answer in time' });
});

test('run refuses output over the limit', async () => {
  const result = await run(NODE, ['-e', 'process.stdout.write("x".repeat(100000))'], { maxOutputBytes: 1000 });
  assert.deepEqual(result, { ok: false, message: 'the output is too large to show' });
});

// With BUZZARD_BIN and BUZZARD_FIXTURE set (CI does this), the real tool is
// run on a real file, which must still be the same file afterwards.
const realBinary = process.env['BUZZARD_BIN'];
const realFixture = process.env['BUZZARD_FIXTURE'];
const skipReal = realBinary === undefined || realFixture === undefined ? 'BUZZARD_BIN and BUZZARD_FIXTURE are not set' : false;

test('the real tool gives an SVG and a summary, and leaves the file alone', { skip: skipReal }, async () => {
  const binary = realBinary ?? '';
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'buzzard-vscode-'));
  try {
    // A name a shell would trip over, and one that looks like an option.
    for (const name of ['-weird $(x) "q" <b>&.vi', '--format=json.vi']) {
      const file = path.join(directory, name);
      fs.copyFileSync(realFixture ?? '', file);
      const before = fs.readFileSync(file);
      const beforeTime = fs.statSync(file).mtimeMs;

      const svg = await inspect(binary, file, 'svg');
      assert.ok(svg.ok, JSON.stringify(svg));
      assert.ok(looksLikeSvg(svg.stdout));
      const summary = await inspect(binary, file, 'markdown');
      assert.ok(summary.ok, JSON.stringify(summary));
      assert.ok(summary.stdout.startsWith('### '));

      const html = renderPage({
        title: name,
        sections: [
          { kind: 'markdown', markdown: summary.stdout },
          { kind: 'svg', svg: svg.stdout, alt: name },
        ],
      });
      assert.ok(!html.includes('<script'));
      assert.ok(!html.includes('<b>'));
      assert.ok(html.includes('<img alt='));

      assert.ok(fs.readFileSync(file).equals(before));
      assert.equal(fs.statSync(file).mtimeMs, beforeTime);
      assert.deepEqual(await inspect(binary, file, 'svg'), svg);
    }
    assert.deepEqual(fs.readdirSync(directory).sort(), ['--format=json.vi', '-weird $(x) "q" <b>&.vi']);

    const missing = await inspect(binary, path.join(directory, 'missing.vi'), 'svg');
    assert.ok(!missing.ok);
    assert.ok(missing.message.includes('missing.vi'));
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});
