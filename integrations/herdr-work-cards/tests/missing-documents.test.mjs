import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtemp, mkdir, writeFile, rm, realpath, rename, symlink, chmod, open } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import * as records from '../dist/records.js';
import { WorkspaceRefresher } from '../dist/pi/workspace-refresh.js';
import { PlanRefresher } from '../dist/pi/plan-refresh.js';
import { captureReferenceBoundary } from '../dist/pi/shared-evidence.js';

const options = { model: 'helpers/simple', thinking_level: null, max_output_tokens: 512, request_timeout_ms: 1000 };
async function fixture(t) {
  const base = await realpath(await mkdtemp(join(tmpdir(), 'wc-missing-')));
  t.after(() => rm(base, { recursive: true, force: true }));
  const root = join(base, 'repo'); await mkdir(root);
  const store = new records.RecordStore(join(base, 'records'));
  await store.createWorkspace({ id: 'w', creationToken: 'creation', title: 'Workspace' });
  const evidence = { compactionSummary: null, transcript: [] };
  const boundary = captureReferenceBoundary(evidence);
  const agent = await store.join({ workspaceId: 'w', sessionId: 's', cwd: root, membershipToken: 'member', boundary, title: 'Agent' });
  const authority = records.agentAuthority(agent.record);
  const doc = path => ({ checkout: agent.record.checkout, path, description: 'Workspace design' });
  const calls = [], handlers = {
    own: () => ({ action: 'no_change' }), shared: () => ({ action: 'no_change' }),
    select: data => ({ sources: data.documents.map(({ checkout, path }) => ({ checkout, path })) }),
    goal: () => ({ goal: 'Build the workspace', goalShort: 'Build workspace' }),
  };
  const registry = {
    find() { return { api: 'pi-virtual' }; },
    streamSimple(_model, context) {
      const data = JSON.parse(context.messages[0].content[0].text);
      const kind = 'agent' in data ? 'own' : 'ownCheckout' in data ? 'shared' : 'documents' in data ? 'select' : 'goal';
      calls.push({ kind, data });
      return { result: async () => ({ stopReason: 'stop', content: [{ type: 'text', text: JSON.stringify(await handlers[kind](data)) }] }) };
    },
  };
  const config = { store, registry, options, loadHelperProfiles: async () => ({ simple: { thinkingLevel: 'low' } }) };
  const refresher = new WorkspaceRefresher(config), plans = new PlanRefresher(config);
  const controller = new AbortController(); let host = async () => {};
  const input = { authority, signal: controller.signal, assertCurrent: () => host(), evidence };
  return { base, root, store, authority, doc, handlers, calls, controller, input, plans,
    host(fn) { host = fn; }, refresh: extra => refresher.refresh({ ...input, ...extra }),
    workspace: () => store.readWorkspace('w'),
    async add(...paths) { await store.addReferences(authority, { documents: paths.map(doc) }); },
    async missing(path) {
      const identity = { checkout: agent.record.checkout, path };
      const proof = await records.probeWorkspaceDocument((await store.readWorkspace('w')).record, identity);
      assert.equal(proof.kind, 'missing'); return proof;
    },
  };
}

for (const path of ['absent.md', 'absent/plan.md']) test(`probe confirms absent leaf/ancestor: ${path}`, async t => {
  const f = await fixture(t); const proof = await f.missing(path);
  assert.equal(proof.root.directory, f.root);
  assert.match(proof.root.dev, /^\d+$/); assert.match(proof.root.ino, /^\d+$/);
});

