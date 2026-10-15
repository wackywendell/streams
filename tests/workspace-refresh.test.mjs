import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtemp, mkdir, writeFile, rm, realpath, readFile, stat } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { stringify } from 'yaml';
import { RecordStore, referenceIdentity } from '../dist/records.js';
import { captureReferenceBoundary } from '../dist/pi/shared-evidence.js';
import { capturePlanSources } from '../dist/pi/plan-sources.js';
import { WorkspaceRefresher } from '../dist/pi/workspace-refresh.js';

const options = { model: 'helpers/simple', thinking_level: null, max_output_tokens: 512, request_timeout_ms: 1000 };
const noChange = () => ({ action: 'no_change' });
const inherited = { compactionSummary: 'Inherited checkpoint', transcript: [{ role: 'user', text: 'OLD plan.md' }] };
async function fixture(t) {
  const base = await realpath(await mkdtemp(join(tmpdir(), 'wc-workspace-refresh-')));
  t.after(() => rm(base, { recursive: true, force: true }));
  const root = join(base, 'repo'); await mkdir(root);
  await writeFile(join(root, 'plan.md'), 'Deliver a library and client');
  const store = new RecordStore(join(base, 'records'));
  await store.createWorkspace({ id: 'w', creationToken: 'creation', title: 'Workspace' });
  const boundary = captureReferenceBoundary(inherited);
  const agent = await store.join({ workspaceId: 'w', sessionId: 's', cwd: root, membershipToken: 'member', boundary, title: 'Agent' });
  const authority = { sessionId: 's', workspaceId: 'w', membershipToken: 'member', evidenceGeneration: agent.record.evidenceGeneration };
  const identity = { checkout: agent.record.checkout, path: 'plan.md' };
  const evidence = structuredClone(inherited);
  evidence.transcript.push({ role: 'assistant', text: 'NEW plan.md and https://example.com/task' });
  const calls = [], handlers = { own: noChange, shared: noChange, select: () => ({ sources: [identity] }), goal: () => ({ goal: 'Deliver library', title: 'Automatic workspace' }) };
  const registry = {
    find(provider, id) { assert.equal(provider, 'helpers'); assert.equal(id, 'simple'); return { api: 'pi-virtual' }; },
    streamSimple(_model, context, config) {
      assert.deepEqual(context.tools, []); assert.equal(config.toolChoice, 'none'); assert.equal(config.reasoning, 'low');
      assert.equal(config.maxTokens, 512); assert.equal(config.signal.aborted, false);
      const data = JSON.parse(context.messages[0].content[0].text);
      const kind = 'agent' in data ? 'own' : 'ownCheckout' in data ? 'shared' : 'documents' in data ? 'select' : 'goal';
      calls.push({ kind, data });
      return { result: async () => ({ stopReason: 'stop', content: [{ type: 'text', text: JSON.stringify(await handlers[kind](data)) }] }) };
    },
  };
  const refresher = new WorkspaceRefresher({ store, registry, options, loadHelperProfiles: async () => ({ simple: { thinkingLevel: 'low' } }) });
  const controller = new AbortController(); let host = async () => {};
  const input = { authority, evidence, signal: controller.signal, assertCurrent: () => host() };
  return { base, root, store, authority, identity, boundary, evidence, calls, handlers, controller, input, refresher,
    refresh: (extra = {}) => refresher.refresh({ ...input, ...extra }), host(fn) { host = fn; },
    agent: () => store.readAgent('s'), workspace: () => store.readWorkspace('w'),
  };
}
async function fileState(path) { return { bytes: await readFile(path, 'utf8'), mtime: (await stat(path, { bigint: true })).mtimeNs }; }
const kinds = f => f.calls.map(c => c.kind);
const issue = url => ({ url: `https://example.com/${url}`, description: url });

