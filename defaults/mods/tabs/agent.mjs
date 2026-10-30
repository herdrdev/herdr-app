// Optional Node adapter. The GPUI mod does not load or start this process.
import { readFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { createInterface } from 'node:readline';
import { createServer } from 'node:http';

export const tools = JSON.parse(await readFile(new URL('./agent-tools.json', import.meta.url), 'utf8'));
const execute = promisify(execFile);
async function app(args) {
  const { stdout } = await execute(process.env.HERDR_APP_BIN || 'herdr-app', args, { timeout: 10000, maxBuffer: 8 * 1024 * 1024 });
  const result = JSON.parse(stdout);
  if (result.error) throw new Error(typeof result.error === 'string' ? result.error : JSON.stringify(result.error));
  return result;
}
export function validate(tool, args) {
  if (!args || typeof args !== 'object' || Array.isArray(args)) throw new Error('arguments must be an object');
  const schema = tool.inputSchema;
  for (const key of Object.keys(args)) if (!Object.hasOwn(schema.properties, key)) throw new Error(`unknown argument: ${key}`);
  for (const key of schema.required ?? []) if (!Object.hasOwn(args, key)) throw new Error(`missing argument: ${key}`);
  for (const [key, value] of Object.entries(args)) {
    if (typeof value !== schema.properties[key].type || (schema.properties[key].minLength && !value.trim())) throw new Error(`invalid argument: ${key}`);
  }
}
export async function callTool(name, args = {}, run = app) {
  const tool = tools.find(t => t.name === name);
  if (!tool) throw new Error(`unknown tool: ${name}`);
  validate(tool, args);
  if (tool.command) {
    const result = await run(['do', tool.command, JSON.stringify(args)]);
    return { id: args.id, mode: args.name.trim() ? 'manual' : 'automatic', result };
  }
  // Confirm there is a live window; never mistake an old disk snapshot for one.
  const world = await run(['ls']);
  const home = process.env.HERDR_APP_HOME || join(process.env.XDG_CONFIG_HOME || join(homedir(), '.config'), 'herdr-app');
  const shared = JSON.parse(await readFile(join(home, 'shared.json'), 'utf8'));
  const snapshot = shared['titles.snapshot'];
  if (!snapshot?.data) throw new Error('No title snapshot yet. Show a shipped tab bar (Herdr, Terminals or Herdr 98).');
  const ids = spaces => spaces.flatMap(s => [s.id, ...s.tabs.map(t => t.id)]).sort();
  if (JSON.stringify(ids(world.spaces)) !== JSON.stringify(ids(snapshot.data.spaces)))
    throw new Error('The title snapshot is catching up with the window. Retry after its next render.');
  return { source: 'last-rendered', ...snapshot };
}

const serverInfo = { name: 'herdr-titles', version: '1.0.0' };
// A deliberately small MCP 2025-11-25 stdio server; no network transport is
// enabled by MCP startup. Newer clients can negotiate this supported revision.
export async function rpc(request, call = callTool) {
  if (!request || request.jsonrpc !== '2.0' || typeof request.method !== 'string')
    return { jsonrpc: '2.0', id: request?.id ?? null, error: { code: -32600, message: 'Invalid Request' } };
  if (!Object.hasOwn(request, 'id')) return null;
  const response = { jsonrpc: '2.0', id: request.id };
  switch (request.method) {
    case 'initialize': return { ...response, result: { protocolVersion: '2025-11-25', serverInfo, capabilities: { tools: {} } } };
    case 'ping': return { ...response, result: {} };
    case 'tools/list': return { ...response, result: { tools: tools.map(({ http, command, uiActions, ...tool }) => tool) } };
    case 'tools/call': {
      try {
        const result = await call(request.params?.name, request.params?.arguments ?? {});
        return { ...response, result: { content: [{ type: 'text', text: JSON.stringify(result) }], structuredContent: result } };
      } catch (error) {
        return { ...response, result: { isError: true, content: [{ type: 'text', text: error.message }] } };
      }
    }
    default: return { ...response, error: { code: -32601, message: 'Method not found' } };
  }
}
export function httpServer(call = callTool) {
  return createServer(async (req, res) => {
    const send = (status, value) => { res.writeHead(status, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' }); res.end(JSON.stringify(value)); };
    // This is a local agent API, not a browser endpoint. Reject browser origins
    // and non-loopback Host headers to prevent cross-site and rebinding writes.
    if (req.headers.origin || !/^127\.0\.0\.1:\d+$/.test(req.headers.host ?? '')) return send(403, { error: 'Loopback agent clients only' });
    if (req.method === 'GET' && req.url === '/v1/tools') return send(200, { tools });
    const tool = tools.find(t => t.http.method === req.method && t.http.path === req.url);
    if (!tool) return send(404, { error: 'Unknown endpoint' });
    try {
      let body = '';
      for await (const chunk of req) {
        body += chunk;
        if (Buffer.byteLength(body) > 16384) return send(413, { error: 'Request too large' });
      }
      send(200, await call(tool.name, body ? JSON.parse(body) : {}));
    } catch (error) { send(400, { error: error.message }); }
  });
}
async function main() {
  if (process.argv[2] === '--http') {
    const port = Number(process.argv[3] ?? 0);
    if (!Number.isInteger(port) || port < 0 || port > 65535) throw new Error('port must be 0..65535');
    const server = httpServer();
    server.listen(port, '127.0.0.1', () => console.log(JSON.stringify({ url: `http://127.0.0.1:${server.address().port}` })));
    server.on('error', error => { console.error(error.message); process.exitCode = 1; });
    return;
  }
  if (process.argv.length > 2) throw new Error('usage: node agent.mjs [--http PORT]');
  for await (const line of createInterface({ input: process.stdin, crlfDelay: Infinity })) {
    let reply;
    try { reply = await rpc(JSON.parse(line)); }
    catch { reply = { jsonrpc: '2.0', id: null, error: { code: -32700, message: 'Parse error' } }; }
    if (reply) process.stdout.write(JSON.stringify(reply) + '\n');
  }
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href)
  main().catch(error => { console.error(error.message); process.exitCode = 1; });