for (const kind of ['unavailable', 'relocated', 'dangling', 'outside', 'internal', 'directory', 'ancestor-file']) test(`probe retains unresolved ${kind} document`, async t => {
  const f = await fixture(t); let path = 'plan.md';
  if (kind === 'unavailable') await rename(f.root, join(f.base, 'moved'));
  if (kind === 'relocated') { await rename(f.root, join(f.base, 'moved')); await symlink(join(f.base, 'moved'), f.root); }
  if (kind === 'dangling') await symlink(join(f.root, 'missing'), join(f.root, path));
  if (kind === 'outside') { await writeFile(join(f.base, 'outside'), 'Outside'); await symlink(join(f.base, 'outside'), join(f.root, path)); }
  if (kind === 'internal') { await writeFile(join(f.root, 'inside'), 'Inside'); await symlink(join(f.root, 'inside'), join(f.root, path)); }
  if (kind === 'directory') await mkdir(join(f.root, path));
  if (kind === 'ancestor-file') { await writeFile(join(f.root, 'ancestor'), 'File'); path = 'ancestor/plan.md'; }
  const probe = await records.probeWorkspaceDocument((await f.workspace()).record, { checkout: f.doc(path).checkout, path });
  assert.equal(probe.kind, 'unresolved');
  await f.add(path); f.handlers.select = () => ({ sources: [] });
  const events = []; const result = await f.refresh({ onDiagnostic: e => events.push(e) });
  assert.deepEqual(result.workspace.record.documents, [f.doc(path)]);
  assert.ok(events.some(e => e.outcome === 'documents_checked' && Object.values(e.unresolvedCounts).reduce((a, b) => a + b, 0) === 1));
  assert.doesNotMatch(JSON.stringify(events), /plan\.md|outside|Inside/);
});

for (const errorCode of ['EACCES', 'EIO']) test(`probe classifies ${errorCode} as unresolved, never missing`, async t => {
  const f = await fixture(t); await writeFile(join(f.root, 'plan.md'), 'Purpose');
  const workspace = (await f.workspace()).record;
  const handle = await open(join(f.root, 'plan.md')); const prototype = Object.getPrototypeOf(handle); await handle.close();
  t.mock.method(prototype, 'stat', async () => { throw Object.assign(new Error('Injected IO'), { code: errorCode }); });
  const probe = await records.probeWorkspaceDocument(workspace, { checkout: f.doc('plan.md').checkout, path: 'plan.md' });
  assert.equal(probe.kind, 'unresolved'); assert.equal(probe.reason, errorCode === 'EACCES' ? 'permission' : 'io');
});

test('probe rejects a root replaced during file verification', async t => {
  const f = await fixture(t); await writeFile(join(f.root, 'plan.md'), 'Purpose'); const workspace = (await f.workspace()).record;
  const handle = await open(join(f.root, 'plan.md')); const identity = await handle.stat({ bigint: true });
  const prototype = Object.getPrototypeOf(handle), original = prototype.stat; await handle.close(); let changed = false;
  t.mock.method(prototype, 'stat', async function (...args) {
    const info = await original.apply(this, args);
    if (!changed && info.ino === identity.ino) {
      changed = true; await rename(f.root, join(f.base, 'moved')); await mkdir(f.root); await writeFile(join(f.root, 'plan.md'), 'Other checkout');
    }
    return info;
  });
  const probe = await records.probeWorkspaceDocument(workspace, { checkout: f.doc('plan.md').checkout, path: 'plan.md' });
  assert.equal(probe.kind, 'unresolved'); assert.equal(probe.reason, 'changed');
});

test('real permission denial does not become missing authority', async t => {
  const f = await fixture(t); const path = join(f.root, 'locked'); await mkdir(path); await chmod(path, 0);
  try {
    const probe = await records.probeWorkspaceDocument((await f.workspace()).record, { checkout: f.doc('locked/plan.md').checkout, path: 'locked/plan.md' });
    assert.equal(probe.kind, 'unresolved'); assert.equal(probe.reason, 'permission');
  } finally { await chmod(path, 0o700); }
});

