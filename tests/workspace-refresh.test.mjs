import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtemp, mkdir, writeFile, rm, realpath, readFile, stat } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { stringify } from 'yaml';
import { evaluatedGoal, goalText, RecordStore, referenceIdentity } from '../dist/records.js';
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
  const calls = [], handlers = { own: noChange, shared: noChange, select: () => ({ sources: [identity] }), goal: () => ({ goal: 'Deliver library', goalShort: 'Deliver library', title: 'Automatic workspace' }) };
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
for (const field of ['workspaceId', 'membershipToken', 'checkout', 'cwd', 'evidenceGeneration']) test(`own inference never starts after ${field} changes during paired read`, async t => {
  const f = await fixture(t), agent = await f.agent();
  const readWorkspace = f.store.readWorkspace.bind(f.store); let once = true;
  f.store.readWorkspace = async id => {
    const result = await readWorkspace(id);
    if (once) {
      once = false;
      await writeFile(agent.path, JSON.stringify({ ...agent.record, [field]: field === 'cwd' ? join(f.root, 'subdir') : 'changed' }));
    }
    return result;
  };
  await assert.rejects(f.refresh(), /membership|generation|checkout/i);
  assert.deepEqual(f.calls, []);
});

test('reference-only evidence is copied before awaits and never sent to own inference', async t => {
  const f = await fixture(t);
  const referenceEvidence = { compactionSummary: null, transcript: [{ role: 'tool_activity', operation: 'edit', path: 'plan.md' }] };
  f.handlers.own = () => { referenceEvidence.transcript[0].path = 'Injected'; return noChange(); };
  await f.refresh({ evidence: { compactionSummary: null, transcript: [] }, referenceEvidence,
    authority: { ...f.authority, evidenceGeneration: (await f.store.resetEvidence(f.authority, captureReferenceBoundary({ compactionSummary: null, transcript: [] }), (await f.agent()).revision)).record.evidenceGeneration } });
  assert.deepEqual(f.calls.find(c => c.kind === 'own').data.transcript, []);
  assert.deepEqual(f.calls.find(c => c.kind === 'shared').data.transcript, [{ role: 'tool_activity', operation: 'edit', path: 'plan.md' }]);
});

const freshOwn = () => ({ action: 'update', patch: { title: 'Fresh assignment', status: 'active', statusShort: '', now: '', remaining: '' } });

test('reset replaces derived own fields but preserves identities, manual titles, references, exclusions and other agents', async t => {
  const f = await fixture(t);
  await f.store.setOwnTitle(f.authority, 'Manual agent', (await f.agent()).revision);
  await f.store.setWorkspaceTitle(f.authority, 'Manual workspace', (await f.workspace()).revision);
  await f.store.updateOwn(f.authority, { now: 'Stale now', remaining: 'Stale scope', statusShort: 'Stale stage' }, (await f.agent()).revision);
  await f.store.addReferences(f.authority, { documents: [{ ...f.identity, description: 'Plan' }], issues: [issue('keep'), issue('removed')], prs: [issue('keep-pr')] });
  const w = await f.workspace();
  await f.store.removeReference(f.authority, { workspaceId: 'w', revision: w.revision, identity: referenceIdentity(issue('removed')) });
  const other = await f.store.join({ workspaceId: 'w', sessionId: 'other', cwd: f.root, membershipToken: 'other', boundary: f.boundary, title: 'Other' });
  const beforeAgent = (await f.agent()).record, beforeWorkspace = (await f.workspace()).record;
  f.handlers.own = data => {
    assert.deepEqual(data.agent, { checkout: beforeAgent.checkout, cwd: beforeAgent.cwd });
    assert.equal(data.workspace, undefined);
    return freshOwn();
  };
  f.handlers.shared = data => {
    assert.equal(data.workspace, undefined); assert.equal(data.references, undefined);
    return { action: 'update', additions: { issues: [issue('removed'), issue('new')] } };
  };
  const result = await f.refresh({ intent: 'reset' });
  const { title: ignoredTitle, ...derived } = freshOwn().patch;
  assert.deepEqual(result.agent.record, { ...beforeAgent, ...derived });
  assert.equal(result.workspace.record.title, 'Manual workspace');
  for (const key of ['id', 'creationToken', 'checkouts', 'documents', 'prs', 'exclusions']) assert.deepEqual(result.workspace.record[key], beforeWorkspace[key]);
  assert.deepEqual(result.workspace.record.issues, [issue('keep'), issue('new')]);
  assert.equal((await f.store.readAgent('other')).revision, other.revision);
  assert.equal(goalText(result.workspace.record.goal), 'Deliver library');
});