test('own gets full uncapped evidence and shared Goal; additions get only the membership suffix', async t => {
  const f = await fixture(t);
  f.evidence.transcript[0].text += 'x'.repeat(100_000);
  // Capture the actual inherited baseline before adding fresh suffix text.
  let a = await f.agent();
  a = await f.store.resetEvidence(f.authority, captureReferenceBoundary({ ...f.evidence, transcript: f.evidence.transcript.slice(0, 1) }), a.revision);
  f.authority.evidenceGeneration = a.record.evidenceGeneration;
  await f.store.addReferences(f.authority, { documents: [{ ...f.identity, description: 'Plan' }] });
  const w = await f.workspace();
  const captured = await capturePlanSources(w.record, [f.identity]);
  await f.store.applyPlan(f.authority, { goal: 'Existing shared scope', sources: [f.identity], fingerprint: captured.fingerprint }, w.revision);
  f.handlers.own = () => ({ action: 'update', patch: { now: 'Library usable', remaining: 'Client integration', status: 'waiting' } });
  f.handlers.shared = () => ({ action: 'add', additions: { issues: [issue('task')] } });
  let started = 0, stopped = 0;
  const result = await f.refresh({ onRequest: () => { started++; return () => { stopped++; }; } });
  assert.deepEqual(kinds(f), ['own', 'shared', 'select']);
  assert.deepEqual(f.calls[0].data.transcript, f.evidence.transcript);
  assert.equal(f.calls[0].data.compactionSummary, 'Inherited checkpoint');
  assert.equal(f.calls[0].data.workspace.goal, 'Existing shared scope');
  assert.deepEqual(f.calls[1].data.transcript, f.evidence.transcript.slice(1));
  assert.equal('compactionSummary' in f.calls[1].data, false);
  assert.equal(f.calls[1].data.ownCheckout, f.identity.checkout);
  assert.equal(result.agent.record.now, 'Library usable');
  assert.equal(result.agent.record.status, 'waiting');
  assert.deepEqual(result.workspace.record.issues, [issue('task')]);
  assert.equal(result.workspace.record.goal, 'Existing shared scope');
  assert.equal(started, 3); assert.equal(stopped, 3);
});

test('empty suffix skips shared helper; no_change preserves record bytes and mtimes', async t => {
  const f = await fixture(t);
  const a = await f.agent(), w = await f.workspace();
  const before = [await fileState(a.path), await fileState(w.path)];
  const result = await f.refresh({ evidence: structuredClone(inherited) });
  assert.deepEqual(kinds(f), ['own']);
  assert.equal(result.agent.revision, a.revision); assert.equal(result.workspace.revision, w.revision);
  assert.deepEqual([await fileState(a.path), await fileState(w.path)], before);
});

for (const discontinuity of ['checkpoint', 'prefix', 'shorter']) {
  test(`discontinuous ${discontinuity} rejects before any phase without resetting records`, async t => {
    const f = await fixture(t); const before = await fileState((await f.agent()).path);
    if (discontinuity === 'checkpoint') f.evidence.compactionSummary = 'New checkpoint';
    if (discontinuity === 'prefix') f.evidence.transcript[0].text = 'Different branch';
    if (discontinuity === 'shorter') f.evidence.transcript = [];
    await assert.rejects(f.refresh(), /boundary|continuity|reset/i);
    assert.deepEqual(kinds(f), []); assert.deepEqual(await fileState((await f.agent()).path), before);
  });
}

for (const invalid of ['sessionId', 'membershipToken', 'workspaceId', 'evidenceGeneration', 'membership', 'abort', 'host']) {
  test(`initial ${invalid} invalidation blocks requests and writes`, async t => {
    const f = await fixture(t); const authority = { ...f.authority };
    if (invalid === 'abort') f.controller.abort(new Error('cancelled'));
    else if (invalid === 'host') f.host(async () => { throw new Error('host changed'); });
    else if (invalid === 'membership') {
      const a = await f.agent(); a.record.checkout = 'missing'; await writeFile(a.path, stringify(a.record));
    } else authority[invalid] = 'wrong';
    const a = await f.agent(), w = await f.workspace();
    const before = [await fileState(a.path), await fileState(w.path)];
    await assert.rejects(f.refresh({ authority }));
    assert.deepEqual(kinds(f), []); assert.deepEqual([await fileState(a.path), await fileState(w.path)], before);
  });
}