test('repair rechecks restoration and leaves exclusions unchanged', async t => {
  const f = await fixture(t); await f.add('old.md'); const baseline = await f.workspace(), proof = await f.missing('old.md');
  await writeFile(join(f.root, 'old.md'), 'Restored');
  const result = await f.store.repairMissingDocuments(f.authority, baseline, [proof], f.input);
  assert.equal(result.removed, 0); assert.equal(result.revision, baseline.revision);
  assert.deepEqual(result.record.documents, [f.doc('old.md')]); assert.deepEqual(result.record.exclusions, []);
});

for (const change of ['description', 'root', 'aba', 'creation']) test(`repair rejects stale ${change} snapshot`, async t => {
  const f = await fixture(t); await f.add('old.md'); const baseline = await f.workspace(), proof = await f.missing('old.md');
  if (change === 'description') await f.store.describeReference(f.authority, { checkout: f.doc('old.md').checkout, path: 'old.md' }, 'Foreground description', baseline.revision);
  if (change === 'aba') {
    await f.store.removeReference(f.authority, { workspaceId: 'w', revision: baseline.revision, identity: records.referenceIdentity(f.doc('old.md')) });
    await f.store.addReferences(f.authority, { documents: [f.doc('old.md')] }, { explicit: true });
  }
  if (change === 'root' || change === 'creation') {
    const record = structuredClone(baseline.record);
    if (change === 'creation') record.creationToken = 'new-creation';
    else record.checkouts[f.doc('old.md').checkout].directory = join(f.base, 'new-root');
    await writeFile(baseline.path, JSON.stringify(records.serializeWorkspaceV1(record)));
  }
  const winner = await f.workspace();
  await assert.rejects(f.store.repairMissingDocuments(f.authority, baseline, [proof], f.input), /Stale/i);
  assert.equal((await f.workspace()).revision, winner.revision);
});

test('a different checkout at the same path cannot inherit missing-proof authority', async t => {
  const f = await fixture(t); await f.add('old.md'); const baseline = await f.workspace(), proof = await f.missing('old.md');
  await rename(f.root, join(f.base, 'moved')); await mkdir(f.root);
  await assert.rejects(f.store.repairMissingDocuments(f.authority, baseline, [proof], f.input), /Checkout identity changed/i);
  assert.equal((await f.workspace()).revision, baseline.revision);
});

for (const invalidation of ['cancel', 'host', 'membership', 'evidence']) test(`repair refuses ${invalidation} invalidation`, async t => {
  const f = await fixture(t); await f.add('old.md'); const baseline = await f.workspace(), proof = await f.missing('old.md');
  if (invalidation === 'cancel') f.controller.abort(new Error('Cancelled'));
  if (invalidation === 'host') f.host(async () => { throw new Error('Host invalidated'); });
  if (invalidation === 'membership' || invalidation === 'evidence') {
    const a = await f.store.readAgent('s');
    if (invalidation === 'membership') await writeFile(a.path, JSON.stringify({ ...a.record, membershipToken: 'different' }));
    else await f.store.resetEvidence(f.authority, a.record.boundary, a.revision);
  }
  await assert.rejects(f.store.repairMissingDocuments(f.authority, baseline, [proof], f.input));
  assert.equal((await f.workspace()).revision, baseline.revision);
});

test('repair snapshots caller-owned proofs, baseline and authority before awaiting', async t => {
  const f = await fixture(t); await f.add('old.md'); const baseline = await f.workspace(), proof = await f.missing('old.md');
  const authority = { ...f.authority };
  const pending = f.store.repairMissingDocuments(authority, baseline, [proof], f.input);
  authority.workspaceId = 'other'; baseline.revision = 'changed'; baseline.record.documents = []; proof.identity.path = 'different.md'; proof.root.ino = '0';
  const result = await pending;
  assert.equal(result.removed, 1); assert.deepEqual(result.record.documents, []);
});

