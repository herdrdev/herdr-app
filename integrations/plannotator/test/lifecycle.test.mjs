import assert from 'node:assert/strict';
import test from 'node:test';
import { createReviewRouter } from '../router.mjs';

const event = (id, url = 'http://localhost:1234', piSessionId = 'owner') => ({ id, url, piSessionId });
const deferred = () => { let resolve; const promise = new Promise(r => { resolve = r; }); return { promise, resolve }; };

test('reusing a review port does not reuse its cancelled lifetime', async () => {
  const opened = [], closed = [];
  const router = createReviewRouter('owner', { open: async e => { opened.push(e.id); return { kind: 'embedded', pane: e.id }; }, close: async r => closed.push(r.pane) });
  try {
    router.opened(event('old')); await router.closed(event('old'));
    router.opened(event('new'));
    assert.deepEqual(await router.request(event('old').url), { kind: 'closed', review: 'old' });
    assert.deepEqual(await router.request(event('new').url), { kind: 'embedded', pane: 'new', review: 'new' });
    await router.closed(event('old'));
    assert.deepEqual(closed, []);
    await router.closed(event('new'));
    assert.deepEqual(opened, ['new']);
    assert.deepEqual(closed, ['new']);
  } finally { await router.shutdown(); }
});

test('different reviews open concurrently and close only their own panes', async () => {
  const first = deferred(), second = deferred(), closed = [];
  const router = createReviewRouter('owner', { open: e => e.id === 'one' ? first.promise : second.promise, close: async r => closed.push(r.pane) });
  try {
    router.opened(event('one')); router.opened(event('two', 'http://localhost:5678'));
    const a = router.request(event('one').url), b = router.request(event('two', 'http://localhost:5678').url);
    second.resolve({ kind: 'embedded', pane: 'two' });
    assert.equal((await b).pane, 'two');
    await router.closed(event('two', 'http://localhost:5678'));
    first.resolve({ kind: 'embedded', pane: 'one' });
    assert.equal((await a).pane, 'one');
    assert.deepEqual(closed, ['two']);
  } finally { await router.shutdown(); }
  assert.deepEqual(closed, ['two', 'one']);
});

test('a close during opening disposes the eventual pane exactly once', async () => {
  const pending = deferred(), started = deferred(), closed = [];
  const router = createReviewRouter('owner', { open: () => { started.resolve(); return pending.promise; }, close: async r => closed.push(r.pane) });
  router.opened(event('one'));
  const request = router.request(event('one').url);
  await started.promise;
  const closing = router.closed(event('one'));
  pending.resolve({ kind: 'embedded', pane: 'late' });
  assert.deepEqual(await request, { kind: 'closed', review: 'one' });
  await closing;
  await router.shutdown();
  assert.deepEqual(closed, ['late']);
});

test('a hook may arrive first; foreign sessions cannot open or close a review', async () => {
  const closed = [];
  const router = createReviewRouter('owner', { open: async e => ({ kind: 'embedded', pane: e.id }), close: async r => closed.push(r.pane), missingDelay: 10 });
  try {
    const waiting = router.request(event('one').url);
    router.opened(event('foreign', event('one').url, 'someone-else'));
    router.opened(event('one'));
    assert.equal((await waiting).pane, 'one');
    await router.closed(event('one', event('one').url, 'someone-else'));
    assert.deepEqual(closed, []);
    assert.deepEqual(await router.request('http://localhost:9999'), { kind: 'external' });
  } finally { await router.shutdown(); }
  assert.deepEqual(closed, ['one']);
});
