import net from 'node:net';

export function request(socketPath, message, { timeout = 10_000, signal } = {}) {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection(socketPath);
    let buffer = '';
    let settled = false;
    const finish = (error, result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      signal?.removeEventListener('abort', abort);
      socket.destroy();
      error ? reject(error) : resolve(result);
    };
    const abort = () => finish(signal.reason ?? new Error('Request cancelled'));
    const timer = setTimeout(() => finish(new Error(`No reply from ${socketPath}`)), timeout);
    socket.setEncoding('utf8');
    socket.on('connect', () => socket.write(`${JSON.stringify(message)}\n`));
    socket.on('error', finish);
    socket.on('end', () => finish(new Error('Connection closed before a complete reply')));
    socket.on('data', chunk => {
      buffer += chunk;
      if (Buffer.byteLength(buffer) > 8 * 1024 * 1024) return finish(new Error('Reply exceeds 8 MiB'));
      const end = buffer.indexOf('\n');
      if (end < 0) return;
      try { finish(null, JSON.parse(buffer.slice(0, end))); }
      catch (error) { finish(error); }
    });
    signal?.addEventListener('abort', abort, { once: true });
    if (signal?.aborted) abort();
  });
}

export async function serveLine(socket, handler, timeout = 10_000) {
  let buffer = '';
  const controller = new AbortController();
  socket.setEncoding('utf8');
  socket.setTimeout(timeout, () => socket.destroy());
  socket.on('close', () => controller.abort());
  socket.on('error', () => {});
  socket.on('data', async chunk => {
    buffer += chunk;
    if (Buffer.byteLength(buffer) > 1024 * 1024) return socket.destroy();
    const end = buffer.indexOf('\n');
    if (end < 0) return;
    socket.removeAllListeners('data');
    let input;
    try {
      input = JSON.parse(buffer.slice(0, end));
      const result = await handler(input, controller.signal);
      socket.end(`${JSON.stringify({ id: input.id, result })}\n`);
    } catch (error) {
      socket.end(`${JSON.stringify({ id: input?.id ?? null, error: { message: error.message } })}\n`);
    }
  });
}