test('refresh removes missing documents without evidence or permanent exclusions', async t => {
  const f = await fixture(t); await f.add('old.md');
  const result = await f.refresh();
  assert.deepEqual(result.workspace.record.documents, []);
  assert.deepEqual(result.workspace.record.exclusions, []);
  assert.deepEqual(f.calls.map(c => c.kind), ['own']);
});

test('automatic additions cannot resurrect missing paths from historical evidence', async t => {
  const f = await fixture(t); await f.add('old.md');
  f.handlers.shared = () => ({ action: 'update', additions: { documents: [f.doc('old.md')] } });
  const referenceEvidence = { compactionSummary: 'Old plan old.md', transcript: [] };
  for (let i = 0; i < 2; i++) {
    const result = await f.refresh({ referenceEvidence });
    assert.deepEqual(result.workspace.record.documents, []);
    assert.deepEqual(result.workspace.record.exclusions, []);
  }
  await writeFile(join(f.root, 'old.md'), 'Restored useful purpose');
  assert.deepEqual((await f.refresh({ referenceEvidence })).workspace.record.documents, [f.doc('old.md')]);
});

test('cleanup invalidates a prior selected-source Goal before shared inference, then derives from survivors', async t => {
  const f = await fixture(t); await writeFile(join(f.root, 'one.md'), 'First'); await writeFile(join(f.root, 'two.md'), 'Second');
  await f.add('one.md', 'two.md'); await f.refresh(); await rm(join(f.root, 'one.md'));
  f.handlers.shared = data => {
    assert.equal(data.workspace.goal, ''); assert.deepEqual(data.references.documents, [f.doc('two.md')]);
    return { action: 'no_change' };
  };
  const result = await f.refresh({ referenceEvidence: { compactionSummary: 'Current work', transcript: [] } });
  assert.deepEqual(result.workspace.record.goal.provenance.sources.map(s => s.path), ['two.md']);
});

test('cleanup failures diagnose the shared stage rather than own summary failure', async t => {
  const f = await fixture(t); await f.add('old.md'); const original = new Error('Repair conflict'), events = [];
  f.store.repairMissingDocuments = async () => { throw original; };
  await assert.rejects(f.refresh({ onDiagnostic: e => events.push(e) }), error => error === original);
  assert.deepEqual(events, [{ stage: 'own', outcome: 'unchanged' }, { stage: 'shared', outcome: 'failed', cancelled: false }, { stage: 'goal', outcome: 'skipped' }]);
});

for (const intent of ['automatic', 'reset']) test(`${intent} discovers a renamed/consolidated replacement from projected evidence`, async t => {
  const f = await fixture(t); await f.add('old-design.md', 'old-plan.md');
  await writeFile(join(f.root, 'design.md'), 'Consolidated design purpose');
  f.handlers.own = () => intent === 'reset' ? { action: 'update', patch: { title: 'Assignment', status: 'active', now: '', remaining: '', statusShort: '' } } : { action: 'no_change' };
  const referenceEvidence = { compactionSummary: null, transcript: [
    { role: 'assistant', text: 'Consolidated old-design.md and old-plan.md into design.md; this is the current workspace design.' },
    { role: 'tool_activity', operation: 'write', path: 'design.md' },
  ] };
  f.handlers.shared = data => {
    assert.deepEqual(data.transcript, referenceEvidence.transcript);
    if (intent === 'reset') { assert.equal(data.references, undefined); assert.equal(data.workspace, undefined); }
    else assert.deepEqual(data.references.documents, []);
    return { action: 'update', additions: { documents: [f.doc('design.md'), f.doc('old-plan.md')] } };
  };
  const result = await f.refresh({ intent, referenceEvidence });
  assert.deepEqual(result.workspace.record.documents, [f.doc('design.md')]);
  assert.deepEqual(result.workspace.record.exclusions, []);
  assert.equal(result.workspace.record.goal.kind, 'known');
});

