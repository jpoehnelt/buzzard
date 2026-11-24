import assert from 'node:assert/strict';
import { test } from 'node:test';

import { baseName, fileKind, hasDiagram, hoverMarkdown, isSupported, locationUris, relativeLabel } from '../../src/files';

test('baseName handles both separators and trailing ones', () => {
  assert.equal(baseName('/a/b/Main.vi'), 'Main.vi');
  assert.equal(baseName('C:\\a\\b\\Main.vi'), 'Main.vi');
  assert.equal(baseName('/a/b/dir/'), 'dir');
  assert.equal(baseName('Main.vi'), 'Main.vi');
  assert.equal(baseName(''), '');
});

test('fileKind goes by the extension, in any letter case', () => {
  assert.equal(fileKind('/a/Main.vi'), 'vi');
  assert.equal(fileKind('/a/Main.VI'), 'vi');
  assert.equal(fileKind('C:\\a\\Cluster.Ctl'), 'ctl');
  assert.equal(fileKind('App.lvproj'), 'lvproj');
  assert.equal(fileKind('Lib.lvlib'), 'lvlib');
  assert.equal(fileKind('Motor.lvclass'), 'lvclass');
});

test('fileKind rejects everything else', () => {
  for (const name of ['', 'vi', '.vi', 'Main.vit', 'Main.vi.bak', 'Main', 'Main.', '/a.vi/readme', 'a.vi/']) {
    assert.equal(fileKind(name), name === 'a.vi/' ? 'vi' : undefined, name);
  }
  assert.equal(isSupported('notes.txt'), false);
  assert.equal(isSupported('weird <name> & "quotes".vi'), true);
});

test('only VIs and controls have a diagram', () => {
  assert.equal(hasDiagram('a.vi'), true);
  assert.equal(hasDiagram('a.ctl'), true);
  assert.equal(hasDiagram('a.lvproj'), false);
  assert.equal(hasDiagram('a.lvclass'), false);
  assert.equal(hasDiagram('a.txt'), false);
});

test('relativeLabel is relative to the deepest workspace folder that holds the file', () => {
  assert.equal(relativeLabel('/w/app/src/Main.vi', ['/w/app'], 'linux'), 'src/Main.vi');
  assert.equal(relativeLabel('/w/app/src/Main.vi', ['/w', '/w/app/src', '/w/app'], 'linux'), 'Main.vi');
  assert.equal(relativeLabel('/elsewhere/Main.vi', ['/w/app'], 'linux'), '/elsewhere/Main.vi');
  assert.equal(relativeLabel('/w/application/Main.vi', ['/w/app'], 'linux'), '/w/application/Main.vi');
  assert.equal(relativeLabel('/w/app/Main.vi', [], 'linux'), '/w/app/Main.vi');
  assert.equal(relativeLabel('C:\\w\\app\\src\\Main.vi', ['c:\\w\\app'], 'win32'), 'src/Main.vi');
  assert.equal(relativeLabel('D:\\other\\Main.vi', ['C:\\w\\app'], 'win32'), 'D:/other/Main.vi');
});

test('locationUris keeps file URIs, sorted and without duplicates', () => {
  const answer = [
    { uri: 'file:///w/b.vi', range: {} },
    { uri: 'file:///w/a.vi', range: {} },
    { uri: 'file:///w/b.vi', range: {} },
  ];
  assert.deepEqual(locationUris(answer), ['file:///w/a.vi', 'file:///w/b.vi']);
});

test('locationUris survives malformed answers', () => {
  for (const answer of [null, undefined, 3, 'file:///x', {}, { uri: 'file:///x' }]) {
    assert.deepEqual(locationUris(answer), []);
  }
  const mixed = [null, 1, 'x', [], {}, { uri: 5 }, { uri: 'https://example.com/a.vi' }, { uri: 'javascript:alert(1)' }, { uri: 'file:///ok.vi' }];
  assert.deepEqual(locationUris(mixed), ['file:///ok.vi']);
});

test('hoverMarkdown reads every shape the protocol allows', () => {
  assert.equal(hoverMarkdown({ contents: { kind: 'markdown', value: '### a' } }), '### a');
  assert.equal(hoverMarkdown({ contents: 'plain' }), 'plain');
  assert.equal(hoverMarkdown({ contents: ['one', { language: 'x', value: 'two' }, '  '] }), 'one\n\ntwo');
  for (const answer of [null, undefined, 1, {}, { contents: null }, { contents: {} }, { contents: [] }, { contents: '  ' }]) {
    assert.equal(hoverMarkdown(answer), undefined);
  }
});
