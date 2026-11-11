import net from 'node:net';
import { randomBytes } from 'node:crypto';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';

const limit = 8192;
// The Plannotator commands another program may run in this Pi (Herdr's tab bar).
export const COMMANDS = ['review', 'last'];

// `run(command)` starts one of COMMANDS in this Pi; it answers before the review ends.
export async function startBrowserIpc(session, router, report, run) {
    const token = randomBytes(32).toString('hex');
    const directory = process.platform === 'win32' ? undefined : await mkdtemp(path.join(tmpdir(), 'herdr-browser-'));
    const socket = directory ? path.join(directory, 'browser.sock') : `\\\\.\\pipe\\herdr-browser-${process.pid}-${token}`;
    const clients = new Set();
    const server = net.createServer(client => {
        clients.add(client); client.on('close', () => clients.delete(client));
        client.on('error', () => {}); // Disconnects are normal when a hook is cancelled.
        client.setTimeout(60000, () => client.destroy()); client.setEncoding('utf8');
        let input = '', processing = false;
        client.on('data', async chunk => {
            if (processing) return;
            input += chunk;
            if (Buffer.byteLength(input) > limit) { client.destroy(); return; }
            const newline = input.indexOf('\n');
            if (newline === -1) return;
            processing = true; client.pause();
            const send = value => { if (!client.destroyed) client.end(`${JSON.stringify(value)}\n`, () => client.destroy()); };
            let request;
            try { request = JSON.parse(input.slice(0, newline)); }
            catch { send({ kind: 'error', code: 'invalid-request' }); return; }
            if (request?.token !== token || request?.session !== session) { send({ kind: 'error', code: 'owner-mismatch' }); return; }
            if (request.command !== undefined) {
                if (!run || !COMMANDS.includes(request.command)) { send({ kind: 'error', code: 'invalid-command' }); return; }
                try { await run(request.command); send({ kind: 'started' }); }
                catch (error) { report(error); send({ kind: 'error', code: 'command-failed', message: error instanceof Error ? error.message : String(error) }); }
                return;
            }
            try {
                const url = new URL(request.url);
                if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password) throw new Error('Invalid browser URL');
            } catch { send({ kind: 'error', code: 'invalid-url' }); return; }
            try { send(await router.request(request.url)); }
            catch (error) { report(error); send({ kind: 'error', code: 'open-failed' }); }
        });
    });
    server.maxConnections = 32;
    try {
        await new Promise((resolve, reject) => { server.once('error', reject); server.listen(socket, () => { server.removeListener('error', reject); resolve(); }); });
    } catch (error) { if (directory) await rm(directory, { recursive: true, force: true }); throw error; }
    server.on('error', report);
    return {
        socket, token,
        async stop() {
            for (const client of clients) client.destroy();
            await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
            if (directory) await rm(directory, { recursive: true, force: true });
        },
    };
}

export function requestBrowser(url, env = process.env) {
    return exchange(env.HERDR_PLANNOTATOR_SOCKET, { url, token: env.HERDR_PLANNOTATOR_TOKEN, session: env.HERDR_PLANNOTATOR_SESSION_ID });
}

// Run one of COMMANDS in the Pi that published `{socket, token, session}`.
export function requestCommand(command, { socket, token, session }) {
    return exchange(socket, { command, token, session });
}

function exchange(socket, message) {
    return new Promise((resolve, reject) => {
        const client = net.createConnection(socket);
        let input = '';
        client.setEncoding('utf8'); client.setTimeout(60000, () => client.destroy(new Error('The Pi session did not answer in time')));
        client.on('error', reject);
        client.on('connect', () => client.write(`${JSON.stringify(message)}\n`));
        client.on('data', chunk => {
            input += chunk;
            if (Buffer.byteLength(input) > limit) client.destroy(new Error('The Pi session answer is too large'));
        });
        client.on('end', () => {
            try { const result = JSON.parse(input); resolve(result); }
            catch (error) { reject(error); }
            finally { client.destroy(); }
        });
    });
}