for (const proposal of [noChange(), { action: 'update', patch: { now: '' } }, { action: 'update', patch: { ...freshOwn().patch, statusShort: 'x'.repeat(24) } }]) {
  test(`invalid reset own proposal never preclears stored records: ${JSON.stringify(proposal)}`, async t => {
    const f = await fixture(t);
    await f.store.updateOwn(f.authority, { now: 'Retained progress', remaining: 'Retained scope', statusShort: 'Retained stage' }, (await f.agent()).revision);
    const a = await f.agent(), w = await f.workspace();
    const before = [await fileState(a.path), await fileState(w.path)];
    f.handlers.own = () => proposal;
    await assert.rejects(f.refresh({ intent: 'reset' }));
    assert.deepEqual([await fileState(a.path), await fileState(w.path)], before);
    assert.deepEqual(kinds(f), ['own']);
  });
}

test('reset forces document selection and Goal inference through unchanged caches with document-only inputs', async t => {
  const f = await fixture(t);
  await f.store.addReferences(f.authority, { documents: [{ ...f.identity, description: 'Plan' }] });
  await f.refresh();
  f.calls.length = 0;
  f.handlers.own = freshOwn;
  const before = (await f.agent()).record;
  // Even identical long fields must not preserve an unseen derived short.
  await f.store.updateOwn(f.authority, { statusShort: 'Stale stage' }, (await f.agent()).revision);
  f.handlers.own = () => ({ action: 'update', patch: { title: before.title, status: before.status, now: before.now, remaining: before.remaining, statusShort: '' } });
  const events = [];
  const result = await f.refresh({ intent: 'reset', onDiagnostic: e => events.push(e) });
  assert.deepEqual(kinds(f), ['own', 'shared', 'select', 'goal']);
  assert.equal(result.agent.record.statusShort, '');
  assert.deepEqual(f.calls.at(-1).data, { sources: [{ identity: f.identity, content: 'Deliver a library and client' }] });
  assert.deepEqual(Object.keys(f.calls.at(-2).data), ['documents']);
  assert.equal(events.find(e => e.stage === 'selection').cacheHit, false);
});

test('reset rejects oversized Goal short instead of retaining an unseen old short for an identical Goal', async t => {
  const f = await fixture(t);
  await f.store.addReferences(f.authority, { documents: [{ ...f.identity, description: 'Plan' }] });
  await f.refresh();
  const before = await fileState((await f.workspace()).path);
  f.handlers.own = freshOwn;
  f.handlers.goal = () => ({ goal: 'Deliver library', goalShort: 'x'.repeat(26) });
  await assert.rejects(f.refresh({ intent: 'reset' }), /goalShort|short/i);
  assert.deepEqual(await fileState((await f.workspace()).path), before);
});

async function fileState(path) { return { bytes: await readFile(path, 'utf8'), mtime: (await stat(path, { bigint: true })).mtimeNs }; }
const kinds = f => f.calls.map(c => c.kind);
const issue = url => ({ url: `https://example.com/${url}`, description: url });