for (const stage of ['own', 'shared']) for (const invalid of ['move', 'reset', 'membership', 'abort', 'host']) {
  test(`${invalid} during ${stage} refuses stale writes and stops later phases`, async t => {
    const f = await fixture(t);
    f.handlers.own = () => ({ action: 'update', patch: { now: 'Committed own progress' } });
    f.handlers[stage] = async () => {
      if (invalid === 'move') {
        await f.store.createWorkspace({ id: 'other', creationToken: 'other', title: 'Other' });
        await f.store.join({ workspaceId: 'other', sessionId: 's', cwd: f.root, membershipToken: 'new', boundary: f.boundary, title: 'Moved', expected: f.authority });
      }
      if (invalid === 'reset') await f.store.resetEvidence(f.authority, f.boundary, (await f.agent()).revision);
      if (invalid === 'membership') {
        const a = await f.agent(); a.record.checkout = 'missing'; await writeFile(a.path, stringify(a.record));
      }
      if (invalid === 'abort') f.controller.abort(new Error('cancelled'));
      if (invalid === 'host') f.host(async () => { throw new Error('host changed'); });
      return stage === 'own' ? { action: 'update', patch: { now: 'Stale result' } } : { action: 'add', additions: { issues: [issue('stale')] } };
    };
    await assert.rejects(f.refresh());
    assert.equal((await f.agent()).record.now, stage === 'own' ? '' : 'Committed own progress');
    assert.deepEqual((await f.workspace()).record.issues, []);
    assert.deepEqual(kinds(f), stage === 'own' ? ['own'] : ['own', 'shared']);
  });
}

test('captures authority, evidence and force before awaits so caller mutation cannot retarget a request', async t => {
  const f = await fixture(t);
  await f.store.addReferences(f.authority, { documents: [{ ...f.identity, description: 'Plan' }] });
  await f.refresh(); f.calls.length = 0;
  const input = { ...f.input, authority: { ...f.authority }, evidence: structuredClone(f.evidence), force: true };
  const expectedEvidence = structuredClone(input.evidence);
  f.host(async () => {
    input.authority.sessionId = 'other'; input.authority.workspaceId = 'other';
    input.evidence.compactionSummary = 'Mutated'; input.evidence.transcript[0].text = 'Mutated';
    input.evidence.transcript.push({ role: 'user', text: 'Injected' }); input.force = false;
  });
  f.handlers.own = () => ({ action: 'update', patch: { now: 'Captured request' } });
  const result = await f.refresher.refresh(input);
  assert.equal(result.agent.record.now, 'Captured request');
  assert.deepEqual(f.calls[0].data.transcript, expectedEvidence.transcript);
  assert.equal(f.calls[0].data.compactionSummary, expectedEvidence.compactionSummary);
  assert.deepEqual(f.calls[1].data.transcript, expectedEvidence.transcript.slice(1));
  assert.deepEqual(kinds(f), ['own', 'shared', 'select', 'goal']);
});

test('own patch uses original whole-record revision, surfaces conflict and does not retry', async t => {
  const f = await fixture(t);
  f.handlers.own = async () => {
    await f.store.updateOwn(f.authority, { remaining: 'Foreground winner' }, (await f.agent()).revision);
    return { action: 'update', patch: { now: 'Stale result' } };
  };
  await assert.rejects(f.refresh(), /revision/i);
  assert.equal((await f.agent()).record.now, ''); assert.equal((await f.agent()).record.remaining, 'Foreground winner');
  assert.deepEqual(kinds(f), ['own']);
});

