import assert from 'node:assert/strict';
import test from 'node:test';
import { createRequestNamer } from '../requested.mjs';

test('the review a tab bar request opens carries the button name; other reviews do not', () => {
  let clock = 0;
  const namer = createRequestNamer({ now: () => clock, within: 1000 });
  const opened = action => ({ id: 'pn-1', action, url: 'http://localhost:1' });
  assert.equal(namer.name(opened('code-review')).tabName, undefined);
  namer.expect('review');
  // An agent's own plan review in the meantime keeps its name.
  assert.equal(namer.name(opened('plan-review')).tabName, undefined);
  assert.equal(namer.name(opened('code-review')).tabName, 'Annotate diff');
  assert.equal(namer.name(opened('code-review')).tabName, undefined, 'one request names one review');
  namer.expect('last');
  assert.deepEqual(namer.name(opened('annotate-last')), { ...opened('annotate-last'), tabName: 'Annotate reply' });
  namer.expect('last');
  clock = 2000;
  assert.equal(namer.name(opened('annotate-last')).tabName, undefined, 'a stale request names nothing');
});
