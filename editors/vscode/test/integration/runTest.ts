// Starts a real VS Code (downloaded on first use) with the extension and
// runs `suite.ts` inside it. Needs a display: on Linux, run under xvfb.
//
//   BUZZARD_BIN=/path/to/buzzard xvfb-run -a npm run test:integration

import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { runTests } from '@vscode/test-electron';

async function main(): Promise<void> {
  const extensionDevelopmentPath = path.resolve(__dirname, '..', '..');
  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'buzzard-vscode-it-'));
  const fixture = process.env['BUZZARD_FIXTURE'];
  if (fixture !== undefined) {
    fs.copyFileSync(fixture, path.join(workspace, 'Fixture.vi'));
  }
  // A file that is not a valid container: the server must report BZ000.
  fs.writeFileSync(path.join(workspace, 'Broken.vi'), 'this is not a resource container');
  try {
    await runTests({
      extensionDevelopmentPath,
      extensionTestsPath: path.resolve(__dirname, 'suite.js'),
      launchArgs: [workspace, '--disable-extensions', '--disable-workspace-trust'],
      extensionTestsEnv: { BUZZARD_IT_WORKSPACE: workspace },
    });
  } finally {
    fs.rmSync(workspace, { recursive: true, force: true });
  }
}

main().catch((error: unknown) => {
  process.stderr.write(`integration tests failed: ${String(error)}\n`);
  process.exit(1);
});
