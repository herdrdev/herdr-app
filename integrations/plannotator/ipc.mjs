import net from 'node:net';
import { randomBytes } from 'node:crypto';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';

const limit = 8192;
export async function startBrowserIpc(session, router, report) {
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
    return new Promise((resolve, reject) => {
        const client = net.createConnection(env.HERDR_PLANNOTATOR_SOCKET);
        let input = '';
        client.setEncoding('utf8'); client.setTimeout(60000, () => client.destroy(new Error('Browser routing timed out')));
        client.on('error', reject);
        client.on('connect', () => client.write(`${JSON.stringify({ url, token: env.HERDR_PLANNOTATOR_TOKEN, session: env.HERDR_PLANNOTATOR_SESSION_ID })}\n`));
        client.on('data', chunk => {
            input += chunk;
            if (Buffer.byteLength(input) > limit) client.destroy(new Error('Browser routing response is too large'));
        });
        client.on('end', () => {
            try { const result = JSON.parse(input); resolve(result); }
            catch (error) { reject(error); }
            finally { client.destroy(); }
        });
    });
}
