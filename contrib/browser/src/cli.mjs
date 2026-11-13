#!/usr/bin/env node
import { parseArgs } from 'node:util';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { BrowserHost, listBrowsers } from './host.mjs';
import { BrowserBridge, createAgentServer } from './server.mjs';
import { HerdrApp, terminalAdapter, panes } from './herdr.mjs';
import { startDaemon, daemonClient } from './daemon.mjs';
import { installMod } from './install.mjs';

const cli = fileURLToPath(import.meta.url);
const help = `herdr-browser: external Chromium companion for Herdr App

  open --space <id> [--url <url>] [--name <name>] [--focus]
  run [--url <url>] [--ssh <host>]       render in the current Herdr terminal
  list                                 list browser instances and CDP targets
  mcp --browser <key>                   serve Playwright MCP on stdio
  serve --browser <key> --socket <path>  keep one connection, references and events
  mcp --socket <path>                   connect an agent to that service
  tools --browser <key>                 print tool schemas
  call --browser <key> <tool> [json]     call one tool
  events --browser <key> [--types <names>] [--cursor <cursor>]
  install-mod [--herdr-home <directory>] install the optional browser launcher

Connection options: --target <CDP-target-id>, --endpoint <CDP-url>,
--binary <terminal-browser-path>, --output-dir <directory>, --herdr-home <dir>.
call, tools and events also accept --socket to reuse a service.
--endpoint connects to an existing browser, including over an SSH tunnel;
it does not stream its display into Herdr. No browser is downloaded here.
`;

export async function main(argv = process.argv.slice(2)) {
  const { values, positionals } = parseArgs({ args: argv, allowPositionals: true, options: {
    help: { type: 'boolean' }, browser: { type: 'string' }, target: { type: 'string' }, endpoint: { type: 'string' },
    binary: { type: 'string', default: process.env.TERMINAL_BROWSER_BIN ?? 'terminal-browser' },
    'output-dir': { type: 'string', default: '.artifacts' }, 'herdr-home': { type: 'string' },
    space: { type: 'string' }, url: { type: 'string', default: 'about:blank' }, name: { type: 'string', default: 'Browser' },
    focus: { type: 'boolean', default: false }, ssh: { type: 'string' }, cursor: { type: 'string' }, types: { type: 'string' }, socket: { type: 'string' },
  } });
  const [command, tool, raw = '{}'] = positionals;
  if (values.help || !command) { process.stdout.write(help); return; }
  const app = new HerdrApp(values['herdr-home']);
  if (command === 'install-mod') {
    const installed = await installMod(app.home);
    process.stdout.write(`${JSON.stringify(installed, null, 2)}\n`);
    return;
  }
  if (command === 'open') {
    const world = await app.world();
    const inferred = panes(world).find(p => p.terminal === process.env.HC_TERMINAL_ID)?.space;
    const space = world.spaces.find(s => s.id === (values.space ?? inferred));
    if (!space) throw new Error('Choose an existing space with --space <id> from herdr-app ls');
    if (space.machine) throw new Error('For a remote space, deploy the companion and terminal-browser there; use a local space with run --ssh for local rendering and remote networking');
    const args = [process.execPath, cli, 'run', '--url', values.url, '--binary', values.binary, '--herdr-home', app.home];
    if (values.ssh) args.push('--ssh', values.ssh);
    const created = await app.newTab({ space: space.id, name: values.name, argv: args, focus: values.focus });
    process.stdout.write(`${JSON.stringify(created, null, 2)}\n`);
    return;
  }
  if (command === 'run' || command === 'host') {
    if (!process.env.HC_TERMINAL_ID) throw new Error('run must be started inside a Herdr App terminal; use open --space <id>');
    const argv = command === 'host' ? JSON.parse(tool) : [values.binary, 'open', values.url, '--no-merge', ...(values.ssh ? ['--ssh', values.ssh] : [])];
    if (!Array.isArray(argv) || !argv.length || argv.some(a => typeof a !== 'string')) throw new Error('host needs an argv array');
    const adapter = await terminalAdapter(app, process.env.HC_TERMINAL_ID, argv => [process.execPath, cli, 'host', JSON.stringify(argv), '--herdr-home', app.home]);
    const child = spawn(argv[0], argv.slice(1), { stdio: 'inherit', env: { ...process.env, TERMINAL_BROWSER_TERMINAL_SOCKET: adapter.socketPath } });
    const signals = ['SIGTERM', 'SIGINT'];
    const forward = signal => child.kill(signal);
    const handlers = signals.map(signal => { const handle = () => forward(signal); process.on(signal, handle); return [signal, handle]; });
    try {
      const code = await new Promise((accept, reject) => { child.once('error', reject); child.once('exit', code => accept(code ?? 1)); });
      process.exitCode = code;
    } finally {
      handlers.forEach(([signal, handle]) => process.off(signal, handle));
      await adapter.close();
    }
    return;
  }
  if (command === 'list') { process.stdout.write(`${JSON.stringify(await listBrowsers(values.binary), null, 2)}\n`); return; }
  if (!['mcp', 'tools', 'call', 'events', 'serve'].includes(command)) throw new Error(`Unknown command: ${command}\n${help}`);
  const options = { outputDir: resolve(values['output-dir']) };
  if (command === 'serve' && !values.socket) throw new Error('serve needs --socket in a private directory');
  const host = values.socket && command !== 'serve' ? null : await BrowserHost.connect({ endpoint: values.endpoint, browserKey: values.browser, binary: values.binary, target: values.target });
  if (command === 'serve') {
    const daemon = await startDaemon(host, resolve(values.socket), options);
    const close = async () => { await daemon.close(); };
    process.once('SIGINT', () => { void close(); });
    process.once('SIGTERM', () => { void close(); });
    process.stderr.write(`Browser service listening at ${resolve(values.socket)}\n`);
    return;
  }
  const bridge = values.socket ? daemonClient(resolve(values.socket)) : new BrowserBridge(host, options);
  if (command === 'mcp') {
    const { server } = createAgentServer(bridge);
    const close = async () => { await server.close(); await bridge.close(); };
    process.once('SIGTERM', () => { void close(); });
    process.once('SIGINT', () => { void close(); });
    await server.connect(new StdioServerTransport());
    return;
  }
  try {
    if (command === 'tools') process.stdout.write(`${JSON.stringify(await bridge.tools(), null, 2)}\n`);
    if (command === 'call') {
      if (!tool) throw new Error('call needs a tool name and optional JSON arguments');
      const result = await bridge.call(tool, JSON.parse(raw));
      process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
      if (result.isError) process.exitCode = 1;
    }
    if (command === 'events') {
      const controller = new AbortController();
      const abort = () => controller.abort();
      process.once('SIGINT', abort);
      process.once('SIGTERM', abort);
      let cursor = values.cursor;
      try {
        while (!controller.signal.aborted) {
          const batch = (await bridge.call('browser_events', { cursor, types: values.types?.split(',') ?? [], waitMs: cursor ? 30_000 : 0 }, controller.signal)).structuredContent;
          cursor = batch.cursor;
          process.stdout.write(`${JSON.stringify(batch)}\n`);
        }
      } catch (error) { if (!controller.signal.aborted) throw error; }
      finally { process.off('SIGINT', abort); process.off('SIGTERM', abort); }
    }
  } finally { await bridge.close(); }
}

if (process.argv[1] && resolve(process.argv[1]) === cli) main().catch(error => { process.stderr.write(`${error.message}\n`); process.exitCode = 1; });
