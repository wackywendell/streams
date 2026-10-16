import test from 'node:test';
import assert from 'node:assert/strict';
import stringWidth from 'string-width';
import { mkdtemp, readdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { RecordStore, validateAgent, validateWorkspace, evaluatedGoal, unevaluatedGoal, conversationGoal } from '../dist/records.js';
import { WorkspaceCardView, renderWorkspaceCard } from '../dist/herdr/workspace-view.js';

const unstyled = text => text.replace(/\x1b\[[\d;]*m/g, '').replace(/\x1b\]8;;[^\x1b]*\x1b\\/g, '');
function fixture() {
  const workspace = { path: '/state/workspace.yaml', revision: 'w1', record: {
    version: 1, id: 'shared', creationToken: 'creation', mutationToken: 'mutation', title: 'Shared workspace', titleMode: 'auto',
    goal: evaluatedGoal('Ship library | client', [{ checkout: 'lib', path: 'docs/design.md' }], 'plan'),
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
  // Keep RecordStore's real paired-read checks; fake only the validated storage boundary.
  store.readAgent = async id => { assert.equal(id, agent.record.sessionId); return { ...agent, record: validateAgent(agent.record) }; };
  store.readWorkspace = async id => { assert.equal(id, workspace.record.id); return { ...workspace, record: validateWorkspace(workspace.record) }; };
  return { workspace, agent, store, view: new WorkspaceCardView(store, 'session') };
}

test('two agents share Goal and references while retaining own title, status, Now and Remaining', async () => {
  const { workspace, agent, view } = fixture(); await view.refresh();
  const other = structuredClone(agent);
  Object.assign(other.record, { sessionId: 'other', title: 'Other assignment', status: 'blocked', now: 'Other stage', remaining: 'Other work' });
  const first = unstyled(view.render(100, 10));
  const second = unstyled(renderWorkspaceCard({ workspace, agent: other }, 100, 10).text);
  for (const text of [first, second]) {
    assert.equal(text.split('\n')[0], 'Ship library | client');
    assert.match(text, /\[a\] docs\/design.md/);
    assert.doesNotMatch(text, /Goal:|Now:/);
    assert.equal((text.match(/Remaining:/g) ?? []).length, 1);
    assert.doesNotMatch(text, /Shared workspace/);
  }
  assert.equal(first.split('\n')[1], '');
  assert.match(first.split('\n')[2], /^Own assignment: Implement \| verify +● active$/);
  assert.equal(first.split('\n')[3], 'Remaining: 界😀é | release');
  assert.equal(first.split('\n')[4], '');
  assert.match(second.split('\n')[2], /^Other assignment: Other stage +! blocked$/);
  const styled = view.render(100, 10);
  assert.ok(styled.startsWith('\x1b[2mShip library | client\x1b[0m\n'));
  assert.ok(styled.includes('\x1b[1mOwn assignment:\x1b[0m Implement | verify'));
  assert.ok(styled.includes('\x1b[1mRemaining:\x1b[0m 界😀é | release'));
  assert.ok(styled.includes('\x1b[36m[a]\x1b[0m \x1b[2mdocs/\x1b[0mdesign.md'));
  assert.doesNotMatch(styled, /\x1b\[3m|\x1b\[1mdesign\.md/);
  assert.doesNotMatch(styled, /\*/);
  assert.doesNotMatch(second, /Implement|release/);
});

for (const goal of [unevaluatedGoal(), evaluatedGoal('', [{ checkout: 'lib', path: 'docs/design.md' }], 'bytes'), conversationGoal('', { sessionId: 's', digest: 'a'.repeat(64) })]) test(`empty ${goal.kind} shared sections omit bodies and dangling colons without losing own status`, () => {
  const { workspace, agent } = fixture();
  workspace.record.goal = goal; agent.record.now = ''; agent.record.remaining = '';
  const rendered = renderWorkspaceCard({ workspace, agent }, 100, 8);
  const lines = unstyled(rendered.text).split('\n');
  assert.match(lines[0], /^Own assignment +● active$/);
  assert.equal(lines[1], '');
  assert.equal(lines[2], '[a] docs/design.md');
  assert.doesNotMatch(rendered.text, /Remaining|Goal:|Now:|assignment:|workspace:/);
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

test('later grid shortcuts open exact opaque targets and fail closed after resize, notice and fresh edits', async () => {
  const { workspace, agent, view } = fixture();
  workspace.record.goal = unevaluatedGoal(); workspace.record.issues = [];
  agent.record.title = 'Own'; agent.record.now = ''; agent.record.remaining = '';
  workspace.record.documents = ['a.md', 'b.md', 'c.md'].map(path => ({ checkout: 'lib', path, description: '' }));
  await view.refresh();
  const wide = unstyled(view.render(150, 4));
  assert.ok(wide.split('\n').some(line => line.includes('[a]') && line.includes('[b]') && line.includes('[c]')));
  for (const key of 'bc') assert.deepEqual((await view.key(key)).identity, { checkout: 'lib', path: key + '.md' });
  view.render(28, 2);
  assert.deepEqual(await view.key('c'), { kind: 'none' });
  assert.deepEqual((await view.key('b')).identity, { checkout: 'lib', path: 'b.md' });
  view.setNotice('projection', 'Notice'); view.render(28, 2);
  assert.deepEqual(await view.key('b'), { kind: 'none' });
  assert.equal(view.notice.source, 'projection');
  view.clearNotice(); view.render(150, 4);
  workspace.record.documents[1].path = 'replacement.md';
  assert.deepEqual(await view.key('b'), { kind: 'none' });
  assert.match(view.notice.text, /changed|redraw/i);
});

test('a fresh grid reflow hiding a previously painted later-column key fails closed', async () => {
  const { workspace, agent, view } = fixture();
  workspace.record.goal = unevaluatedGoal(); workspace.record.issues = [];
  agent.record.title = 'Own'; agent.record.now = ''; agent.record.remaining = '';
  workspace.record.documents = ['a.md', 'b.md', 'c.md'].map(path => ({ checkout: 'lib', path, description: '' }));
  await view.refresh(); view.render(30, 3);
  workspace.record.documents[0].path = 'a-name-too-long-to-share-a-row.md';
  assert.deepEqual(await view.key('c'), { kind: 'none' });
  assert.match(view.notice.text, /changed|redraw/i);
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
  workspace.record.title = '界😀é shared'; agent.record.title = 'é界😀 own';
  for (let rows = 0; rows <= 9; rows++) for (let columns = 0; columns <= 30; columns++) {
    for (const notice of [undefined, 'Notice 界😀é']) {
      const rendered = renderWorkspaceCard({ workspace, agent }, columns, rows, notice);
      const text = unstyled(rendered.text), lines = text.split('\n');
      assert.equal(rendered.text, Buffer.from(rendered.text).toString('utf8'));
      assert.ok(!text || lines.length <= rows); assert.ok(lines.every(line => stringWidth(line) <= columns));
      const painted = lines.some(line => line.includes('[a]'));
      assert.equal(rendered.documents.has('a'), painted);
      if (painted) assert.deepEqual(rendered.documents.get('a'), {
        workspaceId: 'shared', membershipToken: 'membership', checkout: 'lib', path: 'docs/design.md', directory: '/code/library',
      });
      if (rows && columns >= 3) assert.ok(lines.at(-1).endsWith(columns >= 9 ? '[q] close' : '[q]'));
    }
  }
});

for (const [name, change] of [
  ['renumbered key', ({ workspace }) => { workspace.record.documents.unshift({ checkout: 'lib', path: 'new.md', description: '' }); }],
  ['path change', ({ workspace }) => { workspace.record.documents[0].path = 'new.md'; workspace.record.goal = evaluatedGoal(workspace.record.goal.text, [{ checkout: 'lib', path: 'new.md' }], 'plan'); }],
  ['root change', ({ workspace, agent }) => { workspace.record.checkouts.lib.directory = '/new-root'; agent.record.cwd = '/new-root'; }],
  ['membership token change', ({ agent }) => { agent.record.membershipToken = 'new-membership'; }],
  ['checkout key change', ({ workspace }) => { workspace.record.checkouts.other = { directory: '/other' }; workspace.record.documents[0].checkout = 'other'; workspace.record.goal = evaluatedGoal(workspace.record.goal.text, [{ checkout: 'other', path: 'docs/design.md' }], 'plan'); }],
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

for (const field of ['workspaceId', 'membershipToken', 'checkout', 'cwd']) test(`paired read fails visibly if ${field} changes during workspace read`, async () => {
  const { store, view, workspace, agent } = fixture();
  store.readWorkspace = async () => { agent.record[field] = field === 'cwd' ? '/code/library/changed' : 'changed'; return { ...workspace, record: validateWorkspace(workspace.record) }; };
  assert.equal(await view.refresh(), undefined); assert.equal(view.current, undefined);
  assert.match(unstyled(view.render(120, 6)), /unavailable.*membership/i);
  assert.deepEqual(await view.key('a'), { kind: 'none' });
});

for (const change of ['progress', 'evidence']) test(`display paired read accepts ${change} changes and returns the latest agent`, async () => {
  const { store, view, workspace, agent } = fixture();
  store.readWorkspace = async () => {
    agent.revision = 'latest';
    if (change === 'progress') agent.record.now = 'Latest progress';
    else agent.record.evidenceGeneration = 'reset';
    return { ...workspace, record: validateWorkspace(workspace.record) };
  };
  const current = await view.refresh();
  assert.deepEqual(current.agent, agent);
  assert.equal(view.notice, undefined);
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

test('notices clear by source, survive unrelated polls, and clear only on visible document attempts', async () => {
  const { view } = fixture(); await view.refresh(); view.render(80, 10);
  view.setNotice('projection', 'Arbitrary wording');
  view.clearNotice('document'); assert.equal(view.notice.source, 'projection');
  view.clearNotice('projection'); assert.equal(view.notice, undefined);
  for (const source of ['document', 'projection']) {
    view.setNotice(source, 'Card unavailable (not a read error)');
    await view.refresh(); assert.equal(view.notice.source, source);
  }
  view.setNotice('document', 'Herdr projection unavailable: document error');
  view.clearNotice('projection'); assert.equal(view.notice.source, 'document');
  await view.key('z'); assert.equal(view.notice.source, 'document');
  await view.key('q'); assert.equal(view.notice.source, 'document');
  assert.equal((await view.key('a')).kind, 'open-document'); assert.equal(view.notice, undefined);
});

test('read failure disables stale actions and a repaired record clears only its read notice', async () => {
  const { view, store, agent } = fixture(); await view.refresh(); view.render(80, 10);
  store.readAgent = async () => { throw new Error('Malformed record'); };
  assert.deepEqual(await view.key('a'), { kind: 'none' });
  assert.equal(view.current, undefined); assert.equal(view.notice.source, 'card');
  assert.match(unstyled(view.render(80, 10)), /unavailable/i);
  store.readAgent = async () => structuredClone(agent);
  await view.refresh(); assert.equal(view.notice, undefined);
  view.render(80, 10); assert.equal((await view.key('a')).kind, 'open-document');
});

test('j/k are document intents, digits inert, and q or Ctrl-C exit only the view', async () => {
  const { view, workspace } = fixture();
  workspace.record.documents = Array.from({ length: 11 }, (_, i) => ({ checkout: 'lib', path: `doc-${i}.md`, description: '' }));
  workspace.record.goal = unevaluatedGoal();
  await view.refresh(); view.render(80, 20);
  for (const digit of '0123456789') assert.deepEqual(await view.key(digit), { kind: 'none' });
  for (const [key, path] of [['j', 'doc-9.md'], ['k', 'doc-10.md']]) {
    assert.deepEqual((await view.key(key)).identity, { checkout: 'lib', path });
  }
  assert.deepEqual(await view.key('q'), { kind: 'exit' });
  assert.deepEqual(await view.key('\x03'), { kind: 'exit' });
});

test('broken files stay visible and actionable without file checks', async () => {
  const { view } = fixture(); await view.refresh(); assert.match(unstyled(view.render(100, 10)), /docs\/design.md/);
  assert.equal((await view.key('a')).kind, 'open-document');
});

test('footer-covered and notice-reduced shortcut layouts fail closed on resize and fresh reads', async () => {
  const { view, workspace, agent } = fixture(); workspace.record.issues = [];
  workspace.record.goal = unevaluatedGoal(); agent.record.now = ''; agent.record.remaining = ''; agent.record.title = 'Own';
  await view.refresh();
  assert.ok(!unstyled(view.render(11, 2)).includes('[a]'));
  assert.deepEqual(await view.key('a'), { kind: 'none' });
  assert.ok(unstyled(view.render(12, 2)).includes('[a]'));
  assert.deepEqual((await view.key('a')).identity, { checkout: 'lib', path: 'docs/design.md' });
  view.setNotice('projection', 'Notice'); assert.match(unstyled(view.render(30, 3)), /\[a\]/);
  workspace.record.documents.push({ checkout: 'lib', path: 'another-long-document.md', description: '' });
  assert.deepEqual(await view.key('a'), { kind: 'none' });
  assert.ok(!unstyled(view.render(30, 3)).includes('[a]'));
  assert.deepEqual(await view.key('a'), { kind: 'none' });
});
