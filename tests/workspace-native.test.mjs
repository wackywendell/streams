import test from 'node:test';
import assert from 'node:assert/strict';
import { parseNativeSnapshot, locateTerminal, classifyPair, pairMarkers } from '../dist/herdr/workspace-ownership.js';

const pane = (n, tokens = {}) => ({ pane_id: `w:p${n}`, terminal_id: `term${n}`, workspace_id: 'w', tab_id: `w:t${Math.ceil(n/2)}`, tokens });
const snapshot = panes => parseNativeSnapshot({ workspaces: [{ workspace_id: 'w' }], tabs: [...new Set(panes.map(p => p.tab_id))].map(tab_id => ({ tab_id, workspace_id: 'w', label: '1', number: 1 })), panes, layouts: [...new Set(panes.map(p => p.tab_id))].map(tab_id => ({ tab_id, workspace_id: 'w', panes: panes.filter(p => p.tab_id === tab_id).map(p => ({ pane_id: p.pane_id })) })) });
test('two independent pairs coexist in one workspace and track terminals after pane IDs change', () => {
  const a = pairMarkers('a'.repeat(64), 'nonce', 'term1', 'term2');
  const b = pairMarkers('b'.repeat(64), 'other', 'term3', 'term4');
  const panes = [pane(1, { ...a, wca_role: 'agent' }), pane(2, { ...a, wca_role: 'card' }), pane(3, { ...b, wca_role: 'agent' }), pane(4, { ...b, wca_role: 'card' })];
  let s = snapshot(panes);
  assert.equal(classifyPair(s, 'a'.repeat(64), 'term1').kind, 'paired');
  panes[0].pane_id = 'w:p9'; s = snapshot(panes);
  assert.equal(locateTerminal(s, 'term1').pane_id, 'w:p9');
  assert.equal(classifyPair(s, 'b'.repeat(64), 'term3').kind, 'paired');
});
test('native pane labels preserve strings, accept unlabeled panes and reject malformed labels', () => {
  for (const label of ['Work card', '', '日本語 🐕']) assert.equal(snapshot([{ ...pane(1), label }]).panes[0].label, label);
  for (const value of [pane(1), { ...pane(1), label: null }]) assert.equal(snapshot([value]).panes[0].label, undefined);
  for (const label of [7, false, {}, []]) assert.throws(() => snapshot([{ ...pane(1), label }]), /label/i);
});
test('duplicate terminals and contradictory partial ownership fail closed', () => {
  assert.throws(() => snapshot([pane(1), { ...pane(2), terminal_id: 'term1' }]), /duplicate/i);
  assert.throws(() => classifyPair(snapshot([pane(1, { wca_agent: 'a'.repeat(64) })]), 'a'.repeat(64), 'term1'), /incomplete/i);
});
test('missing display is distinct from positively verified separated pair', () => {
  const m = pairMarkers('a'.repeat(64), 'nonce', 'term1', 'term2');
  const a = pane(1, { ...m, wca_role: 'agent' });
  assert.equal(classifyPair(snapshot([a]), 'a'.repeat(64), 'term1').kind, 'absent');
  const c = { ...pane(2, { ...m, wca_role: 'card' }), tab_id: 'w:t2' };
  assert.equal(classifyPair(snapshot([a,c]), 'a'.repeat(64), 'term1').kind, 'separated');
});
