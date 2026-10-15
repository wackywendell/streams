import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtemp, mkdir, writeFile, rm, realpath, readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { stringify } from 'yaml';
import { RecordStore, referenceIdentity } from '../dist/records.js';
import { PlanRefresher } from '../dist/pi/plan-refresh.js';
import { capturePlanSources, MAX_PLAN_SOURCE_BYTES } from '../dist/pi/plan-sources.js';

const options = { model: 'helpers/simple', thinking_level: null, max_output_tokens: 512, request_timeout_ms: 1000 };
const boundary = { compactionDigest: 'd', transcriptLength: 0, prefixDigest: 'p' };
async function fixture(t, withReference = true) {
  const base = await realpath(await mkdtemp(join(tmpdir(), 'wc-refresh-')));
  t.after(() => rm(base, { recursive: true, force: true }));
  const root = join(base, 'repo'); await mkdir(root);
  const path = join(root, 'plan.md'); await writeFile(path, 'Deliver a library and client');
  const store = new RecordStore(join(base, 'records'));
  await store.createWorkspace({ id: 'w', creationToken: 'creation', title: 'Workspace' });
  const agent = await store.join({ workspaceId: 'w', sessionId: 's', cwd: root, membershipToken: 'member', boundary, title: 'SECRET session' });
  const authority = { sessionId: 's', workspaceId: 'w', membershipToken: 'member', evidenceGeneration: agent.record.evidenceGeneration };
  const identity = { checkout: agent.record.checkout, path: 'plan.md' };
  if (withReference) await store.addReferences(authority, { documents: [{ ...identity, description: 'Implementation plan' }, { ...identity, path: 'code.ts', description: 'Not Markdown' }] });
  const calls = []; let select = () => ({ sources: [identity] }); let goal = () => ({ goal: 'Deliver library', goalShort: 'Deliver library', title: 'Automatic' });
  const registry = {
    find(provider, id) { assert.equal(provider, 'helpers'); assert.equal(id, 'simple'); return { api: 'pi-virtual' }; },
    streamSimple(_model, context, config) {
      assert.deepEqual(context.tools, []); assert.equal(config.toolChoice, 'none'); assert.equal(config.reasoning, 'low');
      const data = JSON.parse(context.messages[0].content[0].text);
      const kind = Object.hasOwn(data, 'documents') ? 'select' : 'goal'; calls.push({ kind, data });
      return { result: async () => ({ stopReason: 'stop', content: [{ type: 'text', text: JSON.stringify(await (kind === 'select' ? select(data) : goal(data))) }] }) };
    },
  };
  const refresher = new PlanRefresher({ store, registry, options, loadHelperProfiles: async () => ({ simple: { thinkingLevel: 'low' } }) });
  const controller = new AbortController(); let host = async () => {};
  const refresh = (extra = {}) => refresher.refresh({ authority, signal: controller.signal, assertCurrent: () => host(), ...extra });
  return { base, root, path, store, authority, identity, calls, refresher, controller, refresh,
    select(fn) { select = fn; }, goal(fn) { goal = fn; }, host(fn) { host = fn; },
    workspace: () => store.readWorkspace('w'),
  };
}

test('no references needs no model and preserves valid empty state with checked authority', async t => {
  const f = await fixture(t, false);
  const w = await f.workspace();
  const checked = await f.refresh();
  assert.equal(checked.revision, w.revision);
  assert.equal(checked.record.goal, ''); assert.deepEqual(checked.record.planSources, []); assert.equal(checked.record.sourceFingerprint, null);
  assert.deepEqual(f.calls, []);
});

