import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { chmod, mkdir, mkdtemp, readlink, realpath, rm, symlink, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';

const cli = fileURLToPath(new URL('../src/cli.mjs', import.meta.url));

test('CLI starts through executable and parent-directory symlinks', async t => {
  const root = await mkdtemp(join(tmpdir(), 'herdr-cli-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const executable = join(root, 'herdr-browser');
  const directory = join(root, 'current');
  await symlink(cli, executable);
  await symlink(fileURLToPath(new URL('../', import.meta.url)), directory, 'dir');
  for (const path of [cli, executable, join(directory, 'src/cli.mjs')]) {
    const stdout = execFileSync(process.execPath, [path, '--help'], { encoding: 'utf8' });
    assert.match(stdout, /^herdr-browser:/, path);
  }
});

test('importing the CLI from stdin does not start its command handler', () => {
  const stdout = execFileSync(process.execPath, ['--input-type=module', '-'], {
    encoding: 'utf8',
    input: `await import(${JSON.stringify(new URL('../src/cli.mjs', import.meta.url).href)}); process.stdout.write('imported');`,
  });
  assert.equal(stdout, 'imported');
});

test('setup exposes terminal-browser in a user bin directory and is idempotent', async t => {
  const root = await mkdtemp(join(tmpdir(), 'herdr-browser-setup-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const runtime = join(root, 'runtime', 'terminal-browser');
  const bin = join(root, 'bin');
  await mkdir(join(root, 'runtime'), { recursive: true });
  await writeFile(runtime, '#!/bin/sh\n');
  await chmod(runtime, 0o755);
  const args = [cli, 'setup', '--binary', runtime, '--bin-dir', bin];
  const first = JSON.parse(execFileSync(process.execPath, args, { encoding: 'utf8', env: { ...process.env, PATH: `${bin}:${process.env.PATH ?? ''}` } }));
  const second = JSON.parse(execFileSync(process.execPath, args, { encoding: 'utf8', env: { ...process.env, PATH: `${bin}:${process.env.PATH ?? ''}` } }));
  assert.equal(first.status, 'created');
  assert.equal(second.status, 'unchanged');
  assert.equal(first.onPath, true);
  assert.equal(await readlink(join(bin, 'terminal-browser')), await realpath(runtime));
});

test('setup does not replace another terminal-browser command', async t => {
  const root = await mkdtemp(join(tmpdir(), 'herdr-browser-conflict-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const runtime = join(root, 'terminal-browser-runtime');
  const bin = join(root, 'bin');
  await mkdir(bin, { recursive: true });
  await writeFile(runtime, '#!/bin/sh\n');
  await chmod(runtime, 0o755);
  await writeFile(join(bin, 'terminal-browser'), 'existing\n');
  assert.throws(
    () => execFileSync(process.execPath, [cli, 'setup', '--binary', runtime, '--bin-dir', bin], { encoding: 'utf8', stdio: 'pipe' }),
    error => error.stderr.includes('already exists and is not a symbolic link'),
  );
});
