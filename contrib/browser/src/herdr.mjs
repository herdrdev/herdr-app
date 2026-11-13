import { homedir, tmpdir } from 'node:os';
import { join } from 'node:path';
import { mkdtemp, chmod, rm } from 'node:fs/promises';
import net from 'node:net';
import { request, serveLine } from './rpc.mjs';

export function appHome(env = process.env) {
  return env.HERDR_APP_HOME ?? join(env.XDG_CONFIG_HOME ?? join(homedir(), '.config'), 'herdr-app');
}

export class HerdrApp {
  constructor(home = appHome()) { this.home = home; }
  async call(message) {
    const result = await request(join(this.home, 'herdr-app.sock'), message);
    if (result.error) throw new Error(result.error);
    return result;
  }
  world() { return this.call({ ls: true }); }
  run(name, args) { return this.call({ do: name, args }); }
  async newTab(args) {
    const commands = await this.call({ commands: true });
    const supportsFocus = commands.find(c => c.name === 'tab.new')?.args.some(a => a.name === 'focus');
    if (!supportsFocus) {
      if (args.focus === false) throw new Error('This Herdr release cannot create background tabs. Pass --focus or update Herdr.');
      args = { ...args };
      delete args.focus;
    }
    return this.run('tab.new', args);
  }
}

export function panes(world) {
  return (world.spaces ?? []).flatMap(space => space.tabs.flatMap(tab => (tab.terminals ?? []).map(terminal => ({
    id: terminal.pane, tab: tab.id, space: space.id, machine: space.machine,
    terminal: terminal.terminal, command: terminal.app ?? terminal.name ?? null, tty: null,
  }))));
}

export async function terminalAdapter(app, terminal, wrap = argv => argv) {
  const folder = await mkdtemp(join(tmpdir(), 'herdr-browser-'));
  await chmod(folder, 0o700);
  const socketPath = join(folder, 'terminal.sock');
  const handle = async ({ method, params = {} }) => {
    if (method === 'hello') {
      if (params.protocol !== 1) throw new Error('Unsupported terminal socket protocol');
      return { name: 'herdr-app', methods: ['getCurrentPane', 'listPanes', 'focusPane', 'split'] };
    }
    const all = panes(await app.world());
    const current = all.find(p => p.terminal === terminal);
    if (!current) throw new Error(`Terminal ${terminal} has no Herdr pane`);
    const target = id => {
      const pane = all.find(p => p.id === id && p.tab === current.tab);
      if (!pane) throw new Error('Target pane is outside this browser tab');
      return pane;
    };
    switch (method) {
      case 'getCurrentPane': return { pane: { id: current.id, tab: current.tab } };
      case 'listPanes': return { panes: all.filter(p => p.tab === current.tab).map(({ id, tab, tty, command }) => ({ id, tab, tty, command })) };
      case 'focusPane': await app.run('pane.focus', { pane: target(params.pane).id }); return {};
      case 'split': {
        const from = target(params.from?.id);
        if (!['right', 'left', 'down', 'up'].includes(params.direction)) throw new Error('Invalid split direction');
        if (!Array.isArray(params.command) || !params.command.length || params.command.some(arg => typeof arg !== 'string')) throw new Error('command must be an argv array');
        const direction = ['left', 'right'].includes(params.direction) ? 'right' : 'down';
        const created = await app.run('pane.split', { pane: from.id, direction, argv: wrap(params.command) });
        if (['left', 'up'].includes(params.direction)) await app.run('pane.swap', { pane: created.pane, target: from.id });
        return {};
      }
      default: throw new Error(`Unsupported terminal operation: ${method}`);
    }
  };
  const server = net.createServer(socket => serveLine(socket, handle));
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(socketPath, resolve); });
  await chmod(socketPath, 0o600);
  return { socketPath, handle, async close() { await new Promise(resolve => server.close(resolve)); await rm(folder, { recursive: true, force: true }); } };
}