test('consolidation into an already registered document needs no replacement proposal', async t => {
  const f = await fixture(t); await writeFile(join(f.root, 'design.md'), 'Purpose'); await f.add('old.md', 'design.md');
  const result = await f.refresh();
  assert.deepEqual(result.workspace.record.documents, [f.doc('design.md')]);
  assert.equal(result.workspace.record.goal.kind, 'known');
});

test('foreground exclusions win automatic admission; explicit broken additions remain supported', async t => {
  const f = await fixture(t); await writeFile(join(f.root, 'design.md'), 'Purpose'); await f.add('design.md');
  const before = await f.workspace();
  await f.store.removeReference(f.authority, { workspaceId: 'w', revision: before.revision, identity: records.referenceIdentity(f.doc('design.md')) });
  const result = await f.store.updateSharedReferences(f.authority, await f.workspace(), { additions: { documents: [f.doc('design.md'), f.doc('missing.md')] } }, f.input.signal, f.input.assertCurrent);
  assert.deepEqual(result.record.documents, []); assert.equal(result.record.exclusions.length, 1);
  await f.store.addReferences(f.authority, { documents: [f.doc('missing.md')] }, { explicit: true });
  assert.deepEqual((await f.workspace()).record.documents, [f.doc('missing.md')]);
});

test('automatic admission rejects a file disappearing during its second usability probe', async t => {
  const f = await fixture(t); const path = join(f.root, 'design.md'); await writeFile(path, 'Purpose');
  const handle = await open(path); const identity = await handle.stat({ bigint: true });
  const prototype = Object.getPrototypeOf(handle), original = prototype.stat; await handle.close();
  let checks = 0;
  t.mock.method(prototype, 'stat', async function (...args) {
    const info = await original.apply(this, args);
    if (info.ino === identity.ino && ++checks === 2) await rm(path);
    return info;
  });
  const result = await f.store.updateSharedReferences(f.authority, await f.workspace(), { additions: { documents: [f.doc('design.md')] } }, f.input.signal, f.input.assertCurrent);
  assert.equal(checks, 2); assert.deepEqual(result.record.documents, []); assert.equal(result.omitted[0].kind, 'unresolved');
});

test('automatic admission snapshots caller-owned proposals and authority before awaiting', async t => {
  const f = await fixture(t); await writeFile(join(f.root, 'design.md'), 'Purpose');
  const authority = { ...f.authority }, baseline = await f.workspace(), proposal = { additions: { documents: [f.doc('design.md')] } };
  const pending = f.store.updateSharedReferences(authority, baseline, proposal, f.input.signal, f.input.assertCurrent);
  authority.workspaceId = 'other'; baseline.record.creationToken = 'different'; proposal.additions.documents[0].path = 'injected.md';
  const result = await pending;
  assert.deepEqual(result.record.documents, [f.doc('design.md')]);
});

test('unresolved additions are omitted while safe documents, issues and PRs still merge', async t => {
  const f = await fixture(t); await writeFile(join(f.root, 'design.md'), 'Purpose'); await symlink(join(f.base, 'absent'), join(f.root, 'unsafe.md'));
  const issue = { url: 'https://example.com/issue', description: 'Issue' }, pr = { url: 'https://example.com/pr', description: 'PR' };
  const result = await f.store.updateSharedReferences(f.authority, await f.workspace(), { additions: { documents: [f.doc('unsafe.md'), f.doc('absent.md'), f.doc('design.md')], issues: [issue], prs: [pr] } }, f.input.signal, f.input.assertCurrent);
  assert.deepEqual(result.record.documents, [f.doc('design.md')]); assert.deepEqual(result.record.issues, [issue]); assert.deepEqual(result.record.prs, [pr]);
  assert.deepEqual(result.omitted.map(p => p.kind), ['unresolved', 'missing']);
});

