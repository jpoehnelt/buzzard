// Connects the extension to VS Code. It starts `buzzard lsp` and adds
// commands and a read-only viewer. The logic that can be tested without an
// editor is in the other modules.

import * as fs from 'node:fs';
import * as os from 'node:os';
import * as vscode from 'vscode';
import { LanguageClient, State } from 'vscode-languageclient/node';
import type { LanguageClientOptions, ServerOptions } from 'vscode-languageclient/node';

import { INSTALL_COMMAND, missingBinaryMessage, resolveBinary } from './binary';
import type { Resolution } from './binary';
import { inspect } from './cli';
import { baseName, hasDiagram, hoverMarkdown, isSupported, locationUris, relativeLabel } from './files';
import { renderPage } from './html';
import type { Section } from './html';

const VIEWER = 'buzzard.viewer';
const FILE_FILTERS = { 'LabVIEW files': ['vi', 'ctl', 'lvproj', 'lvlib', 'lvclass'] };
const DIAGRAM_FILTERS = { 'VIs and controls': ['vi', 'ctl'] };

let client: LanguageClient | undefined;
let status: vscode.StatusBarItem | undefined;
let output: vscode.LogOutputChannel | undefined;
let problem: string | undefined;
/** Starts and stops run one after the other. */
let lifecycle: Promise<void> = Promise.resolve();

function isFile(candidate: string): boolean {
  try {
    return fs.statSync(candidate).isFile();
  } catch {
    return false;
  }
}

function findBinary(): Resolution {
  const setting = vscode.workspace.getConfiguration('buzzard').get<string>('path');
  return resolveBinary(setting, {
    platform: process.platform,
    env: process.env,
    homeDir: os.homedir(),
    workspaceDir: vscode.workspace.workspaceFolders?.[0]?.uri.fsPath,
    isFile,
  });
}

function setStatus(text: string, tooltip: string, warning = false): void {
  problem = warning ? tooltip : undefined;
  if (status === undefined) {
    return;
  }
  status.text = text;
  status.tooltip = `${tooltip}\nClick to restart the server.`;
  status.backgroundColor = warning ? new vscode.ThemeColor('statusBarItem.warningBackground') : undefined;
  status.show();
}

async function reportMissing(resolution: Extract<Resolution, { found: false }>): Promise<void> {
  const message = missingBinaryMessage(resolution.configured);
  setStatus('$(warning) Buzzard: not found', message, true);
  output?.info(message);
  for (const candidate of resolution.searched) {
    output?.info(`  looked for ${candidate}`);
  }
  const copy = 'Copy Install Command';
  const settings = 'Open Settings';
  const choice = await vscode.window.showErrorMessage(message, copy, settings);
  if (choice === copy) {
    await vscode.env.clipboard.writeText(INSTALL_COMMAND);
  } else if (choice === settings) {
    await vscode.commands.executeCommand('workbench.action.openSettings', 'buzzard.path');
  }
}

/** The executable, or `undefined` after telling the user that it is missing. */
function requireBinary(): string | undefined {
  const resolution = findBinary();
  if (resolution.found) {
    return resolution.path;
  }
  void reportMissing(resolution);
  return undefined;
}

async function stopClient(): Promise<void> {
  const running = client;
  client = undefined;
  if (running === undefined) {
    return;
  }
  try {
    await running.dispose();
  } catch (error) {
    output?.info(`Stopping the server failed: ${String(error)}`);
  }
}

