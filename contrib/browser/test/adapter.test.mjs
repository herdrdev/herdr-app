import { test } from 'node:test';
import assert from 'node:assert/strict';
import { terminalAdapter } from '../src/herdr.mjs';
import { request } from '../src/rpc.mjs';

test('terminal socket routes splits by identity and refuses another tab', async t => {
  const commands = [];
  const app = {
    world: async () => ({ spaces: [{ id: 's', machine: null, tabs: [
      { id: 'tab-1', terminals: [{ terminal: 't1', pane: 'p1' }, { terminal: 't2', pane: 'p2' }] },
      { id: 'tab-2', terminals: [{ terminal: 't3', pane: 'p3' }] },
    ] }] }),
    run: async (command, args) => { commands.push({ command, args }); return { pane: 'p4' }; },
  };
  const adapter = await terminalAdapter(app, 't1');
  t.after(() => adapter.close());
  const call = (method, params) => request(adapter.socketPath, { id: 'request-1', method, params });
  assert.deepEqual((await call('getCurrentPane', {})).result, { pane: { id: 'p1', tab: 'tab-1' } });
  const capabilities = (await call('hello', { protocol: 1 })).result.methods;
  assert.ok(!capabilities.includes('sendText'));
  assert.equal((await call('listPanes', {})).result.panes.length, 2);
  assert.match((await call('focusPane', { pane: 'p3' })).error.message, /outside/);
  const argv = ['node', 'a path with spaces', '$(no-shell)'];
  await call('split', { from: { id: 'p2' }, direction: 'left', command: argv });
  assert.deepEqual(commands[0], { command: 'pane.split', args: { pane: 'p2', direction: 'right', argv } });
  assert.deepEqual(commands[1], { command: 'pane.swap', args: { pane: 'p4', target: 'p2' } });
});
