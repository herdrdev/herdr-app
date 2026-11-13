import { spawn, execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdir, mkdtemp, writeFile, readFile, open } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import http from 'node:http';
import assert from 'node:assert/strict';
import { HerdrApp } from '../src/herdr.mjs';
import { BrowserHost, listBrowsers } from '../src/host.mjs';
import { BrowserBridge } from '../src/server.mjs';
import { waitUntil } from '../src/events.mjs';
import { installMod } from '../src/install.mjs';

const execute = promisify(execFile);
const desktop = process.env.HERDR_BROWSER_DESKTOP_BIN;
const terminal = process.env.TERMINAL_BROWSER_BIN;
if (!desktop || !terminal || !process.env.HERDR_BROWSER_TERMINAL_DIST) throw new Error('Set HERDR_BROWSER_DESKTOP_BIN, TERMINAL_BROWSER_BIN and HERDR_BROWSER_TERMINAL_DIST for the isolated desktop smoke test');
const nativeFile = await open(desktop, 'r');
const magic = Buffer.alloc(4);
await nativeFile.read(magic, 0, 4, 0);
await nativeFile.close();
if (!['cffaedfe', 'feedfacf', 'cafebabe', 'bebafeca', '7f454c46'].includes(magic.toString('hex'))) throw new Error('HERDR_BROWSER_DESKTOP_BIN must be the native Herdr App executable, not a shell launcher');
const packageRoot = fileURLToPath(new URL('../', import.meta.url));
const root = await mkdtemp('/tmp/hbr-');
const artifacts = resolve(packageRoot, '.artifacts');
await mkdir(artifacts, { recursive: true });
const env = { ...process.env, HERDR_APP_HOME: join(root, 'home'), HC_SESSION: `hb${root.slice(-6)}`, XDG_STATE_HOME: join(root, 'state'), XDG_DATA_HOME: join(root, 'data'), XDG_CONFIG_HOME: join(root, 'config'), XDG_CACHE_HOME: join(root, 'cache'), XDG_RUNTIME_DIR: join(root, 'run'), HERDR_APP_NO_SOUND: '1', DO_NOT_TRACK: '1', TERMINAL_BROWSER_NO_TELEMETRY: '1' };
for (const key of ['HC_SOCKET', 'HC_TERMINAL_ID', 'HC_TERMINAL_SOCKET', 'HC_SCOPE']) delete env[key];
for (const dir of ['home', 'state/terminal-browser', 'data', 'config', 'cache', 'run']) await mkdir(join(root, dir), { recursive: true, mode: 0o700 });
const version = (await readFile(join(process.env.HERDR_BROWSER_TERMINAL_DIST, 'VERSION'), 'utf8')).trim();
await writeFile(join(root, 'state/terminal-browser/setup-version'), `${version} ${resolve(process.env.HERDR_BROWSER_TERMINAL_DIST)}\n`);
await writeFile(join(root, 'home/settings.json'), JSON.stringify({ telemetry: false, sounds: { enabled: false }, appearance: 'dark' }));
await writeFile(join(root, 'home/shared.json'), JSON.stringify({ 'frame.welcomed': 'test' }));
const log = await open(join(root, 'desktop.log'), 'w');
const processHandle = spawn(desktop, [], { env, stdio: ['ignore', log.fd, log.fd] });
const app = new HerdrApp(env.HERDR_APP_HOME);
const fixture = http.createServer((_request, response) => {
  response.setHeader('Content-Type', 'text/html');
  response.end(`<!doctype html><title>Herdr Chromium smoke test</title><main style="font:18px system-ui;padding:40px"><h1>Chromium in Herdr</h1><p>This page is controlled through Playwright.</p><label>Email <input aria-label="Email"></label><button onclick="document.getElementById('status').textContent='Saved through Playwright'">Save</button><p id="status">Ready</p></main>`);
});
await new Promise(resolve => fixture.listen(0, '127.0.0.1', resolve));
let bridge;
const oldEnv = { ...process.env };
try {
  await waitUntil(async () => { try { return await app.world(); } catch { return null; } }, 30_000);
  await waitUntil(async () => {
    try { return await app.run('space.new', { name: 'Browser test', folder: packageRoot, argv: ['/bin/zsh'] }); }
    catch (error) { if (error.message.includes('layout is unavailable')) return null; throw error; }
  }, 30_000);
  await app.run('overlay.close', {});
  const world = await app.world();
  const space = world.spaces.find(s => s.name === 'Browser test');
  const installed = await installMod(env.HERDR_APP_HOME);
  await waitUntil(async () => {
    try { return await app.run('mod.open', { name: installed.name }); }
    catch (error) { if (error.message.includes('no mod folder')) return null; throw error; }
  }, 5000);
  await new Promise(resolve => setTimeout(resolve, 500));
  await app.call({ shot: join(artifacts, 'launcher.png') });
  await app.run('overlay.close', {});
  const url = `http://127.0.0.1:${fixture.address().port}`;
  const launched = await execute(process.execPath, [join(packageRoot, 'src/cli.mjs'), 'open', '--space', space.id, '--url', url, '--binary', terminal, '--herdr-home', env.HERDR_APP_HOME, '--focus'], { env });
  const created = JSON.parse(launched.stdout);
  Object.assign(process.env, env);
  const browsers = await waitUntil(async () => { const browsers = await listBrowsers(terminal); return browsers.length && browsers[0].tabs?.[0]?.targetId ? browsers : null; }, 30_000);
  const host = await BrowserHost.connect({ browserKey: browsers[0].key, binary: terminal });
  bridge = new BrowserBridge(host, { outputDir: artifacts });
  await host.current().page.waitForLoadState('domcontentloaded');
  const snapshot = await bridge.call('browser_snapshot');
  const tree = snapshot.content.filter(c => c.type === 'text').map(c => c.text).join('\n');
  assert.match(tree, /heading "Chromium in Herdr"/);
  const email = tree.match(/textbox "Email".*\[ref=(e\d+)\]/)?.[1];
  const save = tree.match(/button "Save".*\[ref=(e\d+)\]/)?.[1];
  const typed = await bridge.call('browser_type', { target: email, text: 'browser@herdr.test' });
  assert.ok(!typed.isError, JSON.stringify(typed));
  const clicked = await bridge.call('browser_click', { target: save });
  assert.ok(!clicked.isError, JSON.stringify(clicked));
  assert.equal(await host.current().page.locator('#status').textContent(), 'Saved through Playwright');
  await bridge.call('browser_screenshot_region', { x: 0, y: 0, width: 400, height: 240 });
  const firstTarget = host.selected;
  const listing = await host.tabs({ action: 'new', url });
  assert.equal(listing.tabs.length, 2);
  await host.tabs({ action: 'select', index: listing.tabs.find(tab => tab.target === firstTarget).index });
  const status = await app.call({ status: true });
  await new Promise(resolve => setTimeout(resolve, 1000));
  await app.call({ shot: join(artifacts, 'desktop.png') });
  await writeFile(join(artifacts, 'desktop-smoke.json'), JSON.stringify({ root, created, browser: browsers[0].key, tabs: listing, status, snapshot: tree }, null, 2));
  await host.tabs({ action: 'close', index: 1 });
  const last = await host.tabs({ action: 'close' });
  assert.ok(last.tabs.length <= 1, 'closing the last tab either ends the instance or replaces it with a start page');
  console.log(JSON.stringify({ passed: true, root, created, artifacts, browser: browsers[0].key, tabs: listing.tabs.length }));
} finally {
  if (bridge) await bridge.close();
  for (const key of Object.keys(process.env)) if (!(key in oldEnv)) delete process.env[key];
  Object.assign(process.env, oldEnv);
  await execute(terminal, ['shutdown'], { env }).catch(() => {});
  await app.run('app.quit', {}).catch(() => {});
  if (processHandle.exitCode === null) processHandle.kill('SIGTERM');
  await execute('hc', ['server', 'stop'], { env }).catch(() => {});
  await new Promise(resolve => fixture.close(resolve));
  await log.close();
}
