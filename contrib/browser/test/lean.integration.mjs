import { test } from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import { mkdtemp, readFile, rm, readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { chromium } from 'playwright';
import { BrowserHost } from '../src/host.mjs';
import { BrowserBridge } from '../src/server.mjs';

test('lean responses preserve tools, explicit scoped snapshots, errors and independent page state', { timeout: 60_000 }, async t => {
  const root = await mkdtemp('/tmp/hb-lean-');
  let context, bridge;
  const fixture = http.createServer((_request, response) => {
    response.setHeader('Content-Type', 'text/html');
    response.end('<title>Lean fixture</title><main><h1>Lean test</h1><label>Value<input aria-label="Value"></label><button onclick="document.querySelector(\'#status\').textContent=\'Saved\'">Save</button><p id="status">Ready</p></main>');
  });
  t.after(async () => {
    await bridge?.close();
    await context?.close();
    fixture.closeAllConnections();
    await new Promise(resolve => fixture.close(resolve));
    await rm(root, { recursive: true, force: true });
  });
  await new Promise(resolve => fixture.listen(0, '127.0.0.1', resolve));
  const url = `http://127.0.0.1:${fixture.address().port}`;
  context = await chromium.launchPersistentContext(join(root, 'profile'), {
    channel: process.env.HERDR_BROWSER_TEST_CHANNEL ?? 'chrome', headless: true,
    executablePath: process.env.HERDR_BROWSER_TEST_EXECUTABLE,
    args: ['--remote-debugging-port=0'],
  });
  const port = Number((await readFile(join(root, 'profile', 'DevToolsActivePort'), 'utf8')).split('\n')[0]);
  const endpoint = `http://127.0.0.1:${port}`;
  const text = result => result.content.filter(c => c.type === 'text').map(c => c.text).join('\n');
  bridge = new BrowserBridge(await BrowserHost.connect({ endpoint }), { outputDir: join(root, 'full') });
  const defaultTools = await bridge.tools();
  assert.ok(!((await bridge.call('browser_navigate', { url })).isError));
  const normalResponse = text(await bridge.call('browser_type', { target: 'input', text: 'Normal' }));
  await bridge.close();
  const outputDir = join(root, 'lean');
  const host = await BrowserHost.connect({ endpoint });
  bridge = new BrowserBridge(host, { outputDir, lean: true });
  assert.deepEqual(await bridge.tools(), defaultTools);
  const leanResponse = text(await bridge.call('browser_type', { target: 'input', text: 'Lean' }));
  assert.ok(leanResponse.length < normalResponse.length);
  assert.doesNotMatch(leanResponse, /### Snapshot|### Ran Playwright code/);
  assert.equal(await host.current().page.getByLabel('Value').inputValue(), 'Lean');
  assert.equal((await readdir(outputDir)).filter(file => file.endsWith('.yml')).length, 0);
  const snapshot = text(await bridge.call('browser_snapshot', { target: 'main' }));
  assert.match(snapshot, /heading "Lean test"/);
  const save = snapshot.match(/button "Save".*\[ref=([^\]]+)\]/)?.[1];
  assert.ok(save);
  const watch = await bridge.call('browser_observe_dom', { selector: '#status', debounceMs: 50 });
  const cursor = (await bridge.call('browser_events')).structuredContent.cursor;
  assert.ok(!(await bridge.call('browser_click', { target: save })).isError);
  const events = (await bridge.call('browser_events', { cursor, types: ['dom.changed'], waitMs: 1000 })).structuredContent;
  assert.equal(events.events[0].data.watch, watch.structuredContent.watch);
  const updated = text(await bridge.call('browser_snapshot', { target: '#status' }));
  assert.match(updated, /Saved/);
  assert.equal((await bridge.call('browser_click', { target: 'e999999' })).isError, true);
  const initialTarget = host.selected;
  for (const value of ['Second', 'Third']) {
    assert.ok(!(await bridge.call('browser_tabs', { action: 'new', url })).isError);
    await bridge.call('browser_type', { target: 'input', text: value });
  }
  await bridge.call('browser_tabs', { action: 'close', index: 1 });
  await bridge.call('browser_tabs', { action: 'select', index: 1 });
  assert.equal(await host.current().page.getByLabel('Value').inputValue(), 'Third');
  await bridge.call('browser_tabs', { action: 'select', index: 0 });
  assert.equal(host.selected, initialTarget);
  assert.equal(await host.current().page.getByLabel('Value').inputValue(), 'Lean');
});