test('initial selection is metadata-only, stable checks skip inference and force regenerates from captured sources', async t => {
  const f = await fixture(t); let activity = 0; let stopped = 0;
  const first = await f.refresh({ onRequest: () => { activity++; return () => { stopped++; }; } });
  assert.equal(first.record.goal, 'Deliver library'); assert.deepEqual(first.record.planSources, [f.identity]); assert.ok(first.record.sourceFingerprint);
  assert.deepEqual(f.calls.map(c => c.kind), ['select', 'goal']);
  assert.deepEqual(f.calls[0].data, { documents: [{ ...f.identity, description: 'Implementation plan' }] });
  assert.deepEqual(f.calls[1].data, { sources: [{ identity: f.identity, content: 'Deliver a library and client' }] });
  assert.equal(activity, 2); assert.equal(stopped, 2);
  assert.equal((await f.refresh()).revision, first.revision);
  f.goal(() => ({ goal: 'Corrected goal', goalShort: 'Corrected goal' }));
  const corrected = await f.refresh({ force: true });
  assert.equal(corrected.record.goal, 'Corrected goal');
  assert.deepEqual(f.calls.map(c => c.kind), ['select', 'goal', 'select', 'goal']);
  await writeFile(f.path, 'Deliver changed capability');
  await f.refresh();
  assert.deepEqual(f.calls.map(c => c.kind), ['select', 'goal', 'select', 'goal', 'goal']);
  assert.equal(f.calls.at(-1).data.sources[0].content, 'Deliver changed capability');
});

test('candidate descriptions and identities invalidate selection, unrelated non-Markdown metadata does not', async t => {
  const f = await fixture(t); await f.refresh();
  let w = await f.workspace();
  await f.store.describeReference(f.authority, { ...f.identity, path: 'code.ts' }, 'Changed code', w.revision);
  await f.refresh(); assert.equal(f.calls.length, 2);
  w = await f.workspace();
  await f.store.describeReference(f.authority, f.identity, 'New purpose', w.revision);
  await f.refresh(); assert.equal(f.calls.at(-1).kind, 'select'); assert.equal(f.calls.length, 3);
  await f.store.addReferences(f.authority, { documents: [{ ...f.identity, path: 'other.md', description: 'Other plan' }] });
  await f.refresh(); assert.equal(f.calls.length, 4);
});

test('empty selection clears provenance and does not call goal model', async t => {
  const f = await fixture(t); await f.refresh(); f.select(() => ({ sources: [] }));
  const result = await f.refresh({ force: true });
  assert.equal(result.record.goal, ''); assert.deepEqual(result.record.planSources, []); assert.equal(result.record.sourceFingerprint, null);
  await f.refresh(); assert.deepEqual(f.calls.map(c => c.kind), ['select', 'goal', 'select']);
});

test('selection cache survives source errors but failed goal is never treated as applied', async t => {
  const f = await fixture(t); await rm(f.path);
  await assert.rejects(f.refresh(), /Cannot read plan source/);
  assert.deepEqual(f.calls.map(c => c.kind), ['select']);
  await writeFile(f.path, 'Restored'); f.goal(() => { throw new Error('inference failed'); });
  await assert.rejects(f.refresh(), /inference failed/);
  assert.equal((await f.workspace()).record.sourceFingerprint, null);
  f.goal(() => ({ goal: 'Recovered', goalShort: 'Recovered' })); await f.refresh();
  assert.deepEqual(f.calls.map(c => c.kind), ['select', 'goal', 'goal']);
  assert.equal((await f.workspace()).record.goal, 'Recovered');
});

test('oversized selected source fails visibly without goal request or retry', async t => {
  const f = await fixture(t); await writeFile(f.path, 'x'.repeat(MAX_PLAN_SOURCE_BYTES + 1));
  await assert.rejects(f.refresh(), /256|limit/i);
  assert.deepEqual(f.calls.map(c => c.kind), ['select']); assert.equal((await f.workspace()).record.goal, '');
});