test('shared additions merge other agents contributions; foreground exclusions win', async t => {
  const f = await fixture(t);
  const other = await f.store.join({ workspaceId: 'w', sessionId: 'other', cwd: f.root, membershipToken: 'other', boundary: f.boundary, title: 'Other' });
  const otherAuthority = { ...f.authority, sessionId: 'other', membershipToken: 'other', evidenceGeneration: other.record.evidenceGeneration };
  await f.store.addReferences(f.authority, { issues: [issue('removed')] });
  f.handlers.shared = async () => {
    await f.store.addReferences(otherAuthority, { prs: [issue('concurrent')] });
    const w = await f.workspace();
    await f.store.removeReference(f.authority, { workspaceId: 'w', revision: w.revision, identity: referenceIdentity(issue('removed')) });
    await f.store.updateOwn(f.authority, { remaining: 'Foreground scope' }, (await f.agent()).revision);
    return { action: 'add', additions: { issues: [issue('removed'), issue('new')] } };
  };
  const result = await f.refresh();
  assert.deepEqual(result.workspace.record.issues, [issue('new')]);
  assert.deepEqual(result.workspace.record.prs, [issue('concurrent')]);
  assert.deepEqual(result.workspace.record.exclusions, [{ url: issue('removed').url }]);
  assert.equal(result.agent.record.remaining, 'Foreground scope');
});

test('newly added plans participate immediately; retained plan cache skips stable inference, force reruns it', async t => {
  const f = await fixture(t);
  await f.store.setOwnTitle(f.authority, 'Manual agent', (await f.agent()).revision);
  await f.store.setWorkspaceTitle(f.authority, 'Manual workspace', (await f.workspace()).revision);
  f.handlers.own = () => ({ action: 'update', patch: { title: 'Automatic agent', now: 'Library usable' } });
  f.handlers.shared = () => ({ action: 'add', additions: { documents: [{ ...f.identity, description: 'Implementation plan' }] } });
  const first = await f.refresh();
  assert.deepEqual(kinds(f), ['own', 'shared', 'select', 'goal']);
  assert.deepEqual(first.workspace.record.planSources, [f.identity]);
  assert.equal(first.workspace.record.goal, 'Deliver library');
  assert.equal(first.agent.record.title, 'Manual agent'); assert.equal(first.workspace.record.title, 'Manual workspace');
  const before = [await fileState(first.agent.path), await fileState(first.workspace.path)];
  await f.refresh();
  assert.deepEqual(kinds(f), ['own', 'shared', 'select', 'goal', 'own', 'shared']);
  assert.deepEqual([await fileState(first.agent.path), await fileState(first.workspace.path)], before);
  await f.refresh({ force: true });
  assert.deepEqual(kinds(f).slice(-4), ['own', 'shared', 'select', 'goal']);
  await writeFile(join(f.root, 'plan.md'), 'Changed intended scope');
  await f.refresh(); assert.deepEqual(kinds(f).slice(-3), ['own', 'shared', 'goal']);
  assert.equal(f.calls.at(-1).data.sources[0].content, 'Changed intended scope');
});

for (const stage of ['own', 'shared', 'select', 'goal']) {
  test(`${stage} failure rejects without retry or rollback of earlier phases`, async t => {
    const f = await fixture(t);
    f.handlers.own = () => ({ action: 'update', patch: { now: 'Durable progress' } });
    f.handlers.shared = () => ({ action: 'add', additions: { documents: [{ ...f.identity, description: 'Plan' }] } });
    f.handlers[stage] = () => { throw new Error(`${stage} failed`); };
    await assert.rejects(f.refresh(), new RegExp(`${stage} failed`));
    assert.deepEqual(kinds(f), ['own', 'shared', 'select', 'goal'].slice(0, ['own', 'shared', 'select', 'goal'].indexOf(stage) + 1));
    assert.equal((await f.agent()).record.now, stage === 'own' ? '' : 'Durable progress');
    const w = (await f.workspace()).record;
    assert.equal(w.documents.length, ['select', 'goal'].includes(stage) ? 1 : 0);
    assert.equal(w.goal, ''); assert.equal(w.sourceFingerprint, null);
  });
}

test('final snapshots are host checked even after no_change decisions', async t => {
  const f = await fixture(t);
  f.handlers.shared = () => { f.host(async () => { throw new Error('final host invalidated'); }); return noChange(); };
  await assert.rejects(f.refresh(), /final host invalidated/);
  assert.deepEqual(kinds(f), ['own', 'shared']);
});
