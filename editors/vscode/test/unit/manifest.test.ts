// The manifest is where the trademark rule, the pinned versions and the
// contributed commands live, so it is checked like code.

import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { test } from 'node:test';

// Tests run from the extension directory (`npm test`).
const root = process.cwd();

interface Manifest {
  name: string;
  publisher: string;
  license: string;
  main: string;
  activationEvents: string[];
  dependencies: Record<string, string>;
  devDependencies: Record<string, string>;
  capabilities: { untrustedWorkspaces: { restrictedConfigurations: string[] } };
  contributes: {
    configuration: { properties: Record<string, { default: unknown }> };
    commands: { command: string; title: string; category: string }[];
    menus: Record<string, { command: string; when: string }[]>;
    customEditors: { viewType: string; priority: string; selector: { filenamePattern: string }[] }[];
  };
}

const manifest = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8')) as Manifest;

test('identifiers stay in the buzzard namespace', () => {
  assert.equal(manifest.name, 'buzzard');
  assert.equal(manifest.publisher, 'buzzard');
  const identifiers = [
    ...manifest.contributes.commands.map((entry) => entry.command),
    ...Object.keys(manifest.contributes.configuration.properties),
    ...manifest.contributes.customEditors.map((entry) => entry.viewType),
  ];
  for (const identifier of identifiers) {
    assert.match(identifier, /^buzzard\.[A-Za-z.]+$/);
    assert.ok(!/labview|(^|[^a-z])ni([^a-z]|$)/i.test(identifier), identifier);
  }
});

test('it activates on workspaces with the five file types', () => {
  assert.deepEqual(manifest.activationEvents, [
    'workspaceContains:**/*.vi',
    'workspaceContains:**/*.ctl',
    'workspaceContains:**/*.lvproj',
    'workspaceContains:**/*.lvlib',
    'workspaceContains:**/*.lvclass',
  ]);
});

test('the four commands are contributed under the Buzzard category', () => {
  assert.deepEqual(
    manifest.contributes.commands.map((entry) => `${entry.category}: ${entry.title}`),
    ['Buzzard: Show File Summary', 'Buzzard: Show Block Diagram', 'Buzzard: Find Callers', 'Buzzard: Restart Server'],
  );
});

test('every contributed command is registered by the extension, and the other way round', () => {
  const source = fs.readFileSync(path.join(root, 'src', 'extension.ts'), 'utf8');
  const registered = [...source.matchAll(/registerCommand\('([^']+)'/g)].map((match) => match[1]).sort();
  assert.deepEqual(registered, manifest.contributes.commands.map((entry) => entry.command).sort());
});

test('explorer menu entries apply to the right extensions only', () => {
  const entries = manifest.contributes.menus['explorer/context'] ?? [];
  assert.deepEqual(
    entries.map((entry) => entry.command),
    ['buzzard.showSummary', 'buzzard.showDiagram', 'buzzard.findCallers'],
  );
  for (const entry of entries) {
    const pattern = /resourceExtname =~ \/(.+)\/i/.exec(entry.when)?.[1];
    assert.ok(pattern !== undefined, entry.when);
    const matches = (extension: string): boolean => new RegExp(pattern, 'i').test(extension);
    assert.ok(matches('.vi') && matches('.VI') && matches('.ctl'));
    assert.ok(!matches('.vit') && !matches('.txt') && !matches('.vi.bak'));
    assert.equal(matches('.lvproj'), entry.command !== 'buzzard.showDiagram');
    assert.ok(entry.when.includes('resourceScheme == file'));
  }
});

test('the viewer is optional, so it never replaces the default editor', () => {
  assert.deepEqual(manifest.contributes.customEditors, [
    {
      viewType: 'buzzard.viewer',
      displayName: 'Buzzard Viewer',
      selector: [{ filenamePattern: '*.vi' }, { filenamePattern: '*.ctl' }],
      priority: 'option',
    },
  ]);
});

test('settings: the path defaults to the bare name and a workspace cannot set it without trust', () => {
  const properties = manifest.contributes.configuration.properties;
  assert.equal(properties['buzzard.path']?.default, 'buzzard');
  assert.equal(properties['buzzard.trace.server']?.default, 'off');
  assert.deepEqual(manifest.capabilities.untrustedWorkspaces.restrictedConfigurations, ['buzzard.path']);
});

test('dependency versions are pinned exactly', () => {
  for (const [name, version] of Object.entries({ ...manifest.dependencies, ...manifest.devDependencies })) {
    assert.match(version, /^\d+\.\d+\.\d+$/, name);
  }
});

test('the licence is Apache-2.0 and the file matches the one of the repository', () => {
  assert.equal(manifest.license, 'Apache-2.0');
  const own = fs.readFileSync(path.join(root, 'LICENSE'), 'utf8');
  const repository = fs.readFileSync(path.join(root, '..', '..', 'LICENSE'), 'utf8');
  assert.equal(own, repository);
});

test('no telemetry dependency and no network access in the sources', () => {
  const names = Object.keys({ ...manifest.dependencies, ...manifest.devDependencies });
  assert.ok(!names.some((name) => /telemetry|analytics|applicationinsights/i.test(name)));
  for (const file of fs.readdirSync(path.join(root, 'src')).sort()) {
    const source = fs.readFileSync(path.join(root, 'src', file), 'utf8');
    assert.ok(!/\bfetch\(|node:https?|node:net|XMLHttpRequest|TelemetryReporter/.test(source), file);
  }
});
