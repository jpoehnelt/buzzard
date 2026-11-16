import assert from 'node:assert/strict';
import { test } from 'node:test';

import { INSTALL_COMMAND, missingBinaryMessage, resolveBinary } from '../../src/binary';
import type { ResolveEnvironment } from '../../src/binary';

function environment(files: string[], overrides: Partial<ResolveEnvironment> = {}): ResolveEnvironment {
  const present = new Set(files);
  return {
    platform: 'linux',
    env: { PATH: '/usr/local/bin:/usr/bin:/home/u/.cargo/bin' },
    homeDir: '/home/u',
    workspaceDir: '/work/project',
    isFile: (candidate) => present.has(candidate),
    ...overrides,
  };
}

test('a bare name is found in the first PATH directory that has it', () => {
  const env = environment(['/usr/bin/buzzard', '/home/u/.cargo/bin/buzzard']);
  assert.deepEqual(resolveBinary('buzzard', env), { found: true, path: '/usr/bin/buzzard' });
});

test('an empty or missing setting means the default name', () => {
  const env = environment(['/home/u/.cargo/bin/buzzard']);
  for (const setting of [undefined, '', '   ']) {
    assert.deepEqual(resolveBinary(setting, env), { found: true, path: '/home/u/.cargo/bin/buzzard' });
  }
});

test('a missing executable reports every place that was tried, in order', () => {
  const result = resolveBinary('buzzard', environment([]));
  assert.deepEqual(result, {
    found: false,
    configured: 'buzzard',
    searched: ['/usr/local/bin/buzzard', '/usr/bin/buzzard', '/home/u/.cargo/bin/buzzard'],
  });
});

test('the current directory and relative PATH entries are never searched', () => {
  const env = environment(['buzzard', './buzzard', 'bin/buzzard', '/work/project/buzzard'], {
    env: { PATH: ':.:bin:/usr/bin' },
  });
  const result = resolveBinary('buzzard', env);
  assert.deepEqual(result, { found: false, configured: 'buzzard', searched: ['/usr/bin/buzzard'] });
});

test('an absolute path is used as it is, and PATH is not consulted', () => {
  const env = environment(['/opt/tools/buzzard', '/usr/bin/buzzard']);
  assert.deepEqual(resolveBinary('/opt/tools/buzzard', env), { found: true, path: '/opt/tools/buzzard' });
  assert.deepEqual(resolveBinary('/opt/missing/buzzard', env), {
    found: false,
    configured: '/opt/missing/buzzard',
    searched: ['/opt/missing/buzzard'],
  });
});

test('a leading tilde is the home directory', () => {
  const env = environment(['/home/u/.cargo/bin/buzzard']);
  assert.deepEqual(resolveBinary('~/.cargo/bin/buzzard', env), { found: true, path: '/home/u/.cargo/bin/buzzard' });
  assert.equal(resolveBinary('~/.cargo/bin/buzzard', { ...env, homeDir: undefined }).found, false);
  // `~name` is a file name, not a home directory.
  assert.equal(resolveBinary('~other', env).found, false);
});

test('a relative path with a directory is taken from the workspace folder', () => {
  const env = environment(['/work/project/target/debug/buzzard']);
  assert.deepEqual(resolveBinary('target/debug/buzzard', env), { found: true, path: '/work/project/target/debug/buzzard' });
  assert.deepEqual(resolveBinary('./target/debug/buzzard', env), { found: true, path: '/work/project/target/debug/buzzard' });
  assert.equal(resolveBinary('target/debug/buzzard', { ...env, workspaceDir: undefined }).found, false);
});

test('a setting with a NUL character is rejected without a lookup', () => {
  let asked = 0;
  const env = environment([], {
    isFile: () => {
      asked += 1;
      return true;
    },
  });
  assert.equal(resolveBinary('buzz\0ard', env).found, false);
  assert.equal(asked, 0);
});

const windows: Partial<ResolveEnvironment> = {
  platform: 'win32',
  env: { Path: 'C:\\Windows;"C:\\Users\\u\\.cargo\\bin";relative', PathExt: '.COM;.EXE;.BAT;.CMD' },
  homeDir: 'C:\\Users\\u',
  workspaceDir: 'C:\\work',
};

test('on Windows, PATH is matched without regard to case and PATHEXT is applied', () => {
  const env = environment(['C:\\Users\\u\\.cargo\\bin\\buzzard.exe'], windows);
  assert.deepEqual(resolveBinary('buzzard', env), { found: true, path: 'C:\\Users\\u\\.cargo\\bin\\buzzard.exe' });
  assert.deepEqual(resolveBinary('buzzard.exe', env), { found: true, path: 'C:\\Users\\u\\.cargo\\bin\\buzzard.exe' });
});

test('on Windows, a batch file is never chosen because it would need a shell', () => {
  const env = environment(['C:\\Windows\\buzzard.cmd', 'C:\\Windows\\buzzard.bat'], windows);
  assert.equal(resolveBinary('buzzard', env).found, false);
  assert.equal(resolveBinary('C:\\Windows\\buzzard.cmd', env).found, false);
});

test('on Windows, absolute, home and workspace paths work with backslashes', () => {
  const env = environment(['D:\\tools\\buzzard.exe', 'C:\\Users\\u\\bin\\buzzard.exe', 'C:\\work\\target\\buzzard.exe'], windows);
  assert.deepEqual(resolveBinary('D:\\tools\\buzzard', env), { found: true, path: 'D:\\tools\\buzzard.exe' });
  assert.deepEqual(resolveBinary('~\\bin\\buzzard.exe', env), { found: true, path: 'C:\\Users\\u\\bin\\buzzard.exe' });
  assert.deepEqual(resolveBinary('target\\buzzard', env), { found: true, path: 'C:\\work\\target\\buzzard.exe' });
});

test('resolution is deterministic', () => {
  const env = environment([], windows);
  assert.deepEqual(resolveBinary('buzzard', env), resolveBinary('buzzard', env));
});

test('the message for a missing executable names the install command', () => {
  const message = missingBinaryMessage('buzzard');
  assert.ok(message.includes('not found on PATH'));
  assert.ok(message.includes(INSTALL_COMMAND));
  assert.ok(!message.includes('\n'));
});

test('the message shows a custom setting on one line, shortened', () => {
  const message = missingBinaryMessage(`/opt/x\n\tbuzzard${'y'.repeat(1000)}`);
  assert.ok(message.includes('buzzard.path'));
  assert.ok(message.includes('/opt/x  buzzard'));
  assert.ok(!/[\n\t]/.test(message));
  assert.ok(message.length < 500);
});