for (const mutation of ['bytes', 'remove', 'description', 'root', 'shared', 'plan', 'move', 'reset', 'cancel', 'host']) {
  test(`refuses ${mutation} change during goal inference without overwriting concurrent state`, async t => {
    const f = await fixture(t);
    const before = await f.workspace();
    // A prior valid fingerprint matching initial capture cannot prove stability during forced inference.
    const capture = await capturePlanSources(before.record, [f.identity]);
    await f.store.applyPlan(f.authority, { goal: 'Prior', sources: [f.identity], fingerprint: capture.fingerprint }, before.revision);
    f.goal(async () => {
      const w = await f.workspace();
      if (mutation === 'bytes') await writeFile(f.path, 'Different after capture');
      if (mutation === 'remove') await f.store.removeReference(f.authority, { workspaceId: 'w', revision: w.revision, identity: referenceIdentity(f.identity) });
      if (mutation === 'description') await f.store.describeReference(f.authority, f.identity, 'Changed', w.revision);
      if (mutation === 'root') {
        const other = join(f.base, 'other'); await mkdir(other); await writeFile(join(other, 'plan.md'), 'Deliver a library and client');
        w.record.checkouts[f.identity.checkout].directory = other;
        await writeFile(w.path, stringify(w.record));
      }
      if (mutation === 'shared') await f.store.addReferences(f.authority, { issues: [{ url: 'https://example.com/new', description: 'Concurrent' }] });
      if (mutation === 'plan') await f.store.applyPlan(f.authority, { goal: 'Concurrent plan', sources: [f.identity], fingerprint: capture.fingerprint }, w.revision);
      if (mutation === 'move') {
        await f.store.createWorkspace({ id: 'other', creationToken: 'other', title: 'Other' });
        await f.store.join({ workspaceId: 'other', sessionId: 's', cwd: f.root, membershipToken: 'new', boundary, title: 'Moved', expected: f.authority });
      }
      if (mutation === 'reset') { const a = await f.store.readAgent('s'); await f.store.resetEvidence(f.authority, boundary, a.revision); }
      if (mutation === 'cancel') f.controller.abort(new Error('cancelled'));
      if (mutation === 'host') f.host(async () => { throw new Error('host invalidated'); });
      return { goal: 'Must not apply', goalShort: 'Must not apply' };
    });
    await assert.rejects(f.refresh({ force: true }));
    assert.notEqual((await f.workspace()).record.goal, 'Must not apply');
    assert.equal(f.calls.length, 2);
  });
}

test('manual title stays protected and concurrent own-status update is allowed', async t => {
  const f = await fixture(t); const w = await f.workspace();
  await f.store.setWorkspaceTitle(f.authority, 'My title', w.revision);
  f.goal(async () => {
    const a = await f.store.readAgent('s');
    await f.store.updateOwn(f.authority, { status: 'waiting', now: 'SECRET own status' }, a.revision);
    return { goal: 'Valid plan goal', goalShort: 'Valid plan goal', title: 'Model title' };
  });
  const result = await f.refresh(); assert.equal(result.record.title, 'My title'); assert.equal(result.record.goal, 'Valid plan goal');
  assert.equal((await f.store.readAgent('s')).record.status, 'waiting');
  assert.doesNotMatch(JSON.stringify(f.calls), /SECRET/);
});

test('wrong authority, pre-cancel and initial host invalidation block all requests and effects', async t => {
  for (const kind of ['membershipToken', 'evidenceGeneration', 'workspaceId', 'cancel', 'host']) {
    const f = await fixture(t); const before = await readFile(f.store.workspacePath('w'));
    const authority = { ...f.authority };
    if (kind === 'cancel') f.controller.abort(new Error('cancelled'));
    else if (kind === 'host') f.host(async () => { throw new Error('invalid host'); });
    else authority[kind] = 'wrong';
    await assert.rejects(f.refresh({ authority })); assert.deepEqual(f.calls, []);
    assert.deepEqual(await readFile(f.store.workspacePath('w')), before);
  }
});

test('shared mutation during selection refuses before goal request and does not cache stale choice', async t => {
  const f = await fixture(t);
  f.select(async () => { const w = await f.workspace(); await f.store.describeReference(f.authority, f.identity, 'Updated', w.revision); return { sources: [f.identity] }; });
  await assert.rejects(f.refresh()); assert.deepEqual(f.calls.map(c => c.kind), ['select']);
  f.select(() => ({ sources: [f.identity] })); await f.refresh();
  assert.deepEqual(f.calls.map(c => c.kind), ['select', 'select', 'goal']);
});

test('host invalidation after filesystem preparation blocks goal request', async t => {
  const f = await fixture(t);
  f.select(() => { f.host(async () => { throw new Error('invalidated after selection'); }); return { sources: [f.identity] }; });
  await assert.rejects(f.refresh(), /invalidated/); assert.equal(f.calls.length, 1);
});

