import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtemp, symlink, rm } from 'node:fs/promises';
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
