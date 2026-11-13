import { randomUUID } from 'node:crypto';
import { setTimeout as delay } from 'node:timers/promises';

export class EventLog {
  constructor(capacity = 1024, maxBytes = 1024 * 1024) {
    this.capacity = capacity;
    this.maxBytes = maxBytes;
    this.bytes = 0;
    this.sizes = [];
    this.epoch = randomUUID();
    this.sequence = 0;
    this.events = [];
    this.waiters = new Set();
  }

  push(type, tab, data = {}) {
    const event = { sequence: ++this.sequence, timestamp: new Date().toISOString(), type, tab, data };
    this.events.push(event);
    const bytes = Buffer.byteLength(JSON.stringify(event));
    this.sizes.push(bytes);
    this.bytes += bytes;
    // A count limit alone does not bound memory when a page emits long URLs.
    // Byte eviction uses the same explicit cursor-gap recovery as count eviction.
    while (this.events.length && (this.events.length > this.capacity || this.bytes > this.maxBytes)) {
      this.events.shift();
      this.bytes -= this.sizes.shift();
    }
    for (const wake of [...this.waiters]) wake();
    return event;
  }

  parse(cursor) {
    if (cursor === undefined) return this.sequence;
    if (typeof cursor !== 'string') throw new Error('cursor must be the string returned by browser_events');
    const [epoch, sequence] = cursor.split(':');
    if (epoch !== this.epoch) throw new Error('event_session_changed: read a fresh snapshot and omit cursor');
    const after = Number(sequence);
    if (!Number.isSafeInteger(after) || after < 0 || after > this.sequence) throw new Error('Invalid event cursor');
    return after;
  }

  async read({ cursor, types = [], tab, waitMs = 0, limit = 100 } = {}, signal) {
    if (!Array.isArray(types) || types.some(t => typeof t !== 'string')) throw new Error('types must be an array of event names');
    if (!Number.isInteger(waitMs) || waitMs < 0 || waitMs > 30_000) throw new Error('waitMs must be 0..30000');
    if (!Number.isInteger(limit) || limit < 1 || limit > 500) throw new Error('limit must be 1..500');
    const after = this.parse(cursor);
    const deadline = Date.now() + waitMs;
    for (;;) {
      signal?.throwIfAborted();
      const oldest = this.events[0]?.sequence ?? this.sequence + 1;
      const gap = after < oldest - 1;
      const matches = this.events.filter(e => e.sequence > after && (!types.length || types.includes(e.type)) && (!tab || e.tab === tab));
      const events = matches.slice(0, limit);
      if (events.length || gap || Date.now() >= deadline) {
        const next = matches.length > limit ? events.at(-1).sequence : this.sequence;
        return { cursor: `${this.epoch}:${next}`, gap, events };
      }
      await new Promise((resolve, reject) => {
        const wake = () => { cleanup(); resolve(); };
        const abort = () => { cleanup(); reject(signal.reason); };
        const timer = setTimeout(wake, deadline - Date.now());
        const cleanup = () => { clearTimeout(timer); this.waiters.delete(wake); signal?.removeEventListener('abort', abort); };
        this.waiters.add(wake);
        signal?.addEventListener('abort', abort, { once: true });
        if (signal?.aborted) abort();
      });
    }
  }
}

const short = value => String(value).slice(0, 2048);

export function observePage(page, tab, log) {
  const listeners = [];
  const on = (name, listener) => { page.on(name, listener); listeners.push([name, listener]); };
  log.push('tab.opened', tab, { url: page.url() });
  on('framenavigated', frame => { if (frame === page.mainFrame()) log.push('navigation', tab, { url: frame.url() }); });
  on('domcontentloaded', () => log.push('dom.ready', tab, { url: page.url() }));
  on('load', () => log.push('page.loaded', tab, { url: page.url() }));
  on('console', message => log.push('console', tab, { level: message.type(), text: short(message.text()) }));
  on('pageerror', error => log.push('page.error', tab, { message: short(error.message) }));
  on('requestfailed', request => log.push('request.failed', tab, { url: request.url(), method: request.method(), error: request.failure()?.errorText }));
  on('response', response => { if (response.status() >= 400) log.push('response.error', tab, { url: response.url(), status: response.status() }); });
  on('download', download => log.push('download', tab, { url: download.url(), filename: download.suggestedFilename() }));
  on('dialog', dialog => log.push('dialog', tab, { type: dialog.type(), message: short(dialog.message()) }));
  on('crash', () => log.push('tab.crashed', tab));
  on('close', () => log.push('tab.closed', tab));
  return () => listeners.forEach(([name, listener]) => page.off(name, listener));
}

export async function waitUntil(predicate, timeout = 5000) {
  const deadline = Date.now() + timeout;
  do {
    const result = await predicate();
    if (result) return result;
    await delay(50);
  } while (Date.now() < deadline);
  throw new Error('Timed out waiting for the browser to register its page');
}