test('own and shared helpers get full uncapped evidence and the current workspace purpose', async t => {
  const f = await fixture(t);
  f.evidence.transcript[0].text += 'x'.repeat(100_000);
  // Capture the actual inherited baseline before adding fresh suffix text.
  let a = await f.agent();
  a = await f.store.resetEvidence(f.authority, captureReferenceBoundary({ ...f.evidence, transcript: f.evidence.transcript.slice(0, 1) }), a.revision);
  f.authority.evidenceGeneration = a.record.evidenceGeneration;
  await f.store.addReferences(f.authority, { documents: [{ ...f.identity, description: 'Plan' }] });
  const w = await f.workspace();
  const captured = await capturePlanSources(w.record, [f.identity]);
  await f.store.applyPlan(f.authority, { goal: evaluatedGoal('Existing shared scope', [f.identity], captured.fingerprint, 'Shared scope') }, w.revision);
  f.handlers.own = () => ({ action: 'update', patch: { now: 'Library usable', remaining: 'Client integration', status: 'waiting' } });
  f.handlers.shared = () => ({ action: 'update', additions: { issues: [issue('task')] } });
  let started = 0, stopped = 0;
  const result = await f.refresh({ onRequest: () => { started++; return () => { stopped++; }; } });
  assert.deepEqual(kinds(f), ['own', 'shared', 'select']);
  assert.deepEqual(f.calls[0].data.transcript, f.evidence.transcript);
  assert.equal(f.calls[0].data.compactionSummary, 'Inherited checkpoint');
  assert.equal(f.calls[0].data.workspace.goal, 'Existing shared scope');
  assert.deepEqual(f.calls[1].data.transcript, f.evidence.transcript);
  assert.equal(f.calls[1].data.compactionSummary, 'Inherited checkpoint');
  assert.deepEqual(f.calls[1].data.workspace, { title: 'Workspace', goal: 'Existing shared scope' });
  assert.equal(f.calls[1].data.ownCheckout, f.identity.checkout);
  assert.equal(result.agent.record.now, 'Library usable');
  assert.equal(result.agent.record.status, 'waiting');
  assert.deepEqual(result.workspace.record.issues, [issue('task')]);
  assert.equal(goalText(result.workspace.record.goal), 'Existing shared scope');
  assert.equal(started, 3); assert.equal(stopped, 3);
});

test('unchanged full context still reaches shared helper; no_change preserves record bytes and mtimes', async t => {
  const f = await fixture(t);
  const a = await f.agent(), w = await f.workspace();
  const before = [await fileState(a.path), await fileState(w.path)];
  const result = await f.refresh({ evidence: structuredClone(inherited) });
  assert.deepEqual(kinds(f), ['own', 'shared']);
  assert.equal(result.agent.revision, a.revision); assert.equal(result.workspace.revision, w.revision);
  assert.deepEqual([await fileState(a.path), await fileState(w.path)], before);
});

for (const source of ['checkpoint', 'dialogue']) {
  test(`ordinary refresh discovers ${source}-only documents before attachment and derives Goal/title`, async t => {
    const f = await fixture(t);
    const evidence = source === 'checkpoint'
      ? { compactionSummary: 'The workspace design is in plan.md', transcript: [] }
      : { compactionSummary: null, transcript: [{ role: 'user', text: 'The workspace design is in plan.md' }] };
    const agent = await f.store.resetEvidence(f.authority, captureReferenceBoundary(evidence), (await f.agent()).revision);
    f.authority.evidenceGeneration = agent.record.evidenceGeneration;
    f.handlers.shared = data => {
      assert.equal(data.compactionSummary, evidence.compactionSummary);
      assert.deepEqual(data.transcript, evidence.transcript);
      assert.deepEqual(data.workspace, { title: 'Workspace', goal: '' });
      return { action: 'update', additions: { documents: [{ ...f.identity, description: 'Library design' }] } };
    };
    const first = await f.refresh({ evidence });
    assert.deepEqual(kinds(f), ['own', 'shared', 'select', 'goal']);
    assert.deepEqual(first.workspace.record.goal.sources, [f.identity]);
    assert.equal(goalText(first.workspace.record.goal), 'Deliver library');
    assert.equal(first.workspace.record.title, 'Automatic workspace');
    assert.equal(f.calls.at(-1).data.sources[0].content, 'Deliver a library and client');
    await f.store.removeReference(f.authority, { workspaceId: 'w', revision: first.workspace.revision, identity: referenceIdentity(f.identity) });
    f.handlers.shared = () => ({ action: 'update', additions: { documents: [{ ...f.identity, description: 'Historical retry' }] } });
    const removed = await f.refresh({ evidence });
    assert.deepEqual(removed.workspace.record.documents, []);
    assert.deepEqual(removed.workspace.record.exclusions, [f.identity]);
    assert.equal(goalText(removed.workspace.record.goal), '');
  });
}

