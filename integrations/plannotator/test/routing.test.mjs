import assert from 'node:assert/strict';
import test from 'node:test';
import { findOriginSpace } from '../open.mjs';

const spaces = [
  { id: 'laptop/sp-one', tabs: [{ terminals: [{ terminal: 'laptop/t7', pane: 'laptop/p1' }] }] },
  { id: 'worker/sp-two', tabs: [{ terminals: [{ terminal: 'worker/t7', pane: 'worker/p2' }] }] },
];
test('a review opens in the requester’s space even when terminal ids repeat on another machine', () => {
  assert.equal(findOriginSpace({ spaces }, { address: 't7', labels: { 'herdr.space': 'sp-one' } }), 'laptop/sp-one');
  assert.equal(findOriginSpace({ spaces }, { address: 't7', labels: { 'herdr.space': 'sp-two' } }), 'worker/sp-two');
});
test('unknown and ambiguous origins never fall back to the active agent', () => {
  assert.equal(findOriginSpace({ spaces }, { address: 't7', labels: {} }), undefined);
  assert.equal(findOriginSpace({ spaces }, { address: 't9', labels: { 'herdr.space': 'sp-one' } }), undefined);
  const duplicate = [...spaces, { id: 'other/sp-one', tabs: [{ terminals: [{ terminal: 'other/t7', pane: 'other/p3' }] }] }];
  assert.equal(findOriginSpace({ spaces: duplicate }, { address: 't7', labels: { 'herdr.space': 'sp-one' } }), undefined);
});
