import { test } from 'node:test';
import assert from 'node:assert/strict';
import { linearTarget } from '../defaults/mods/new-menu/linear-target.js';
import { resolveLinearTarget } from '../defaults/mods/new-menu/linear-target.js';

test('opens the linked project in Linear', () => {
  const target = linearTarget(JSON.stringify({ workspace: 'example', project: '12345678-1234-1234-1234-123456789abc' }));
  assert.equal(target.url, 'https://linear.app/example/project/12345678-1234-1234-1234-123456789abc?noRedirect=1');
  assert.equal(target.linked, true);
});

test('falls back safely for missing, invalid or malicious project bindings', () => {
  for (const value of [null, '{', '{}', JSON.stringify({ workspace: 'example.evil/path', project: 'invalid' })]) {
    const target = linearTarget(value);
    assert.equal(target.url, 'https://linear.app/?noRedirect=1');
    assert.equal(target.linked, false);
  }
});

test('resolves a repository binding on the space machine from a nested folder', async () => {
  const calls = [];
  const binding = JSON.stringify({ workspace: 'example', project: '12345678-1234-1234-1234-123456789abc' });
  const read = async (path, opts) => { calls.push([path, opts.on]); return path === '/repo/.linear-project' ? binding : null; };
  const exec = async (_cmd, _args, opts) => { calls.push([opts.cwd, opts.on]); return { code: 0, stdout: '/repo\n' }; };
  const target = await resolveLinearTarget('/repo/src', 'remote', read, exec);
  assert.equal(target.linked, true);
  assert.deepEqual(calls, [['/repo/src/.linear-project', 'remote'], ['/repo/src', 'remote'], ['/repo/.linear-project', 'remote']]);
});

test('opens Linear even when the folder or machine is unavailable', async () => {
  const target = await resolveLinearTarget('/missing', 'remote', async () => { throw Error('offline'); }, async () => { throw Error('offline'); });
  assert.equal(target.linked, false);
  assert.equal(new URL(target.url).hostname, 'linear.app');
});