test('completely empty projected evidence skips the shared helper', async t => {
  const f = await fixture(t), evidence = { compactionSummary: null, transcript: [] };
  const agent = await f.store.resetEvidence(f.authority, captureReferenceBoundary(evidence), (await f.agent()).revision);
  f.authority.evidenceGeneration = agent.record.evidenceGeneration;
  await f.refresh({ evidence });
  assert.deepEqual(kinds(f), ['own']);
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
      return stage === 'own' ? { action: 'update', patch: { now: 'Stale result' } } : { action: 'update', additions: { issues: [issue('stale')] } };
    };
    await assert.rejects(f.refresh());
    assert.equal((await f.agent()).record.now, stage === 'own' ? '' : 'Committed own progress');
    assert.deepEqual((await f.workspace()).record.issues, []);
    assert.deepEqual(kinds(f), stage === 'own' ? ['own'] : ['own', 'shared']);
  });
}

test('captures authority, evidence and intent before awaits so caller mutation cannot retarget a request', async t => {
  const f = await fixture(t);
  await f.store.addReferences(f.authority, { documents: [{ ...f.identity, description: 'Plan' }] });
  await f.refresh(); f.calls.length = 0;
  const input = { ...f.input, authority: { ...f.authority }, evidence: structuredClone(f.evidence), intent: 'refresh' };
  const expectedEvidence = structuredClone(input.evidence);
  f.host(async () => {
    input.authority.sessionId = 'other'; input.authority.workspaceId = 'other';
    input.evidence.compactionSummary = 'Mutated'; input.evidence.transcript[0].text = 'Mutated';
    input.evidence.transcript.push({ role: 'user', text: 'Injected' }); input.intent = 'automatic';
  });
  f.handlers.own = () => ({ action: 'update', patch: { now: 'Captured request' } });
  const result = await f.refresher.refresh(input);
  assert.equal(result.agent.record.now, 'Captured request');
  assert.deepEqual(f.calls[0].data.transcript, expectedEvidence.transcript);
  assert.equal(f.calls[0].data.compactionSummary, expectedEvidence.compactionSummary);
  assert.deepEqual(f.calls[1].data.transcript, expectedEvidence.transcript);
  assert.equal(f.calls[1].data.compactionSummary, expectedEvidence.compactionSummary);
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
    return { action: 'update', additions: { issues: [issue('removed'), issue('new')] } };
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
  f.handlers.shared = () => ({ action: 'update', additions: { documents: [{ ...f.identity, description: 'Implementation plan' }] } });
  const first = await f.refresh();
  assert.deepEqual(kinds(f), ['own', 'shared', 'select', 'goal']);
  assert.deepEqual(first.workspace.record.goal.sources, [f.identity]);
  assert.equal(goalText(first.workspace.record.goal), 'Deliver library');
  assert.equal(first.agent.record.title, 'Manual agent'); assert.equal(first.workspace.record.title, 'Manual workspace');
  const before = [await fileState(first.agent.path), await fileState(first.workspace.path)];
  await f.refresh();
  assert.deepEqual(kinds(f), ['own', 'shared', 'select', 'goal', 'own', 'shared']);
  assert.deepEqual([await fileState(first.agent.path), await fileState(first.workspace.path)], before);
  await f.refresh({ intent: 'refresh' });
  assert.deepEqual(kinds(f).slice(-4), ['own', 'shared', 'select', 'goal']);
  await writeFile(join(f.root, 'plan.md'), 'Changed intended scope');
  await f.refresh(); assert.deepEqual(kinds(f).slice(-3), ['own', 'shared', 'goal']);
  assert.equal(f.calls.at(-1).data.sources[0].content, 'Changed intended scope');
});

