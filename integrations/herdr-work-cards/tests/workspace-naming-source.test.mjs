import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { SessionManager } from '@earendil-works/pi-coding-agent';
import { acceptWorkspaceTitle, acceptedWorkspaceTitle, workspaceNamingEntry } from '../dist/pi/workspace-naming.js';
import { validateWorkspace, conversationGoal } from '../dist/records.js';
const receipt = 'a'.repeat(64);
function fixture(manager = SessionManager.inMemory()) {
  const workspace = { revision: 'revision', record: validateWorkspace({ version: 1, id: 'w', creationToken: 'creation', mutationToken: 'mutation', title: 'Grounded purpose', titleMode: 'auto', goal: conversationGoal('Grounded purpose', { sessionId: manager.getSessionId(), digest: 'b'.repeat(64) }, 'Purpose'), checkouts: {}, documents: [], issues: [], prs: [], exclusions: [] }) };
  const pi = { appendEntry: (type, data) => manager.appendCustomEntry(type, data) };
  return { manager, workspace, accept: (guard = () => {}) => acceptWorkspaceTitle(pi, manager, manager.getSessionId(), workspace, receipt, guard), read: () => acceptedWorkspaceTitle(manager, manager.getSessionId(), workspace) };
}
test('workspace proof persists exact lifetime/title/receipt without duplicate writes and survives same UUID navigation', () => {
  const h = fixture(); assert.equal(h.read(), null); h.accept();
  const count = h.manager.getEntries().length; h.accept(); assert.equal(h.manager.getEntries().length, count);
  assert.deepEqual(h.read(), { title: 'Grounded purpose', pendingRevision: receipt });
  h.manager.resetLeaf(); assert.deepEqual(h.read(), { title: 'Grounded purpose', pendingRevision: receipt });
  for (const [key, value] of [['id', 'other'], ['creationToken', 'new'], ['title', 'Changed']]) {
    const previous = h.workspace.record[key]; h.workspace.record[key] = value; assert.equal(h.read(), null); h.workspace.record[key] = previous;
  }
  h.workspace.record.goal = { kind: 'unevaluated' }; assert.equal(h.read(), null);
});
test('persisted resume retains proof but copied fork provenance grants none', async t => {
  const dir = await mkdtemp(join(tmpdir(), 'wc-workspace-source-')); t.after(() => rm(dir, { recursive: true, force: true }));
  const manager = SessionManager.create(dir, dir); manager.appendMessage({ role: 'user', content: 'Purpose', timestamp: Date.now() });
  const h = fixture(manager); h.accept();
  const resumed = SessionManager.open(manager.getSessionFile());
  assert.deepEqual(acceptedWorkspaceTitle(resumed, resumed.getSessionId(), h.workspace), h.read());
  resumed.createBranchedSession(resumed.getLeafId()); assert.equal(acceptedWorkspaceTitle(resumed, resumed.getSessionId(), h.workspace), null);
});
test('latest malformed entry never falls back and latest foreign entry revokes old proof', () => {
  const h = fixture(); h.accept(); const proof = h.manager.getEntries().at(-1).data;
  h.manager.appendCustomEntry(workspaceNamingEntry, { ...proof, sessionId: 'foreign' }); assert.equal(h.read(), null);
  for (const data of [null, {}, { ...proof, pendingRevision: 'bad' }, { ...proof, title: 'unsafe\nname' }, { ...proof, extra: true }]) {
    h.manager.appendCustomEntry(workspaceNamingEntry, data);
    assert.throws(h.read, /malformed.*workspace.*naming/i); assert.throws(() => h.accept(), /malformed.*workspace.*naming/i);
  }
});
test('acceptance requires a live UUID, known purpose and valid receipt before persisting', () => {
  const h = fixture(); assert.throws(() => h.accept(() => { throw new Error('stale'); }), /stale/); assert.equal(h.read(), null);
  h.workspace.record.goal = { kind: 'unevaluated' }; assert.throws(() => h.accept(), /known/i);
  assert.equal(h.manager.getEntries().length, 0);
});
