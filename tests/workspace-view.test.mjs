import test from 'node:test';
import assert from 'node:assert/strict';
import stringWidth from 'string-width';
import { mkdtemp, readdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { RecordStore } from '../dist/records.js';
import { WorkspaceCardView, renderWorkspaceCard } from '../dist/herdr/workspace-view.js';

const unstyled = text => text.replace(/\x1b\[[\d;]*m/g, '').replace(/\x1b\]8;;[^\x1b]*\x1b\\/g, '');
function fixture() {
  const workspace = { path: '/state/workspace.yaml', revision: 'w1', record: {
    version: 1, id: 'shared', creationToken: 'creation', mutationToken: 'mutation', title: 'Shared workspace', titleMode: 'auto',
    goal: 'Ship library | client', planSources: [{ checkout: 'lib', path: 'docs/design.md' }], sourceFingerprint: 'plan',
    checkouts: { lib: { directory: '/code/library' } },
    documents: [{ checkout: 'lib', path: 'docs/design.md', description: 'Design' }],
    issues: [{ url: 'https://example.com/1', description: 'Issue' }], prs: [], exclusions: [],
  } };
  const agent = { path: '/state/agent.yaml', revision: 'a1', record: {
    version: 1, workspaceId: 'shared', sessionId: 'session', membershipToken: 'membership', evidenceGeneration: 'evidence',
    boundary: { compactionDigest: 'compact', transcriptLength: 0, prefixDigest: 'prefix' },
    checkout: 'lib', cwd: '/code/library/subdir', title: 'Own assignment', titleMode: 'auto', status: 'active',
    now: 'Implement | verify', remaining: '界😀é | release',
  } };
  const store = new RecordStore('/never-written');
  store.readAgent = async id => { assert.equal(id, agent.record.sessionId); return structuredClone(agent); };
  store.readWorkspace = async id => { assert.equal(id, workspace.record.id); return structuredClone(workspace); };
  return { workspace, agent, store, view: new WorkspaceCardView(store, 'session') };
}

test('two agents share Goal and references while retaining own title, status, Now and Remaining', async () => {
  const { workspace, agent, view } = fixture(); await view.refresh();
  const other = structuredClone(agent);
  Object.assign(other.record, { sessionId: 'other', title: 'Other assignment', status: 'blocked', now: 'Other stage', remaining: 'Other work' });
  const first = unstyled(view.render(100, 10));
  const second = unstyled(renderWorkspaceCard({ workspace, agent: other }, 100, 10).text);
  for (const text of [first, second]) { assert.match(text, /Goal: Ship library \| client/); assert.match(text, /\[a\] docs\/design.md/); }
  assert.match(first, /Own assignment.*● active/); assert.match(first, /Now: Implement \| verify/);
  assert.match(first, /Remaining: 界😀é \| release/);
  assert.match(second, /Other assignment.*! blocked/); assert.match(second, /Now: Other stage/);
  assert.doesNotMatch(second, /Implement|release/);
});

test('multirepo same relative paths retain exact opaque identities and checkout roots', async () => {
  const { workspace, view } = fixture();
  workspace.record.checkouts.client = { directory: '/code/client' };
  workspace.record.documents.push({ checkout: 'client', path: 'docs/design.md', description: '' });
  await view.refresh(); const text = unstyled(view.render(100, 10));
  assert.match(text, /\[a\] library\/docs\/design.md/); assert.match(text, /\[b\] client\/docs\/design.md/);
  const opened = await view.key('b');
  assert.equal(opened.kind, 'open-document');
  assert.deepEqual(opened.identity, { checkout: 'client', path: 'docs/design.md' });
  assert.deepEqual(opened.workspace, view.current.workspace); assert.deepEqual(opened.agent, view.current.agent);
  const target = renderWorkspaceCard(view.current, 100, 10).documents.get('b');
  assert.deepEqual(target, { workspaceId: 'shared', membershipToken: 'membership', checkout: 'client', path: 'docs/design.md', directory: '/code/client' });
});

test('basename, ancestor and root collisions use workspace-wide distinguishing labels', () => {
  const { workspace, agent } = fixture();
  workspace.record.checkouts = { lib: { directory: '/backend/common' }, client: { directory: '/frontend/common' }, root: { directory: '/' }, ancestor: { directory: '/backend' } };
  agent.record.cwd = '/backend/common';
  workspace.record.documents = Object.keys(workspace.record.checkouts).map(checkout => ({ checkout, path: 'docs/design.md', description: '' }));
  const rendered = renderWorkspaceCard({ workspace, agent }, 100, 12);
  const text = unstyled(rendered.text);
  for (const label of ['backend/common/docs/design.md', 'frontend/common/docs/design.md', '/docs/design.md', 'backend/docs/design.md']) assert.ok(text.includes(label), label);
  assert.equal(new Set([...rendered.documents.values()].map(d => d.directory)).size, 4);
});

test('structured literal pipes and Unicode remain bounded in tiny panes with notices', () => {
  const { workspace, agent } = fixture();
  for (let rows = 0; rows <= 9; rows++) for (let columns = 0; columns <= 30; columns++) {
    for (const notice of [undefined, 'Notice 界😀é']) {
      const rendered = renderWorkspaceCard({ workspace, agent }, columns, rows, notice);
      const text = unstyled(rendered.text), lines = text.split('\n');
      assert.equal(rendered.text, Buffer.from(rendered.text).toString('utf8'));
      assert.ok(!text || lines.length <= rows); assert.ok(lines.every(line => stringWidth(line) <= columns));
      for (const key of rendered.documents.keys()) assert.ok(lines.some(line => line.startsWith(`[${key}]`)));
      if (rows && columns >= 3) assert.ok(lines.at(-1).endsWith(columns >= 9 ? '[q] close' : '[q]'));
    }
  }
});

for (const [name, change] of [
  ['renumbered key', ({ workspace }) => { workspace.record.documents.unshift({ checkout: 'lib', path: 'new.md', description: '' }); }],
  ['path change', ({ workspace }) => { workspace.record.documents[0].path = 'new.md'; workspace.record.planSources[0].path = 'new.md'; }],
  ['root change', ({ workspace, agent }) => { workspace.record.checkouts.lib.directory = '/new-root'; agent.record.cwd = '/new-root'; }],
  ['membership token change', ({ agent }) => { agent.record.membershipToken = 'new-membership'; }],
  ['checkout key change', ({ workspace }) => { workspace.record.checkouts.other = { directory: '/other' }; workspace.record.documents[0].checkout = 'other'; workspace.record.planSources[0].checkout = 'other'; }],
  ['workspace switch', ({ workspace, agent }) => { workspace.record.id = 'destination'; agent.record.workspaceId = 'destination'; }],
]) test(`fresh key rejects ${name} without opening another target`, async () => {
  const f = fixture(); await f.view.refresh(); f.view.render(100, 10); change(f);
  assert.deepEqual(await f.view.key('a'), { kind: 'none' });
  assert.match(f.view.notice.text, /changed|redraw/i);
});

test('fresh key returns fresh snapshots after unrelated own/shared edits and leaves numeric links inert', async () => {
  const { view, workspace, agent } = fixture(); await view.refresh(); view.render(100, 10);
  workspace.revision = 'w2'; agent.revision = 'a2'; agent.record.now = 'Fresh stage';
  const opened = await view.key('a'); assert.equal(opened.kind, 'open-document');
  assert.equal(opened.workspace.revision, 'w2'); assert.equal(opened.agent.record.now, 'Fresh stage');
  assert.deepEqual(await view.key('1'), { kind: 'none' });
  assert.deepEqual(await view.key('q'), { kind: 'exit' }); assert.deepEqual(await view.key('\x03'), { kind: 'exit' });
});

test('background refresh cannot replace the identity behind a previously painted shortcut', async () => {
  const { view, workspace, agent } = fixture(); await view.refresh(); view.render(100, 10);
  workspace.record.checkouts.lib.directory = '/relocated'; agent.record.cwd = '/relocated';
  await view.refresh();
  assert.deepEqual(await view.key('a'), { kind: 'none' });
});

test('manual own-checkout changes invalidate a painted membership even with an unchanged token', async () => {
  const { view, workspace, agent } = fixture();
  workspace.record.checkouts.other = { directory: '/code/other' };
  await view.refresh(); view.render(100, 10);
  agent.record.checkout = 'other'; agent.record.cwd = '/code/other';
  await view.refresh();
  assert.deepEqual(await view.key('a'), { kind: 'none' });
  assert.match(view.notice.text, /changed|redraw/i);
});

for (const field of ['workspaceId', 'membershipToken', 'checkout']) test(`paired read fails visibly if ${field} changes during workspace read`, async () => {
  const { store, view, workspace, agent } = fixture();
  store.readWorkspace = async () => { agent.record[field] = 'changed'; return structuredClone(workspace); };
  assert.equal(await view.refresh(), undefined); assert.equal(view.current, undefined);
  assert.match(unstyled(view.render(120, 6)), /unavailable.*membership/i);
  assert.deepEqual(await view.key('a'), { kind: 'none' });
});

test('missing agent/workspace reads never create records or directories', async t => {
  const root = await mkdtemp(join(tmpdir(), 'workspace-view-')); t.after(() => rm(root, { recursive: true, force: true }));
  const store = new RecordStore(join(root, 'absent'));
  const view = new WorkspaceCardView(store, 'missing');
  assert.equal(await view.refresh(), undefined); assert.match(unstyled(view.render(120, 4)), /unavailable/i);
  assert.deepEqual(await readdir(root), []);
  store.readAgent = async () => fixture().agent;
  assert.equal(await view.refresh(), undefined); assert.match(unstyled(view.render(120, 4)), /unavailable/i);
  assert.deepEqual(await readdir(root), []);
});

test('broken files stay visible and actionable without file checks', async () => {
  const { view } = fixture(); await view.refresh(); assert.match(unstyled(view.render(100, 10)), /docs\/design.md/);
  assert.equal((await view.key('a')).kind, 'open-document');
});

test('footer-covered and notice-reduced shortcut layouts fail closed on resize and fresh reads', async () => {
  const { view, workspace } = fixture(); workspace.record.issues = []; await view.refresh();
  view.render(11, 5); assert.deepEqual(await view.key('a'), { kind: 'none' });
  view.render(12, 5); assert.equal((await view.key('a')).kind, 'open-document');
  view.setNotice('projection', 'Notice'); assert.match(unstyled(view.render(30, 4)), /\[a\]/);
  workspace.record.documents.push({ checkout: 'lib', path: 'other.md', description: '' });
  assert.deepEqual(await view.key('a'), { kind: 'none' });
  view.render(30, 4); assert.deepEqual(await view.key('a'), { kind: 'none' });
});
