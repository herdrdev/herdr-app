import { test } from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import { mkdtemp, readFile, writeFile, cp, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';

test('fresh copied skill and configured stdio process attach to the requested existing page', { timeout: 60_000 }, async t => {
  const root = await mkdtemp(join(tmpdir(), 'herdr-stdio-'));
  let context, client;
  const fixture = http.createServer((_request, response) => {
    response.setHeader('Content-Type', 'text/html');
    response.end('<title>Agent setup fixture</title><main><h1>Configured browser</h1><label>Value<input aria-label="Value"></label></main>');
  });
  t.after(async () => {
    await client?.close();
    await context?.close();
    fixture.closeAllConnections();
    await new Promise(resolve => fixture.close(resolve));
    await rm(root, { recursive: true, force: true });
  });
  await new Promise(resolve => fixture.listen(0, '127.0.0.1', resolve));
  const url = `http://127.0.0.1:${fixture.address().port}`;
  const profile = join(root, 'profile');
  context = await chromium.launchPersistentContext(profile, {
    channel: process.env.HERDR_BROWSER_TEST_CHANNEL ?? 'chrome', headless: true,
    executablePath: process.env.HERDR_BROWSER_TEST_EXECUTABLE,
    args: ['--remote-debugging-port=0'],
  });
  const wanted = context.pages()[0];
  await wanted.goto(url);
  const other = await context.newPage();
  await other.goto(url);
  await other.getByLabel('Value').fill('Other page');
  const targetSession = await context.newCDPSession(wanted);
  const target = (await targetSession.send('Target.getTargetInfo')).targetInfo.targetId;
  await targetSession.detach();
  const port = Number((await readFile(join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]);
  const skill = join(root, 'skill');
  await cp(fileURLToPath(new URL('../skill', import.meta.url)), skill, { recursive: true });
  const configPath = join(root, 'mcp.json');
  await writeFile(configPath, JSON.stringify({ mcpServers: { herdr_browser: {
    command: join(skill, 'scripts', 'herdr-browser'),
    args: ['mcp', '--endpoint', `http://127.0.0.1:${port}`, '--target', target, '--lean', '--output-dir', join(root, 'artifacts')],
    env: { HERDR_BROWSER_CLI: fileURLToPath(new URL('../src/cli.mjs', import.meta.url)) },
  } } }));
  const config = JSON.parse(await readFile(configPath, 'utf8')).mcpServers.herdr_browser;
  const transport = new StdioClientTransport({ ...config, cwd: root, stderr: 'pipe' });
  client = new Client({ name: 'fresh-agent-setup', version: '1' });
  await client.connect(transport);
  const tools = (await client.listTools()).tools;
  assert.ok(tools.some(tool => tool.name === 'browser_events'));
  assert.ok(tools.some(tool => tool.name === 'browser_observe_dom'));
  assert.ok(!tools.some(tool => tool.name.startsWith('browser_run_code')));
  const call = async (name, args = {}) => {
    const result = await client.callTool({ name, arguments: args });
    assert.ok(!result.isError, JSON.stringify(result));
    return result;
  };
  const tabs = (await call('browser_tabs', { action: 'list' })).structuredContent;
  assert.equal(tabs.browser, 'cdp');
  assert.equal(tabs.tabs.length, 2);
  assert.equal(tabs.tabs.find(tab => tab.selected).target, target);
  const snapshot = (await call('browser_snapshot')).content.filter(item => item.type === 'text').map(item => item.text).join('\n');
  assert.match(snapshot, /heading "Configured browser"/);
  const field = snapshot.match(/textbox "Value".*\[ref=([^\]]+)\]/)?.[1];
  assert.ok(field);
  await call('browser_type', { target: field, text: 'Configured agent' });
  assert.equal(await wanted.getByLabel('Value').inputValue(), 'Configured agent');
  assert.equal(await other.getByLabel('Value').inputValue(), 'Other page');
  const denied = await client.callTool({ name: 'browser_run_code_unsafe', arguments: {} });
  assert.equal(denied.isError, true);
  assert.match(denied.content[0].text, /not exposed by this companion/);
  await client.close();
  assert.equal(await wanted.title(), 'Agent setup fixture');
  assert.equal(context.pages().length, 2, 'detaching preserves the existing browser pages');
});
