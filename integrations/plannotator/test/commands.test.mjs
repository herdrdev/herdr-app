import assert from 'node:assert/strict';
import test from 'node:test';
import { chmod, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { PiIntegrationTest, assistantMessage, toolCall, text, getToolExecution, getToolExecutionDetails } from 'pi-coding-agent-test';

const here = path.dirname(fileURLToPath(import.meta.url));
test('Herdr runs Plannotator commands in the Pi its terminal label names', { timeout: 60000, skip: process.platform === 'win32' }, async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'herdr-plannotator-commands-'));
  try {
    await chmod(path.join(here, 'fixtures/hc'), 0o755);
    const run = await new PiIntegrationTest({
      testName: 'plannotator-commands',
      cwd: root,
      artifactsDir: path.resolve(here, '../.tmp/test-runs'),
      isolateUserResources: true,
      transport: 'rpc',
      extensions: [path.resolve(here, '../index.ts'), path.join(here, 'fixtures/commands.ts')],
      tools: ['annotation_request_probe'],
      environment: {
        PATH: `${path.join(here, 'fixtures')}${path.delimiter}${process.env.PATH}`,
        HC_TERMINAL_ID: 't1', HC_TERMINAL_SOCKET: path.join(root, 'hc.sock'), FAKE_HC_STATE: path.join(root, 'hc.json'),
        PLANNOTATOR_BROWSER: undefined, PLANNOTATOR_REMOTE: undefined,
        HERDR_PLANNOTATOR_SESSION_ID: undefined, HERDR_PLANNOTATOR_NODE: undefined,
        HERDR_PLANNOTATOR_SOCKET: undefined, HERDR_PLANNOTATOR_TOKEN: undefined,
      },
      conversation: [assistantMessage([toolCall({ id: 'probe', name: 'annotation_request_probe', arguments: {} })], { stopReason: 'toolUse' }), assistantMessage([text('Probe complete')])],
    }).run('Run the annotation request probe');
    assert.equal(getToolExecution(run, 'probe').isError, false);
    const details = getToolExecutionDetails(getToolExecution(run, 'probe'));
    assert.equal(details.label.session, run.state.sessionId);
    assert.equal(details.label.token, 'string');
    assert.equal(details.label.request, path.resolve(here, '../request.mjs'));
    assert.equal(details.label.hc, path.join(here, 'fixtures/hc'));
    assert.deepEqual(details.runs.map(r => [r.command, r.code]), [['review', 0], ['last', 0], ['other', 1]]);
    assert.match(details.runs[2].stderr, /Usage/);
    assert.deepEqual(details.ran, ['plannotator-review', 'plannotator-last']);
    assert.deepEqual(details.wrongToken, { kind: 'error', code: 'owner-mismatch' });
  } finally { await rm(root, { recursive: true, force: true }); }
});
