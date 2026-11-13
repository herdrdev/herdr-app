import { promisify } from 'node:util';
import { execFile } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import net from 'node:net';
import { mkdtemp, chmod, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { chromium } from 'playwright';
import { request, serveLine } from './rpc.mjs';
import { EventLog, observePage, waitUntil } from './events.mjs';

const execute = promisify(execFile);

export async function listBrowsers(binary = process.env.TERMINAL_BROWSER_BIN ?? 'terminal-browser') {
  const folder = await mkdtemp(join(tmpdir(), 'hbr-list-'));
  const socketPath = join(folder, 'terminal.sock');
  const server = net.createServer(socket => serveLine(socket, async ({ method }) => {
    if (method !== 'hello') throw new Error('Discovery does not control panes');
    return { name: 'herdr-app', methods: [] };
  }));
  try {
    await new Promise((resolve, reject) => { server.once('error', reject); server.listen(socketPath, resolve); });
    await chmod(socketPath, 0o600);
    const { stdout } = await execute(binary, ['ls', '--all', '--json'], {
      timeout: 10_000, maxBuffer: 4 * 1024 * 1024,
      env: { ...process.env, TERMINAL_BROWSER_TERMINAL_SOCKET: socketPath },
    });
    const result = JSON.parse(stdout);
    if (!Array.isArray(result.browsers)) throw new Error('terminal-browser ls returned no browsers array');
    return result.browsers;
  } finally {
    if (server.listening) await new Promise(resolve => server.close(resolve));
    await rm(folder, { recursive: true, force: true });
  }
}

export function chooseBrowser(browsers, key) {
  const matches = key ? browsers.filter(b => b.key === key) : browsers.filter(b => b.inCurrentTab);
  if (matches.length !== 1) throw new Error('Choose one browser explicitly with --browser <key> from: herdr-browser list');
  const browser = matches[0];
  if (!Number.isInteger(browser.cdpPort) || browser.cdpPort < 1 || browser.cdpPort > 65535) throw new Error('Browser has no valid CDP port');
  return browser;
}

export class BrowserHost {
  constructor(browser, record = null) {
    this.browser = browser;
    this.record = record;
    this.events = new EventLog();
    this.targets = new Map();
    this.ids = new WeakMap();
    this.observers = new Map();
    this.selected = null;
    this.domWatches = new Map();
    this.domBindings = new WeakMap();
    this.browser.on('disconnected', () => this.events.push('browser.disconnected', null));
  }

  static async connect({ endpoint, browserKey, binary, target } = {}) {
    const record = endpoint ? null : chooseBrowser(await listBrowsers(binary), browserKey);
    const browser = await chromium.connectOverCDP(endpoint ?? `http://127.0.0.1:${record.cdpPort}`, { timeout: 10_000, noDefaults: true });
    const host = new BrowserHost(browser, record);
    try {
      await host.refresh();
      if (target && !host.targets.has(target)) throw new Error(`No tab with target ${target}`);
      host.selected = target ?? [...host.targets.values()].find(t => t.active)?.id ?? host.targets.keys().next().value ?? null;
      if (!host.selected) throw new Error('Browser has no registered content tabs');
      for (const context of browser.contexts()) {
        context.on('page', () => { void host.refresh().catch(error => host.events.push('bridge.error', null, { message: error.message })); });
      }
      return host;
    } catch (error) { await host.close(); throw error; }
  }

  async control(cmd, params = {}) {
    if (!this.record) throw new Error('This operation needs a terminal-browser instance');
    const id = randomUUID();
    const reply = await request(this.record.socket, { id, cmd, ...params });
    if (reply.id !== id) throw new Error('Browser control response id mismatch');
    if (!reply.ok) throw new Error(reply.error ?? 'Browser control failed');
    return reply.data;
  }

  async idFor(page) {
    if (!this.ids.has(page)) {
      const session = await page.context().newCDPSession(page);
      try { this.ids.set(page, (await session.send('Target.getTargetInfo')).targetInfo.targetId); }
      finally { await session.detach(); }
    }
    return this.ids.get(page);
  }

  async refresh() {
    if (this.closedInstance) return this.targets;
    const native = this.record ? (await this.control('targets')).tabs : null;
    const next = new Map();
    for (const page of this.browser.contexts().flatMap(c => c.pages())) {
      if (page.isClosed()) continue;
      let id;
      try { id = await this.idFor(page); } catch { continue; }
      const metadata = native?.find(t => t.targetId === id);
      if (native && !metadata) continue;
      next.set(id, { id, nativeId: metadata?.id, active: metadata?.active ?? id === this.selected, page });
      if (!this.observers.has(page)) {
        const dispose = observePage(page, id, this.events);
        this.observers.set(page, dispose);
        page.once('close', () => { dispose(); this.observers.delete(page); });
      }
    }
    this.targets = next;
    if (!this.targets.has(this.selected)) this.selected = this.targets.keys().next().value ?? null;
    return next;
  }

  current() {
    const target = this.targets.get(this.selected);
    if (!target || target.page.isClosed()) throw new Error('No selected content tab; use browser_tabs to inspect the browser');
    return target;
  }

  async tabs({ action = 'list', index, url, isolatedContext } = {}) {
    if (isolatedContext && isolatedContext !== 'default') throw new Error('Create profiles in terminal-browser; isolatedContext is not supported by this adapter');
    await this.refresh();
    let entries = [...this.targets.values()];
    if (index !== undefined && (!Number.isInteger(index) || index < 0 || index >= entries.length)) throw new Error('Tab index is out of range; refresh browser_tabs first');
    if (action === 'new') {
      let target;
      if (this.record) {
        const created = await this.control('open-tab', { url: url ?? 'about:blank' });
        target = await waitUntil(async () => { await this.refresh(); return [...this.targets.values()].find(t => t.nativeId === created.openedTab); });
      } else {
        const context = this.current().page.context();
        const page = await context.newPage();
        await this.refresh();
        target = this.targets.get(await this.idFor(page));
        if (url) await page.goto(url);
      }
      this.selected = target.id;
    } else if (action === 'select') {
      if (index === undefined) throw new Error('Selecting a tab needs index');
      const target = entries[index];
      if (this.record) await this.control('activate-tab', { tab: target.nativeId });
      this.selected = target.id;
    } else if (action === 'close') {
      const target = index === undefined ? this.current() : entries[index];
      if (this.record) await this.control('close-tab', { tab: target.nativeId });
      else await target.page.close();
      if (entries.length === 1) {
        this.closedInstance = Boolean(this.record);
        this.targets.clear();
        this.selected = null;
        return { browser: this.record?.key ?? 'cdp', tabs: [] };
      }
    } else if (action !== 'list') throw new Error('action must be list, new, select or close');
    await this.refresh();
    entries = [...this.targets.values()];
    return { browser: this.record?.key ?? 'cdp', tabs: await Promise.all(entries.map(async (t, index) => ({ index, target: t.id, nativeId: t.nativeId, selected: t.id === this.selected, url: t.page.url(), title: await t.page.title() }))) };
  }

  async touch() {
    if (this.record) await this.control('agent-touch', { tab: this.current().nativeId });
  }

  async observeDom({ selector, stop, debounceMs = 150 } = {}) {
    const { page, id: tab } = this.current();
    if (stop) {
      const watch = this.domWatches.get(stop);
      if (!watch || watch.page !== page) throw new Error('No such DOM observer on this tab');
      await page.evaluate(id => { globalThis.__herdrBrowserWatches?.get(id)?.disconnect(); globalThis.__herdrBrowserWatches?.delete(id); }, stop);
      watch.cleanup();
      return { stopped: stop };
    }
    if (typeof selector !== 'string' || !selector.trim()) throw new Error('A CSS selector is required for a targeted DOM observer');
    if (!Number.isInteger(debounceMs) || debounceMs < 50 || debounceMs > 5000) throw new Error('debounceMs must be 50..5000');
    if (this.domWatches.size >= 32) throw new Error('Stop an existing DOM observer before adding more (limit 32)');
    const id = randomUUID();
    if (!this.domBindings.has(page)) {
      const binding = `__herdrBrowser_${randomUUID().replaceAll('-', '')}`;
      await page.exposeBinding(binding, (_source, data) => {
        const watch = this.domWatches.get(data.watch);
        if (watch?.page === page) this.events.push('dom.changed', tab, { watch: data.watch, selector: watch.selector, mutations: data.mutations, connected: data.connected });
      });
      this.domBindings.set(page, binding);
    }
    const binding = this.domBindings.get(page);
    const ended = frame => {
      if (frame && frame !== page.mainFrame()) return;
      cleanup();
      this.events.push('dom.observer.ended', tab, { watch: id, reason: frame ? 'navigation' : 'closed' });
    };
    const closed = () => ended(null);
    const cleanup = () => {
      this.domWatches.delete(id);
      page.off('framenavigated', ended);
      page.off('close', closed);
    };
    this.domWatches.set(id, { page, selector, cleanup });
    try {
      await page.evaluate(({ selector, id, binding, debounceMs }) => {
        const root = document.querySelector(selector);
        if (!root) throw new Error(`No element matches ${selector}`);
        let timer;
        let changes = 0;
        const observer = new MutationObserver(records => {
          changes += records.length;
          if (timer) return;
          timer = setTimeout(() => {
            timer = null;
            void globalThis[binding]({ watch: id, mutations: changes, connected: root.isConnected }).catch(() => {});
            changes = 0;
          }, debounceMs);
        });
        observer.observe(root, { childList: true, subtree: true, attributes: true, characterData: true });
        globalThis.__herdrBrowserWatches ??= new Map();
        globalThis.__herdrBrowserWatches.set(id, { disconnect() { observer.disconnect(); clearTimeout(timer); } });
      }, { selector, id, binding, debounceMs });
    } catch (error) { cleanup(); throw error; }
    page.on('framenavigated', ended);
    page.on('close', closed);
    return { watch: id, tab, selector, endsOnNavigation: true };
  }

  close() {
    return this.closePromise ??= this.dispose();
  }

  async dispose() {
    for (const watch of [...this.domWatches.values()]) watch.cleanup();
    for (const [page, dispose] of this.observers) {
      dispose();
      if (!page.isClosed()) await page.evaluate(() => { for (const watch of globalThis.__herdrBrowserWatches?.values() ?? []) watch.disconnect(); globalThis.__herdrBrowserWatches?.clear(); }).catch(() => {});
    }
    if (this.record && this.browser.isConnected()) await this.control('agent-release').catch(() => {});
    await this.browser.close();
  }
}