for (const invalidation of ['cancel', 'host']) test(`automatic admission has a final ${invalidation} guard`, async t => {
  const f = await fixture(t); await writeFile(join(f.root, 'design.md'), 'Purpose'); const baseline = await f.workspace();
  const guard = async () => { if (invalidation === 'cancel') f.controller.abort(new Error('Cancelled')); else throw new Error('Host invalidated'); };
  await assert.rejects(f.store.updateSharedReferences(f.authority, baseline, { additions: { documents: [f.doc('design.md')] } }, f.input.signal, guard));
  assert.equal((await f.workspace()).revision, baseline.revision);
});

for (const operation of ['repair', 'admission']) test(`${operation} checks the host again after temporary metadata sync`, async t => {
  const f = await fixture(t); await f.add('old.md'); await writeFile(join(f.root, 'design.md'), 'Purpose');
  const baseline = await f.workspace(), proof = await f.missing('old.md');
  const handle = await open(join(f.root, 'design.md')); const prototype = Object.getPrototypeOf(handle); await handle.close();
  const sync = prototype.sync; let stale = false;
  t.mock.method(prototype, 'sync', async function () { await sync.call(this); stale = true; });
  const assertCurrent = async () => { if (stale) throw new Error('Host invalidated during metadata preparation'); };
  const pending = operation === 'repair'
    ? f.store.repairMissingDocuments(f.authority, baseline, [proof], { signal: f.input.signal, assertCurrent })
    : f.store.updateSharedReferences(f.authority, baseline, { additions: { documents: [f.doc('design.md')] } }, f.input.signal, assertCurrent);
  await assert.rejects(pending, /Host invalidated/);
  assert.equal((await f.workspace()).revision, baseline.revision);
});

for (const invalidation of ['cancel', 'host', 'membership', 'revision']) test(`source loss cannot repair through ${invalidation} invalidation`, async t => {
  const f = await fixture(t); await writeFile(join(f.root, 'plan.md'), 'Purpose'); await f.add('plan.md'); await f.refresh();
  const baseline = await f.workspace();
  f.handlers.goal = async () => {
    await rm(join(f.root, 'plan.md'));
    if (invalidation === 'cancel') f.controller.abort(new Error('Cancelled'));
    if (invalidation === 'host') f.host(async () => { throw new Error('Host invalidated'); });
    if (invalidation === 'membership') {
      const a = await f.store.readAgent('s'); await writeFile(a.path, JSON.stringify({ ...a.record, membershipToken: 'different' }));
    }
    if (invalidation === 'revision') await f.store.addReferences(f.authority, { issues: [{ url: 'https://example.com/concurrent', description: 'Concurrent' }] });
    return { goal: 'Stale purpose', goalShort: 'Stale purpose' };
  };
  await assert.rejects(f.refresh({ intent: 'refresh' }));
  const result = await f.workspace();
  assert.deepEqual(result.record.documents, baseline.record.documents);
  assert.deepEqual(result.record.goal, baseline.record.goal);
});

test('selected source disappearing during Goal inference invalidates all provenance and fails without retry', async t => {
  const f = await fixture(t);
  await writeFile(join(f.root, 'one.md'), 'First purpose'); await writeFile(join(f.root, 'two.md'), 'Second purpose');
  await f.add('one.md', 'two.md'); await f.refresh();
  f.calls.length = 0;
  f.handlers.goal = async () => { await rm(join(f.root, 'one.md')); return { goal: 'Stale', goalShort: 'Stale' }; };
  await assert.rejects(f.refresh({ intent: 'refresh' }), /source|ENOENT/i);
  const after = (await f.workspace()).record;
  assert.deepEqual(after.goal, records.unevaluatedGoal());
  assert.deepEqual(after.documents, [f.doc('two.md')]);
  assert.deepEqual(after.exclusions, []);
  assert.equal(f.calls.filter(c => c.kind === 'goal').length, 1);
  f.handlers.goal = () => ({ goal: 'Surviving purpose', goalShort: 'Surviving purpose' });
  assert.equal((await f.refresh()).workspace.record.goal.text, 'Surviving purpose');
});
