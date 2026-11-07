import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFile as execFileCallback } from 'node:child_process';
import { mkdtemp, mkdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { githubTarget, resolveGithubTarget } from '../defaults/mods/new-menu/github-target.js';

const execFile = promisify(execFileCallback);

test('opens the GitHub repository represented by common Git remote URLs', () => {
  for (const remote of ['https://github.com/example/project.git', 'git@github.com:example/project.git', 'ssh://git@github.com/example/project.git']) {
    assert.equal(githubTarget(remote), 'https://github.com/example/project');
  }
});

test('does not treat other hosts or invalid remote paths as GitHub repositories', () => {
  for (const remote of ['', 'https://github.com.evil.test/example/project', 'https://gitlab.com/example/project', 'https://github.com/example/project/extra', 'git@github.com:../project.git', 'https://user:secret@github.com/example/project']) {
    assert.equal(githubTarget(remote), 'https://github.com/');
  }
});

test('finds the origin repository from a nested project folder', async () => {
  const root = await mkdtemp(join(tmpdir(), 'herdr-github-target-'));
  try {
    await execFile('git', ['init', '-q', root]);
    await execFile('git', ['-C', root, 'remote', 'add', 'origin', 'git@github.com:example/project.git']);
    const child = join(root, 'src');
    await mkdir(child);
    const exec = async (command, args, options) => {
      assert.equal(options.on, 'machine');
      try { const result = await execFile(command, args, { cwd: options.cwd }); return { code: 0, stdout: result.stdout }; }
      catch { return { code: 1, stdout: '' }; }
    };
    assert.equal(await resolveGithubTarget(child, 'machine', exec), 'https://github.com/example/project');
    assert.equal(await resolveGithubTarget(tmpdir(), 'machine', exec), 'https://github.com/');
  } finally { await rm(root, { recursive: true, force: true }); }
});