test('explicit force can replace a cached choice and infer from the new source', async t => {
  const f = await fixture(t);
  const other = { ...f.identity, path: 'other.md' };
  await writeFile(join(f.root, other.path), 'Another intended scope');
  await f.store.addReferences(f.authority, { documents: [{ ...other, description: 'Alternative' }] });
  await f.refresh(); f.select(() => ({ sources: [other] }));
  await f.refresh(); assert.equal(f.calls.length, 2);
  const result = await f.refresh({ force: true });
  assert.deepEqual(result.record.planSources, [other]);
  assert.equal(f.calls.at(-1).data.sources[0].content, 'Another intended scope');
  assert.deepEqual(f.calls.map(c => c.kind), ['select', 'goal', 'select', 'goal']);
});

test('final CAS rejects a concurrent plan written after filesystem recheck', async t => {
  const f = await fixture(t); let afterInference = false; let guards = 0;
  f.goal(() => { afterInference = true; return { goal: 'Stale result', goalShort: 'Stale result' }; });
  f.host(async () => {
    if (afterInference && ++guards === 2) {
      const w = await f.workspace();
      const capture = await capturePlanSources(w.record, [f.identity]);
      await f.store.applyPlan(f.authority, { goal: 'Concurrent winner', sources: [f.identity], fingerprint: capture.fingerprint }, w.revision);
    }
  });
  await assert.rejects(f.refresh(), /revision/i);
  assert.equal((await f.workspace()).record.goal, 'Concurrent winner');
});

test('host guard after filesystem recheck rejects effects even after valid inference', async t => {
  const f = await fixture(t); let afterInference = false; let guards = 0;
  f.goal(() => { afterInference = true; return { goal: 'Stale result', goalShort: 'Stale result' }; });
  f.host(async () => { if (afterInference && ++guards === 2) throw new Error('host invalidated after recheck'); });
  await assert.rejects(f.refresh(), /after recheck/);
  assert.equal((await f.workspace()).record.goal, '');
});

test('workspace recreation does not reuse prior selection cache', async t => {
  const f = await fixture(t); await f.refresh();
  let a = await f.store.readAgent('s'); await f.store.deleteOwn(f.authority, a.revision);
  const w = await f.workspace(); await f.store.deleteWorkspace('w', w.revision);
  await f.store.createWorkspace({ id: 'w', creationToken: 'recreated', title: 'New' });
  a = await f.store.join({ workspaceId: 'w', sessionId: 's', cwd: f.root, membershipToken: 'new', boundary, title: 'New' });
  const authority = { ...f.authority, membershipToken: 'new', evidenceGeneration: a.record.evidenceGeneration };
  // Preserve identical candidate metadata to isolate the creation-token cache boundary.
  const replacement = await f.workspace();
  const replacementAgent = { ...a.record, checkout: f.identity.checkout };
  replacement.record.checkouts = { [f.identity.checkout]: { directory: f.root } };
  await writeFile(replacement.path, stringify(replacement.record));
  await writeFile(a.path, stringify(replacementAgent));
  await f.store.addReferences(authority, { documents: [{ ...f.identity, description: 'Implementation plan' }, { ...f.identity, path: 'code.ts', description: 'Not Markdown' }] });
  f.select(() => ({ sources: [f.identity] })); await f.refresh({ authority });
  assert.deepEqual(f.calls.map(c => c.kind), ['select', 'goal', 'select', 'goal']);
});


for (const short of [undefined, '']) test(`${short === undefined ? 'missing' : 'empty'} derived goal short is regenerated without reselecting unchanged source metadata`, async t => {
  const f = await fixture(t); await f.refresh(); const before = await f.workspace();
  if (short === undefined) delete before.record.goalShort; else before.record.goalShort = short;
  await writeFile(before.path, stringify(before.record));
  f.calls.length = 0; const result = await f.refresh();
  assert.deepEqual(f.calls.map(c => c.kind), ['goal']);
  assert.equal(result.record.goalShort, 'Deliver library');
});

