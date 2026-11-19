import { test } from 'node:test';
import assert from 'node:assert/strict';
import { chmod, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { LIST_TIMEOUT_MS, listBrowsers } from '../src/host.mjs';

async function fakeBinary(t, script) {
  const folder = await mkdtemp(join(tmpdir(), 'hbr-host-test-'));
  t.after(() => rm(folder, { recursive: true, force: true }));
  const binary = join(folder, 'terminal-browser');
  await writeFile(binary, `#!/bin/sh\n${script}\n`);
  await chmod(binary, 0o755);
  return binary;
}

test('a failing terminal-browser ls reports its exit code and stderr', async t => {
  const binary = await fakeBinary(t, 'echo "daemon refused the request" >&2; exit 3');
  await assert.rejects(listBrowsers(binary), error => {
    assert.match(error.message, /ls --all --json exited with code 3/);
    assert.match(error.message, /stderr: daemon refused the request/);
    return true;
  });
});

test('a stalled terminal-browser ls reports the timeout instead of a bare failure', async t => {
  const binary = await fakeBinary(t, 'echo "still waiting" >&2; exec sleep 5');
  await assert.rejects(listBrowsers(binary, { timeout: 300 }), error => {
    assert.match(error.message, /timed out after 300 ms \(stopped with SIGTERM\)/);
    assert.match(error.message, /stderr: still waiting/);
    return true;
  });
});

test('a failure without stderr says so', async t => {
  const binary = await fakeBinary(t, 'exit 1');
  await assert.rejects(listBrowsers(binary), /exited with code 1; no stderr/);
});

test('the list waits longer than terminal-browser waits for one instance', () => {
  assert.ok(LIST_TIMEOUT_MS > 10_000);
});

test('a successful list still returns the browsers array', async t => {
  const binary = await fakeBinary(t, 'echo \'{"self":null,"browsers":[{"key":"1-1"}]}\'');
  assert.deepEqual(await listBrowsers(binary), [{ key: '1-1' }]);
});
