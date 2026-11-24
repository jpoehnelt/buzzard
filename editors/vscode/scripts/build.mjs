// Bundles the extension (default) or the tests (--tests) with esbuild.
// `tsc --noEmit` checks the types separately.
import { build } from 'esbuild';
import { readdirSync } from 'node:fs';

const production = process.argv.includes('--production');
const tests = process.argv.includes('--tests');

const common = {
  bundle: true,
  platform: 'node',
  format: 'cjs',
  target: 'node20',
  external: ['vscode'],
  logLevel: 'warning',
};

if (tests) {
  const unit = readdirSync('test/unit')
    .filter((name) => name.endsWith('.test.ts'))
    .sort()
    .map((name) => `test/unit/${name}`);
  await build({ ...common, entryPoints: unit, outdir: 'out/unit', sourcemap: 'inline' });
  await build({
    ...common,
    entryPoints: ['test/integration/runTest.ts', 'test/integration/suite.ts'],
    outdir: 'out/integration',
    external: ['vscode', '@vscode/test-electron'],
    sourcemap: 'inline',
  });
} else {
  await build({
    ...common,
    entryPoints: ['src/extension.ts'],
    outfile: 'dist/extension.js',
    minify: production,
    sourcemap: !production,
  });
}