for (const stage of ['own', 'shared', 'select', 'goal']) {
  test(`${stage} failure rejects without retry or rollback of earlier phases`, async t => {
    const f = await fixture(t);
    f.handlers.own = () => ({ action: 'update', patch: { now: 'Durable progress' } });
    f.handlers.shared = () => ({ action: 'update', additions: { documents: [{ ...f.identity, description: 'Plan' }] } });
    f.handlers[stage] = () => { throw new Error(`${stage} failed`); };
    await assert.rejects(f.refresh(), new RegExp(`${stage} failed`));
    assert.deepEqual(kinds(f), ['own', 'shared', 'select', 'goal'].slice(0, ['own', 'shared', 'select', 'goal'].indexOf(stage) + 1));
    assert.equal((await f.agent()).record.now, stage === 'own' ? '' : 'Durable progress');
    const w = (await f.workspace()).record;
    assert.equal(w.documents.length, ['select', 'goal'].includes(stage) ? 1 : 0);
    assert.deepEqual(w.goal, { kind: 'unevaluated' });
  });
}

test('oversized own short preserves full updates and lets shared, selection and Goal finish without retries', async t => {
  const f = await fixture(t);
  await f.store.updateOwn(f.authority, { statusShort: 'Stale assignment' }, (await f.agent()).revision);
  const now = 'The library and client are usable across the complete supported assignment scope';
  const remaining = 'Compatibility rollout across supported environments remains';
  f.handlers.own = () => ({ action: 'update', patch: { now, remaining, status: 'waiting', statusShort: 'x'.repeat(24) } });
  f.handlers.shared = () => ({ action: 'update', additions: { documents: [{ ...f.identity, description: 'Plan' }], issues: [issue('task')] } });
  const events = [];
  const first = await f.refresh({ onDiagnostic: event => events.push(event) });
  assert.equal(first.agent.record.now, now); assert.equal(first.agent.record.remaining, remaining);
  assert.equal(first.agent.record.status, 'waiting'); assert.equal(first.agent.record.statusShort, undefined);
  assert.deepEqual(first.workspace.record.issues, [issue('task')]);
  assert.deepEqual(first.workspace.record.goal.sources, [f.identity]);
  assert.equal(goalText(first.workspace.record.goal), 'Deliver library'); assert.ok(first.workspace.record.goal.fingerprint);
  assert.deepEqual(kinds(f), ['own', 'shared', 'select', 'goal']);
  assert.equal(events.some(event => event.outcome === 'failed'), false);
  f.handlers.own = data => {
    assert.equal(data.agent.statusShort, undefined);
    return { action: 'update', patch: { statusShort: 'Assignment usable' } };
  };
  await f.refresh();
  assert.equal((await f.agent()).record.statusShort, 'Assignment usable');
  assert.deepEqual(kinds(f), ['own', 'shared', 'select', 'goal', 'own', 'shared']);
});

test('oversized-only own short becomes no_change and does not stop later stages', async t => {
  const f = await fixture(t);
  const before = await fileState((await f.agent()).path);
  f.handlers.own = () => ({ action: 'update', patch: { statusShort: 'x'.repeat(24) } });
  const events = [];
  await f.refresh({ onDiagnostic: event => events.push(event) });
  assert.deepEqual(await fileState((await f.agent()).path), before);
  assert.deepEqual(kinds(f), ['own', 'shared']);
  assert.deepEqual(events[0], { stage: 'own', outcome: 'unchanged' });
});

test('final snapshots are host checked even after no_change decisions', async t => {
  const f = await fixture(t);
  f.handlers.shared = () => { f.host(async () => { throw new Error('final host invalidated'); }); return noChange(); };
  await assert.rejects(f.refresh(), /final host invalidated/);
  assert.deepEqual(kinds(f), ['own', 'shared']);
});