test('oversized Goal short preserves the full Goal, title and provenance, then regenerates on an ordinary check', async t => {
  const f = await fixture(t); await f.refresh();
  await writeFile(f.path, 'A changed intended workspace purpose');
  const goal = 'Deliver the complete library and client capability with a stable public integration surface';
  f.goal(() => ({ goal, goalShort: '界'.repeat(13), title: 'Library integration' }));
  f.calls.length = 0;
  const events = [];
  const degraded = await f.refresh({ onDiagnostic: event => events.push(event) });
  assert.equal(degraded.record.goal, goal); assert.equal(degraded.record.title, 'Library integration');
  assert.equal(degraded.record.goalShort, undefined);
  assert.deepEqual(degraded.record.planSources, [f.identity]);
  const capture = await capturePlanSources(degraded.record, [f.identity]);
  assert.equal(degraded.record.sourceFingerprint, capture.fingerprint);
  assert.deepEqual(f.calls.map(c => c.kind), ['goal']);
  assert.deepEqual(events.at(-1), { stage: 'goal', outcome: 'applied' });
  f.goal(() => ({ goal, goalShort: 'Library integration' }));
  const recovered = await f.refresh();
  assert.equal(recovered.record.goalShort, 'Library integration');
  assert.deepEqual(f.calls.map(c => c.kind), ['goal', 'goal']);
  assert.equal((await f.refresh()).revision, recovered.revision);
  assert.deepEqual(f.calls.map(c => c.kind), ['goal', 'goal']);
});

test('oversized Goal short leaves an unchanged valid short intact rather than overwriting it', async t => {
  const f = await fixture(t); const first = await f.refresh();
  f.goal(() => ({ goal: first.record.goal, goalShort: 'x'.repeat(26) }));
  const result = await f.refresh({ force: true });
  assert.equal(result.record.goalShort, first.record.goalShort);
  assert.equal(result.revision, first.revision);
  assert.deepEqual(f.calls.map(c => c.kind), ['select', 'goal', 'select', 'goal']);
});

test('diagnostics distinguish selection, cache, no sources and successful goal writes', async t => {
  const f = await fixture(t); const events = [];
  const onDiagnostic = event => events.push(event);
  await f.refresh({ onDiagnostic });
  assert.deepEqual(events, [
    { stage: 'selection', outcome: 'selected', selectedCount: 1, cacheHit: false },
    { stage: 'goal', outcome: 'applied' },
  ]);
  events.length = 0; await f.refresh({ onDiagnostic });
  assert.deepEqual(events, [
    { stage: 'selection', outcome: 'selected', selectedCount: 1, cacheHit: true },
    { stage: 'goal', outcome: 'unchanged' },
  ]);
  events.length = 0; f.select(() => ({ sources: [] })); await f.refresh({ force: true, onDiagnostic });
  assert.deepEqual(events, [
    { stage: 'selection', outcome: 'no_sources', selectedCount: 0, cacheHit: false },
    { stage: 'goal', outcome: 'no_sources' },
  ]);
  assert.equal((await f.workspace()).record.goalShort, undefined);
  assert.doesNotMatch(JSON.stringify(events), /plan.md|Deliver|file|https/);
});

for (const stage of ['selection', 'goal', 'source', 'cas', 'abort']) {
  test(`diagnostics preserve original ${stage} failure without claiming applied`, async t => {
    const f = await fixture(t); const events = []; const error = new Error('original failure');
    if (stage === 'selection') f.select(() => { throw error; });
    if (stage === 'goal') f.goal(() => { throw error; });
    if (stage === 'source') await rm(f.path);
    if (stage === 'cas') t.mock.method(f.store, 'applyPlan', async () => { throw error; });
    if (stage === 'abort') f.goal(() => { f.controller.abort(error); return { goal: 'Must not apply', goalShort: 'Not applied' }; });
    const result = f.refresh({ onDiagnostic: event => events.push(event) });
    if (stage === 'source') await assert.rejects(result, /Cannot read/); else await assert.rejects(result, e => e === error);
    assert.equal(events.some(e => e.outcome === 'applied'), false);
    if (stage === 'selection') assert.deepEqual(events, [
      { stage: 'selection', outcome: 'failed', cancelled: false },
      { stage: 'goal', outcome: 'skipped' },
    ]);
    else assert.deepEqual(events.at(-1), { stage: 'goal', outcome: 'failed', cancelled: stage === 'abort' });
  });
}
