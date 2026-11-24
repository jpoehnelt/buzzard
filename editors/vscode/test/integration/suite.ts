// Runs inside the extension host of a real VS Code. There is no test
// framework. A rejected promise from `run` fails the run.

import assert from 'node:assert/strict';
import * as path from 'node:path';
import * as vscode from 'vscode';

function log(message: string): void {
  process.stderr.write(`[buzzard integration] ${message}\n`);
}

async function waitFor<T>(what: string, probe: () => T | undefined, timeoutMs = 20_000): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = probe();
    if (value !== undefined) {
      return value;
    }
    if (Date.now() > deadline) {
      throw new Error(`timed out waiting for ${what}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

export async function run(): Promise<void> {
  const workspace = process.env['BUZZARD_IT_WORKSPACE'] ?? '';
  const extension = vscode.extensions.getExtension('buzzard.buzzard');
  assert.ok(extension !== undefined, 'the extension is installed');

  const binary = process.env['BUZZARD_BIN'];
  if (binary !== undefined) {
    await vscode.workspace.getConfiguration('buzzard').update('path', binary, vscode.ConfigurationTarget.Global);
  }
  const api = (await extension.activate()) as { serverState(): string; problem(): string | undefined };
  log(`activated; server state: ${api.serverState()}; problem: ${String(api.problem())}; folders: ${vscode.workspace.workspaceFolders?.length ?? 0}; buzzard.path: ${String(vscode.workspace.getConfiguration('buzzard').get('path'))}`);

  const commands = await vscode.commands.getCommands(true);
  for (const command of ['buzzard.showSummary', 'buzzard.showDiagram', 'buzzard.findCallers', 'buzzard.restartServer']) {
    assert.ok(commands.includes(command), command);
  }
  log('commands are registered');

  if (binary === undefined) {
    log('BUZZARD_BIN is not set: skipping the checks that need the server');
    return;
  }

  // Diagnostics arrive for a file that no editor has open.
  const broken = vscode.Uri.file(path.join(workspace, 'Broken.vi'));
  await waitFor('the server to run', () => (api.serverState() === 'running' ? true : undefined));
  log('the server is running');
  const diagnostics = await waitFor('diagnostics for Broken.vi', () => {
    const found = vscode.languages.getDiagnostics(broken);
    return found.length > 0 ? found : undefined;
  });
  assert.ok(
    diagnostics.some((diagnostic) => diagnostic.source === 'buzzard' && diagnostic.code === 'BZ000'),
    JSON.stringify(diagnostics),
  );
  assert.equal(vscode.workspace.textDocuments.filter((document) => document.uri.fsPath === broken.fsPath).length, 0);
  log(`diagnostics published for an unopened file: ${diagnostics.map((d) => JSON.stringify(d.code)).join(', ')}`);

  // The viewer opens as a custom editor, and the summary and diagram panels open.
  await vscode.commands.executeCommand('vscode.openWith', broken, 'buzzard.viewer');
  const tabs = (): vscode.Tab[] => vscode.window.tabGroups.all.flatMap((group) => group.tabs);
  await waitFor('the viewer tab', () =>
    tabs().find((tab) => tab.input instanceof vscode.TabInputCustom && tab.input.viewType === 'buzzard.viewer'),
  );
  log('the custom editor opened');

  if (process.env['BUZZARD_FIXTURE'] !== undefined) {
    const fixture = vscode.Uri.file(path.join(workspace, 'Fixture.vi'));
    await vscode.commands.executeCommand('buzzard.showSummary', fixture);
    await vscode.commands.executeCommand('buzzard.showDiagram', fixture);
    // The list of tabs reaches the extension host a moment after a panel opens.
    for (const label of ['Summary: Fixture.vi', 'Diagram: Fixture.vi']) {
      await waitFor(`the tab "${label}"`, () => tabs().find((tab) => tab.label === label));
    }
    log('summary and diagram panels opened');
  }

  await vscode.commands.executeCommand('buzzard.restartServer');
  await waitFor('diagnostics after a restart', () => {
    const found = vscode.languages.getDiagnostics(broken);
    return found.length > 0 ? found : undefined;
  });
  log('server restarted');

  // A missing executable is reported with the install command.
  const configuration = vscode.workspace.getConfiguration('buzzard');
  await configuration.update('path', path.join(workspace, 'no-such-buzzard'), vscode.ConfigurationTarget.Global);
  const problem = await waitFor('the missing executable to be reported', () => {
    const text = api.problem();
    return text?.includes('was not found') === true ? text : undefined;
  });
  assert.ok(problem.includes('cargo install --path crates/buzzard-cli'), problem);
  assert.equal(api.serverState(), 'none');
  log('a missing executable is reported with the install command');
  await configuration.update('path', binary, vscode.ConfigurationTarget.Global);
  await waitFor('the server to run again', () => (api.serverState() === 'running' ? true : undefined));
  log('the server runs again after the setting is corrected');
}