test('shared relabels use the captured full descriptions and do not overwrite foreground edits', async t => {
  const f = await fixture(t); const target = issue('task');
  await f.store.addReferences(f.authority, { issues: [target] });
  f.handlers.own = () => ({ action: 'update', patch: { now: 'Assignment usable', statusShort: 'Assignment ready' } });
  f.handlers.shared = async data => {
    assert.deepEqual(data.references.issues, [target]);
    const w = await f.workspace();
    await f.store.describeReference(f.authority, { url: target.url }, 'Foreground full description', w.revision);
    return { action: 'update', labels: [{ url: target.url, label: 'Stale label' }], additions: { prs: [{ ...issue('new'), label: 'New artifact purpose' }] } };
  };
  let result = await f.refresh();
  assert.equal(result.agent.record.statusShort, 'Assignment ready');
  assert.deepEqual(result.workspace.record.issues, [{ ...target, description: 'Foreground full description' }]);
  assert.equal(result.workspace.record.prs[0].label, 'New artifact purpose');
  f.handlers.shared = () => ({ action: 'update', labels: [{ url: target.url, label: 'Fresh purpose label' }] });
  result = await f.refresh(); assert.equal(result.workspace.record.issues[0].label, 'Fresh purpose label');
});

test('diagnostics report reached phases and no-source completion without aggregate agent progress', async t => {
  const f = await fixture(t); const events = [];
  f.handlers.own = () => ({ action: 'update', patch: { statusShort: 'Assignment ready' } });
  await f.refresh({ onDiagnostic: event => events.push(event) });
  assert.deepEqual(events, [
    { stage: 'own', outcome: 'applied' },
    { stage: 'shared', outcome: 'unchanged' },
    { stage: 'selection', outcome: 'no_sources', selectedCount: 0, cacheHit: false },
    { stage: 'goal', outcome: 'no_sources' },
  ]);
  const retained = []; await f.refresh({ evidence: structuredClone(inherited), onDiagnostic: event => retained.push(event) });
  assert.deepEqual(retained.find(e => e.stage === 'shared'), { stage: 'shared', outcome: 'unchanged' });
});

for (const stage of ['own', 'shared']) {
  test(`pre-plan ${stage} failure diagnoses goal skipped and preserves original error`, async t => {
    const f = await fixture(t); const events = []; const error = new Error('original');
    f.handlers[stage] = () => { throw error; };
    await assert.rejects(f.refresh({ onDiagnostic: event => events.push(event) }), e => e === error);
    assert.deepEqual(events.at(-2), { stage, outcome: 'failed', cancelled: false });
    assert.deepEqual(events.at(-1), { stage: 'goal', outcome: 'skipped' });
    assert.equal(events.some(e => e.stage === 'selection'), false);
  });
}

test('own CAS failure and cancellation never diagnose an unapplied patch as applied', async t => {
  for (const abort of [false, true]) {
    const f = await fixture(t); const events = [];
    f.handlers.own = async () => {
      if (abort) f.controller.abort(new Error('cancelled'));
      else await f.store.updateOwn(f.authority, { now: 'Foreground wins' }, (await f.agent()).revision);
      return { action: 'update', patch: { now: 'Stale', statusShort: 'Stale' } };
    };
    await assert.rejects(f.refresh({ onDiagnostic: event => events.push(event) }));
    assert.deepEqual(events, [
      { stage: 'own', outcome: 'failed', cancelled: abort },
      { stage: 'goal', outcome: 'skipped' },
    ]);
  }
});


test('final host invalidation is diagnosed even after completed no-source goal checking', async t => {
  const f = await fixture(t); const events = []; const error = new Error('final invalidation');
  await assert.rejects(f.refresh({ onDiagnostic: event => {
    events.push(event);
    if (event.stage === 'goal' && event.outcome === 'no_sources') f.host(async () => { throw error; });
  } }), e => e === error);
  assert.deepEqual(events.at(-1), { stage: 'goal', outcome: 'failed', cancelled: false });
});
