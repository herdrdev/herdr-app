import net from 'node:net';
import { randomUUID } from 'node:crypto';
import { chmod, stat, unlink } from 'node:fs/promises';
import { dirname } from 'node:path';
import { request, serveLine } from './rpc.mjs';
import { BrowserBridge } from './server.mjs';

export async function startDaemon(host, socketPath, options = {}) {
  const directory = await stat(dirname(socketPath));
  if (directory.mode & 0o077) throw new Error('The daemon socket must live in a private directory (chmod 700)');
  const bridge = new BrowserBridge(host, options);
  const sockets = new Set();
  const server = net.createServer(socket => {
    sockets.add(socket);
    socket.once('close', () => sockets.delete(socket));
    serveLine(socket, async ({ method, params = {} }, signal) => {
      if (method === 'tools/list') return { tools: await bridge.tools() };
      if (method === 'tools/call') return bridge.call(params.name, params.arguments, signal);
      throw new Error(`Unsupported daemon method: ${method}`);
    }, 70_000);
  });
  try {
    await new Promise((resolve, reject) => { server.once('error', reject); server.listen(socketPath, resolve); });
    await chmod(socketPath, 0o600);
  } catch (error) { await bridge.close(); throw error; }
  let closePromise;
  return { bridge, server, close() {
    return closePromise ??= (async () => {
      for (const socket of sockets) socket.destroy();
      await new Promise(resolve => server.close(resolve));
      await bridge.close();
      await unlink(socketPath).catch(error => { if (error.code !== 'ENOENT') throw error; });
    })();
  } };
}

export function daemonClient(socketPath) {
  const call = async (method, params, signal) => {
    const id = randomUUID();
    const response = await request(socketPath, { id, method, params }, { timeout: 70_000, signal });
    if (response.id !== id) throw new Error('Daemon response id mismatch');
    if (response.error) throw new Error(response.error.message);
    return response.result;
  };
  return {
    async tools() { return (await call('tools/list')).tools; },
    call(name, args, signal) { return call('tools/call', { name, arguments: args }, signal); },
    async close() {},
  };
}
