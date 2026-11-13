import { test } from 'node:test';
import assert from 'node:assert/strict';
import net from 'node:net';
import { mkdtemp, chmod, rm, mkdir, symlink, stat } from 'node:fs/promises';
import { join } from 'node:path';
import { request, serveLine } from '../src/rpc.mjs';
import { startDaemon } from '../src/daemon.mjs';
import { BrowserBridge } from '../src/server.mjs';

test('RPC rejects malformed or oversized input and remains usable', async t => {
  const root = await mkdtemp('/tmp/hb-rpc-');
  const path = join(root, 'rpc.sock');
  const server = net.createServer(socket => serveLine(socket, async input => input.value));
  await new Promise(resolve => server.listen(path, resolve));
  t.after(async () => { await new Promise(resolve => server.close(resolve)); await rm(root, { recursive: true, force: true }); });
  const exchange = payload => new Promise((resolve, reject) => {
    const socket = net.connect(path); let response = '';
    socket.setEncoding('utf8');
    socket.on('connect', () => socket.end(payload));
    socket.on('data', text => { response += text; });
    socket.on('error', error => { if (error.code !== 'EPIPE' && error.code !== 'ECONNRESET') reject(error); });
    socket.on('close', () => resolve(response));
  });
  assert.ok(JSON.parse(await exchange('{broken}\n')).error);
  assert.equal(await exchange('x'.repeat(1024 * 1024 + 1)), '');
  assert.equal((await request(path, { id: 'ok', value: 42 })).result, 42);
});

test('daemon refuses a shared socket directory before opening a connection', async t => {
  const root = await mkdtemp('/tmp/hb-dir-');
  t.after(() => rm(root, { recursive: true, force: true }));
  await chmod(root, 0o755);
  await assert.rejects(startDaemon(null, join(root, 'service.sock')), /private directory/);
});

test('artifact directory is private and a symlink cannot change another directory permissions', async t => {
  const root = await mkdtemp('/tmp/hb-output-');
  t.after(() => rm(root, { recursive: true, force: true }));
  const shared = join(root, 'shared');
  await mkdir(shared, { mode: 0o755 });
  await symlink(shared, join(root, 'link'));
  await assert.rejects(new BrowserBridge(null, { outputDir: join(root, 'link') }).prepareOutput(), /directory owned/);
  assert.equal((await stat(shared)).mode & 0o777, 0o755);
  await new BrowserBridge(null, { outputDir: shared }).prepareOutput();
  assert.equal((await stat(shared)).mode & 0o777, 0o700);
});