async function startClient(): Promise<void> {
  await stopClient();
  if ((vscode.workspace.workspaceFolders ?? []).length === 0) {
    // The server analyzes workspace folders. Without one it has nothing to
    // do. The viewer and the previews still work on single files.
    setStatus('$(circle-slash) Buzzard', 'No folder is open, so the buzzard language server is not running.');
    problem = 'no folder is open';
    return;
  }
  const resolution = findBinary();
  if (!resolution.found) {
    void reportMissing(resolution);
    return;
  }
  const serverOptions: ServerOptions = {
    // No `transport` is set, so the client talks to the executable over
    // stdin and stdout and adds nothing to its arguments. Naming the stdio
    // transport would append `--stdio`, which `buzzard lsp` does not accept.
    command: resolution.path,
    args: ['lsp'],
  };
  const clientOptions: LanguageClientOptions = {
    // The files are binary and are never opened as text documents, so the
    // client synchronizes nothing. Diagnostics arrive by
    // `publishDiagnostics` for the files the server finds in the workspace
    // folders.
    documentSelector: [],
    diagnosticCollectionName: 'buzzard',
    ...(output === undefined ? {} : { outputChannel: output }),
  };
  const created = new LanguageClient('buzzard', 'Buzzard', serverOptions, clientOptions);
  created.onDidChangeState((event) => {
    if (client !== created) {
      return;
    }
    if (event.newState === State.Running) {
      setStatus('$(check) Buzzard', `The buzzard language server is running (${resolution.path}).`);
    } else if (event.newState === State.Starting) {
      setStatus('$(sync~spin) Buzzard', 'The buzzard language server is starting.');
    } else {
      setStatus('$(warning) Buzzard: stopped', 'The buzzard language server is not running.', true);
    }
  });
  client = created;
  setStatus('$(sync~spin) Buzzard', 'The buzzard language server is starting.');
  try {
    await created.start();
  } catch (error) {
    if (client === created) {
      client = undefined;
    }
    const reason = error instanceof Error ? error.message : String(error);
    setStatus('$(warning) Buzzard: failed', `The buzzard language server could not be started: ${reason}`, true);
    void vscode.window.showErrorMessage(`The buzzard language server (${resolution.path}) could not be started: ${reason}`);
  }
}

function restart(): Promise<void> {
  lifecycle = lifecycle.then(startClient, startClient);
  return lifecycle;
}

function runningClient(): LanguageClient | undefined {
  return client?.state === State.Running ? client : undefined;
}

function activeTabUri(): vscode.Uri | undefined {
  const input: unknown = vscode.window.tabGroups.activeTabGroup.activeTab?.input;
  if (input instanceof vscode.TabInputCustom || input instanceof vscode.TabInputText) {
    return input.uri;
  }
  return undefined;
}

/** The file a command is about: the explorer selection, the active tab, or one the user picks. */
async function targetUri(argument: unknown, accepts: (file: string) => boolean, filters: Record<string, string[]>): Promise<vscode.Uri | undefined> {
  const candidates = [argument instanceof vscode.Uri ? argument : undefined, activeTabUri()];
  for (const candidate of candidates) {
    if (candidate?.scheme === 'file' && accepts(candidate.fsPath)) {
      return candidate;
    }
  }
  if (argument instanceof vscode.Uri) {
    void vscode.window.showWarningMessage(`Buzzard cannot do this for ${baseName(argument.fsPath)}.`);
    return undefined;
  }
  const picked = await vscode.window.showOpenDialog({ canSelectMany: false, filters, openLabel: 'Select' });
  const first = picked?.[0];
  return first !== undefined && first.scheme === 'file' && accepts(first.fsPath) ? first : undefined;
}

function position(uri: vscode.Uri, running: LanguageClient): { textDocument: { uri: string }; position: { line: number; character: number } } {
  // A whole file is addressed as position 0:0 of its URI.
  return { textDocument: { uri: running.code2ProtocolConverter.asUri(uri) }, position: { line: 0, character: 0 } };
}

/** The summary of a file. It comes from the server when the server runs, because the server knows the callers, and from `buzzard inspect` otherwise. */
async function summarySection(uri: vscode.Uri, binary: string | undefined): Promise<Section> {
  const running = runningClient();
  if (running !== undefined) {
    try {
      const text = hoverMarkdown(await running.sendRequest<unknown>('textDocument/hover', position(uri, running)));
      if (text !== undefined) {
        return { kind: 'markdown', markdown: text };
      }
    } catch (error) {
      output?.info(`hover failed for ${uri.fsPath}: ${String(error)}`);
    }
  }
  if (binary === undefined) {
    return { kind: 'error', text: missingBinaryMessage(findBinaryName()) };
  }
  const result = await inspect(binary, uri.fsPath, 'markdown');
  return result.ok ? { kind: 'markdown', markdown: result.stdout } : { kind: 'error', text: `No summary: ${result.message}` };
}

function findBinaryName(): string {
  const resolution = findBinary();
  return resolution.found ? resolution.path : resolution.configured;
}

