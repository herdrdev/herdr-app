import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtemp, rm } from 'node:fs/promises';
import net from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { PiIntegrationTest, assistantMessage, toolCall, text, getToolExecution, getToolExecutionDetails } from 'pi-coding-agent-test';

const here = path.dirname(fileURLToPath(import.meta.url));
test('a real Pi binds browser launches and cleanup to its own session', { timeout: 60000 }, async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'herdr-plannotator-test-'));
  try {
    const run = await new PiIntegrationTest({
      testName: 'browser-session-route',
      cwd: root,
      artifactsDir: path.resolve(here, '../.tmp/test-runs'),
      isolateUserResources: true,
      transport: 'rpc',
      extensions: [path.resolve(here, '../index.ts'), path.join(here, 'fixtures/probe.ts')],
      tools: ['browser_route_probe'],
      environment: { HC_TERMINAL_ID: '', HC_TERMINAL_SOCKET: '' },
      conversation: [assistantMessage([toolCall({ id: 'probe', name: 'browser_route_probe', arguments: {} })], { stopReason: 'toolUse' }), assistantMessage([text('Probe complete')])],
    }).run('Run the browser routing probe');
    assert.equal(getToolExecution(run, 'probe').isError, false);
    const details = getToolExecutionDetails(getToolExecution(run, 'probe'));
    assert.equal(details.configuredSession, details.session);
    assert.equal(details.session, run.state.sessionId);
    assert.equal(details.browser, path.resolve(here, process.platform === 'win32' ? '../open.cmd' : '../open.mjs'));
    assert.deepEqual(details.receipts, [{ kind: 'closed', review: 'pn-111111' }, { kind: 'closed', review: 'pn-222222' }]);
    assert.deepEqual(details.wrongOwner, { kind: 'error', code: 'owner-mismatch' });
    assert.deepEqual(details.wrongToken, { kind: 'error', code: 'owner-mismatch' });
    assert.deepEqual(details.unmatched, { kind: 'external' });
    await assert.rejects(new Promise((resolve, reject) => {
      const client = net.createConnection(details.socket);
      client.once('connect', () => { client.destroy(); resolve(); });
      client.once('error', reject);
    }));
  } finally { await rm(root, { recursive: true, force: true }); }
});
