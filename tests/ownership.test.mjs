import test from 'node:test';
import assert from 'node:assert/strict';
import { classifyExplicitOwner, liveOwner } from '../dist/herdr/ownership.js';

const identity = 'a'.repeat(64), socket = '/socket';
const nonce = '12345678-1234-4234-8234-123456789abc';
const pane = (pane_id, terminal_id, tokens = {}) => ({ pane_id, terminal_id, workspace_id: 'w1', tab_id: 'w1:t1', tokens });
const tuple = () => ({ wc_card: identity, wc_nonce: nonce, wc_pane: 'w1:p2', wc_terminal: 'term2' });
const fixture = () => ({ workspace: { workspace_id: 'w1', tokens: tuple() }, caller: pane('w1:p1', 'term1'), panes: [] });

test('explicit ownership decision table is pure and preserves conservative live/orphan rules', async t => {
  const cases = [
    ['empty', f => { f.workspace.tokens = { other: 'kept' }; }, 'empty'],
    ['orphan', () => {}, 'recoverable-orphan'],
    ['live', f => { f.panes.push(pane('w1:p2', 'term2', { ...tuple(), wc_role: 'card' })); }, 'live'],
    ['noncanonical but consistent live nonce', f => { f.workspace.tokens.wc_nonce = 'legacy'; f.panes.push(pane('w1:p2', 'term2', { ...tuple(), wc_nonce: 'legacy', wc_role: 'card' })); }, 'live'],
    ['noncanonical but consistent live identity', f => { f.workspace.tokens.wc_card = 'legacy'; f.panes.push(pane('w1:p2', 'term2', { ...tuple(), wc_card: 'legacy', wc_role: 'card' })); f.identity = 'legacy'; }, 'live'],
    ...['wc_card', 'wc_nonce', 'wc_pane', 'wc_terminal'].map(key => [`missing ${key}`, f => { delete f.workspace.tokens[key]; }]),
    ['summary only', f => { f.workspace.tokens = { wc_summary: 'old' }; }],
    ['foreign card', f => { f.workspace.tokens.wc_card = 'f'.repeat(64); }],
    ['malformed identity', f => { f.identity = f.workspace.tokens.wc_card = 'legacy'; }],
    ['malformed nonce', f => { f.workspace.tokens.wc_nonce = 'legacy'; }],
    ['foreign pane', f => { f.workspace.tokens.wc_pane = 'w2:p2'; }],
    ['malformed pane', f => { f.workspace.tokens.wc_pane = 'w1:p!'; }],
    ['malformed terminal', f => { f.workspace.tokens.wc_terminal = ' '; }],
    ['restored shell', f => { f.panes.push(pane('w1:p2', 'restored')); }],
    ['live terminal elsewhere', f => { f.panes.push(pane('w1:p9', 'term2')); }],
    ['marked pane', f => { f.panes.push(pane('w1:p9', 'term9', { wc_role: 'card' })); }],
    ['partial pane marker', f => { f.panes.push(pane('w1:p9', 'term9', { wc_nonce: nonce })); }],
    ['duplicate pane', f => { f.panes.push(pane('w1:p9', 'term9'), pane('w1:p9', 'term10')); }],
    ['duplicate terminal', f => { f.panes.push(pane('w1:p9', 'term9'), pane('w1:p10', 'term9')); }],
    ['missing caller', f => { f.panes.shift(); }],
    ['changed caller tab', f => { f.caller = { ...f.caller, tab_id: 'other' }; }],
    ['multiple owners', f => { f.panes.push(pane('w1:p2', 'term2', { ...tuple(), wc_role: 'card' }), pane('w1:p3', 'term3', { ...tuple(), wc_role: 'card' })); }],
    ['inconsistent live nonce', f => { f.panes.push(pane('w1:p2', 'term2', { ...tuple(), wc_nonce: 'other', wc_role: 'card' })); }],
  ];
  for (const [name, mutate, expected] of cases) await t.test(name, () => {
    const f = fixture(); f.panes.push(f.caller); mutate(f);
    const before = structuredClone(f);
    const classify = () => classifyExplicitOwner(socket, f.identity ?? identity, f.workspace, f.panes, f.caller);
    if (expected) {
      const result = classify(); assert.equal(result.kind, expected);
      if (expected === 'live') assert.deepEqual(result.attachment, { socket, workspaceId: 'w1', paneId: 'w1:p2', terminalId: 'term2', cardIdentity: f.identity ?? identity, nonce: f.workspace.tokens.wc_nonce });
    } else assert.throws(classify, /conflict|ambiguous/i);
    assert.deepEqual(f, before);
  });
});

test('publication never classifies absent ownership as recoverable', () => {
  const f = fixture();
  assert.throws(() => liveOwner(socket, identity, f.workspace, [f.caller]), /conflict/);
});