async function diagramSection(uri: vscode.Uri, binary: string | undefined): Promise<Section> {
  if (binary === undefined) {
    return { kind: 'error', text: missingBinaryMessage(findBinaryName()) };
  }
  const result = await inspect(binary, uri.fsPath, 'svg');
  return result.ok
    ? { kind: 'svg', svg: result.stdout, alt: `Block diagram of ${baseName(uri.fsPath)}` }
    : { kind: 'error', text: `No block diagram: ${result.message}` };
}

function quietBinary(): string | undefined {
  const resolution = findBinary();
  return resolution.found ? resolution.path : undefined;
}

async function viewerHtml(uri: vscode.Uri, parts: { summary: boolean; diagram: boolean }): Promise<string> {
  const binary = quietBinary();
  const sections: Section[] = [];
  if (parts.summary) {
    sections.push(await summarySection(uri, binary));
  }
  if (parts.diagram) {
    if (parts.summary) {
      sections.push({ kind: 'markdown', markdown: '## Block diagram picture' });
    }
    sections.push(await diagramSection(uri, binary));
  }
  const title = parts.summary ? baseName(uri.fsPath) : `Block diagram of ${baseName(uri.fsPath)}`;
  return renderPage({ title, subtitle: uri.fsPath, sections });
}

/** No script runs in these webviews and they read no local resource. */
const WEBVIEW_OPTIONS: vscode.WebviewOptions = { enableScripts: false, localResourceRoots: [] };

async function showPanel(uri: vscode.Uri, parts: { summary: boolean; diagram: boolean }): Promise<void> {
  const name = baseName(uri.fsPath);
  const panel = vscode.window.createWebviewPanel(
    parts.summary ? 'buzzard.summary' : 'buzzard.diagram',
    parts.summary ? `Summary: ${name}` : `Diagram: ${name}`,
    vscode.ViewColumn.Active,
    WEBVIEW_OPTIONS,
  );
  panel.webview.html = renderPage({ title: name, sections: [{ kind: 'note', text: 'Loading…' }] });
  const html = await viewerHtml(uri, parts);
  try {
    panel.webview.html = html;
  } catch {
    // The panel was closed in the meantime.
  }
}

async function showSummary(argument: unknown): Promise<void> {
  const uri = await targetUri(argument, isSupported, FILE_FILTERS);
  if (uri === undefined) {
    return;
  }
  if (runningClient() === undefined && requireBinary() === undefined) {
    return;
  }
  await showPanel(uri, { summary: true, diagram: false });
}

async function showDiagram(argument: unknown): Promise<void> {
  const uri = await targetUri(argument, hasDiagram, DIAGRAM_FILTERS);
  if (uri === undefined || requireBinary() === undefined) {
    return;
  }
  await showPanel(uri, { summary: false, diagram: true });
}

async function findCallers(argument: unknown): Promise<void> {
  const uri = await targetUri(argument, isSupported, FILE_FILTERS);
  if (uri === undefined) {
    return;
  }
  const running = runningClient();
  if (running === undefined) {
    if (requireBinary() !== undefined) {
      void vscode.window.showWarningMessage('The buzzard language server is not running. Open a folder, or run "Buzzard: Restart Server".');
    }
    return;
  }
  let answer: unknown;
  try {
    answer = await running.sendRequest<unknown>('textDocument/references', {
      ...position(uri, running),
      context: { includeDeclaration: false },
    });
  } catch (error) {
    void vscode.window.showErrorMessage(`Buzzard could not find the callers: ${error instanceof Error ? error.message : String(error)}`);
    return;
  }
  const name = baseName(uri.fsPath);
  const roots = (vscode.workspace.workspaceFolders ?? []).map((folder) => folder.uri.fsPath);
  const items = locationUris(answer)
    .map((text) => running.protocol2CodeConverter.asUri(text))
    .map((caller) => ({ label: baseName(caller.fsPath), description: relativeLabel(caller.fsPath, roots), uri: caller }))
    .sort((a, b) => (a.description < b.description ? -1 : a.description > b.description ? 1 : 0));
  if (items.length === 0) {
    void vscode.window.showInformationMessage(`No file in the workspace uses ${name}.`);
    return;
  }
  const picked = await vscode.window.showQuickPick(items, {
    title: `Callers of ${name} (${items.length})`,
    placeHolder: 'Select a file to reveal it in the Explorer',
    matchOnDescription: true,
  });
  if (picked !== undefined) {
    await vscode.commands.executeCommand('revealInExplorer', picked.uri);
  }
}

