import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventLog } from '../src/events.mjs';
import { chooseBrowser } from '../src/host.mjs';

test('event readers wait for their own filter, replay and retain unread batches', async () => {
  const log = new EventLog();
  const { cursor } = await log.read();
  const read = log.read({ cursor, types: ['navigation'], tab: 'tab-1', waitMs: 1000, limit: 1 });
  log.push('console', 'tab-1');
  log.push('navigation', 'tab-2');
  log.push('navigation', 'tab-1', { url: '/first' });
  log.push('navigation', 'tab-1', { url: '/second' });
  const first = await read;
  assert.equal(first.events.length, 1);
  assert.equal(first.events[0].data.url, '/first');
  const second = await log.read({ cursor: first.cursor, types: ['navigation'], tab: 'tab-1' });
  assert.equal(second.events[0].data.url, '/second');
});

test('event eviction and a restarted collector are explicit', async () => {
  const log = new EventLog(2);
  const { cursor } = await log.read();
  for (let i = 0; i < 3; i++) log.push('console', 'tab-1');
  assert.equal((await log.read({ cursor })).gap, true);
  await assert.rejects(new EventLog().read({ cursor }), /event_session_changed/);
});

test('long event payloads respect the byte budget and expose eviction gaps', async () => {
  const log = new EventLog(1024, 700);
  const { cursor } = await log.read();
  log.push('navigation', 'tab-1', { url: 'x'.repeat(400) });
  log.push('navigation', 'tab-1', { url: 'y'.repeat(400) });
  const batch = await log.read({ cursor });
  assert.equal(batch.gap, true);
  assert.equal(batch.events.length, 1);
  assert.ok(log.bytes <= 700);
  log.push('navigation', 'tab-1', { url: 'z'.repeat(1000) });
  const oversized = await log.read({ cursor: batch.cursor });
  assert.equal(oversized.gap, true);
  assert.equal(oversized.events.length, 0);
  assert.equal(log.bytes, 0);
  log.push('navigation', 'tab-1', { url: '/small' });
  assert.equal((await log.read({ cursor: oversized.cursor })).events[0].data.url, '/small');
});

test('cancelled event waits release their listeners', async () => {
  const log = new EventLog();
  const controller = new AbortController();
  const wait = log.read({ waitMs: 30_000 }, controller.signal);
  controller.abort();
  await assert.rejects(wait);
  assert.equal(log.waiters.size, 0);
});

test('instance selection never picks an arbitrary browser', () => {
  const a = { key: 'a', cdpPort: 1234, inCurrentTab: false };
  const b = { key: 'b', cdpPort: 1235, inCurrentTab: false };
  assert.throws(() => chooseBrowser([a, b]), /explicitly/);
  assert.equal(chooseBrowser([a, b], 'b'), b);
  assert.throws(() => chooseBrowser([{ ...a, cdpPort: null }], 'a'), /CDP/);
});