/** The read-only viewer offered for `.vi` and `.ctl` files: summary and diagram. */
class ViewerProvider implements vscode.CustomReadonlyEditorProvider {
  openCustomDocument(uri: vscode.Uri): vscode.CustomDocument {
    return { uri, dispose: () => undefined };
  }

  async resolveCustomEditor(document: vscode.CustomDocument, panel: vscode.WebviewPanel): Promise<void> {
    const uri = document.uri;
    const name = baseName(uri.fsPath);
    panel.webview.options = WEBVIEW_OPTIONS;
    if (uri.scheme !== 'file') {
      panel.webview.html = renderPage({ title: name, sections: [{ kind: 'error', text: 'Buzzard reads files on disk only.' }] });
      return;
    }
    let disposed = false;
    let generation = 0;
    const refresh = async (): Promise<void> => {
      const mine = ++generation;
      const html = await viewerHtml(uri, { summary: true, diagram: hasDiagram(uri.fsPath) });
      if (!disposed && mine === generation) {
        panel.webview.html = html;
      }
    };
    // Another program saves the file. The viewer shows the new state after each save.
    const watcher = vscode.workspace.createFileSystemWatcher(
      new vscode.RelativePattern(vscode.Uri.joinPath(uri, '..'), name.replace(/[\\*?[\]{}]/g, '?')),
    );
    let timer: NodeJS.Timeout | undefined;
    const schedule = (changed: vscode.Uri): void => {
      if (changed.fsPath !== uri.fsPath) {
        return;
      }
      clearTimeout(timer);
      timer = setTimeout(() => void refresh(), 300);
    };
    watcher.onDidChange(schedule);
    watcher.onDidCreate(schedule);
    panel.onDidDispose(() => {
      disposed = true;
      clearTimeout(timer);
      watcher.dispose();
    });
    panel.webview.html = renderPage({ title: name, sections: [{ kind: 'note', text: 'Loading…' }] });
    if (quietBinary() === undefined) {
      requireBinary();
    }
    await refresh();
  }
}

/** What `activate` hands to other extensions and to the integration tests. */
export interface BuzzardApi {
  /** `running`, `starting`, `stopped`, or `none` when no server was started. */
  serverState(): 'running' | 'starting' | 'stopped' | 'none';
  /** Why the server is not running, in words, if it is not. */
  problem(): string | undefined;
}

function serverState(): ReturnType<BuzzardApi['serverState']> {
  switch (client?.state) {
    case undefined:
      return 'none';
    case State.Running:
      return 'running';
    case State.Starting:
      return 'starting';
    default:
      return 'stopped';
  }
}

export async function activate(context: vscode.ExtensionContext): Promise<BuzzardApi> {
  output = vscode.window.createOutputChannel('Buzzard', { log: true });
  status = vscode.window.createStatusBarItem('buzzard.status', vscode.StatusBarAlignment.Left, 0);
  status.name = 'Buzzard';
  status.command = 'buzzard.restartServer';
  context.subscriptions.push(
    output,
    status,
    vscode.commands.registerCommand('buzzard.showSummary', showSummary),
    vscode.commands.registerCommand('buzzard.showDiagram', showDiagram),
    vscode.commands.registerCommand('buzzard.findCallers', findCallers),
    vscode.commands.registerCommand('buzzard.restartServer', restart),
    vscode.window.registerCustomEditorProvider(VIEWER, new ViewerProvider(), {
      supportsMultipleEditorsPerDocument: true,
      webviewOptions: { retainContextWhenHidden: false },
    }),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration('buzzard.path')) {
        void restart();
      }
    }),
    // The server learns its folders once, when it starts.
    vscode.workspace.onDidChangeWorkspaceFolders(() => void restart()),
    vscode.workspace.onDidGrantWorkspaceTrust(() => void restart()),
  );
  await restart();
  return { serverState, problem: () => problem };
}

export async function deactivate(): Promise<void> {
  lifecycle = lifecycle.then(stopClient, stopClient);
  await lifecycle;
}
