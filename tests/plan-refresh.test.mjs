import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtemp, mkdir, writeFile, rm, realpath, readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { stringify } from 'yaml';
import { evaluatedGoal, serializeWorkspaceV1, goalText, RecordStore, referenceIdentity } from '../dist/records.js';
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

test('workspace title publication requires a guarded accepted known title, including same text but never cached or protected proposals', async t => {
  for (const documents of [true, false]) {
    const f = await fixture(t, documents), published = [];
    const input = { evidence: dialogue(), onWorkspaceTitlePublished: w => published.push(w.record.title) };
    await f.refresh(input); assert.deepEqual(published, ['Automatic']);
    await f.refresh(input); assert.equal(published.length, 1);
    await f.refresh({ ...input, intent: 'reset' }); assert.deepEqual(published, ['Automatic', 'Automatic']);
    f.goal(() => ({ goal: 'Known purpose', goalShort: 'Purpose' })); await f.refresh({ ...input, intent: 'refresh' }); assert.equal(published.length, 2);
    await f.store.setWorkspaceTitle(f.authority, 'Protected', (await f.workspace()).revision);
    f.goal(() => ({ goal: 'Known purpose', goalShort: 'Purpose', title: 'Rejected' }));
    await f.refresh({ ...input, intent: 'refresh' }); assert.equal(published.length, 2);
  }
});
test('workspace publication excludes sparse retained and unknown proposals and rejected CAS', async t => {
  const f = await fixture(t, false), published = [], input = { evidence: dialogue(), onWorkspaceTitlePublished: w => published.push(w) };
  await f.refresh(input); assert.equal(published.length, 1);
  f.goal(() => ({ goal: '', goalShort: '', title: 'Automatic' }));
  await f.refresh({ ...input, intent: 'reset' }); assert.equal(published.length, 1);
  const fresh = await fixture(t, false); fresh.goal(() => ({ goal: '', goalShort: '', title: 'Invented' }));
  await fresh.refresh(input); assert.equal(published.length, 1);
  f.goal(async () => {
    await f.store.setWorkspaceTitle(f.authority, 'Concurrent title', (await f.workspace()).revision);
    return { goal: 'New purpose', goalShort: 'Purpose', title: 'New title' };
  });
  await assert.rejects(f.refresh({ ...input, intent: 'refresh' }), /stale/i); assert.equal(published.length, 1);
});

test('cancelled title publication cannot grant workspace readiness after apply', async t => {
  const f = await fixture(t), published = [], apply = f.store.applyPlan.bind(f.store);
  f.store.applyPlan = async (...args) => { const result = await apply(...args); f.controller.abort(); return result; };
  await assert.rejects(f.refresh({ onWorkspaceTitlePublished: w => published.push(w) }), /abort/i);
  assert.deepEqual(published, []);
});

for (const field of ['workspaceId', 'membershipToken', 'checkout', 'cwd', 'evidenceGeneration']) test(`plan inference never starts after ${field} changes during paired read`, async t => {
  const f = await fixture(t), agent = await f.store.readAgent('s');
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

test('no references needs no model and preserves valid empty state with checked authority', async t => {
  const f = await fixture(t, false);
  const w = await f.workspace();
  const checked = await f.refresh();
  assert.equal(checked.revision, w.revision);
  assert.equal(goalText(checked.record.goal), ''); assert.equal(checked.record.goal.kind, 'unevaluated');
  assert.deepEqual(f.calls, []);
});

test('initial selection is metadata-only, stable checks skip inference and force regenerates from captured sources', async t => {
  const f = await fixture(t); let activity = 0; let stopped = 0;
  const first = await f.refresh({ onRequest: () => { activity++; return () => { stopped++; }; } });
  assert.equal(goalText(first.record.goal), 'Deliver library'); assert.deepEqual(first.record.goal.provenance.sources, [f.identity]); assert.ok(first.record.goal.provenance.fingerprint);
  assert.deepEqual(f.calls.map(c => c.kind), ['select', 'goal']);
  assert.deepEqual(f.calls[0].data, { documents: [{ ...f.identity, description: 'Implementation plan' }] });
  assert.deepEqual(f.calls[1].data, { sources: [{ identity: f.identity, content: 'Deliver a library and client' }] });
  assert.equal(activity, 2); assert.equal(stopped, 2);
  assert.equal((await f.refresh()).revision, first.revision);
  f.goal(() => ({ goal: 'Corrected goal', goalShort: 'Corrected goal' }));
  const corrected = await f.refresh({ intent: 'refresh' });
  assert.equal(goalText(corrected.record.goal), 'Corrected goal');
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

test('evaluated unknown purpose retains provenance and skips repeated inference', async t => {
  const f = await fixture(t); f.goal(() => ({ goal: '', goalShort: '' }));
  const first = await f.refresh();
  assert.equal(first.record.goal.kind, 'unknown');
  assert.deepEqual(first.record.goal.provenance.sources, [f.identity]);
  assert.ok(first.record.goal.provenance.fingerprint);
  assert.equal((await f.refresh()).revision, first.revision);
  assert.deepEqual(f.calls.map(c => c.kind), ['select', 'goal']);
  await writeFile(f.path, 'Changed documents still establish no purpose');
  await f.refresh(); assert.deepEqual(f.calls.map(c => c.kind), ['select', 'goal', 'goal']);
});

test('empty selection alone cannot erase established shared purpose and does not call goal model', async t => {
  const f = await fixture(t); await f.refresh(); f.select(() => ({ sources: [] }));
  const result = await f.refresh({ intent: 'refresh' });
  assert.equal(goalText(result.record.goal), 'Deliver library'); assert.equal(result.record.goal.kind, 'known');
  await f.refresh(); assert.deepEqual(f.calls.map(c => c.kind), ['select', 'goal', 'select']);
});

const dialogue = (text = 'Build a library and client for shared workspace context') => ({ compactionSummary: null, transcript: [{ role: 'user', text }] });

test('meaningful dialogue establishes attributed Goal, short and automatic name, caches, grows and forces', async t => {
  const f = await fixture(t, false), evidence = dialogue();
  const first = await f.refresh({ evidence });
  assert.equal(goalText(first.record.goal), 'Deliver library'); assert.equal(first.record.goal.short, 'Deliver library');
  assert.equal(first.record.title, 'Automatic');
  assert.equal(first.record.goal.provenance.kind, 'conversation'); assert.equal(first.record.goal.provenance.sessionId, 's');
  assert.match(first.record.goal.provenance.digest, /^[a-f0-9]{64}$/);
  assert.deepEqual(f.calls[0].data.transcript, evidence.transcript); assert.equal(f.calls[0].data.sources, undefined);
  assert.equal((await f.refresh({ evidence })).revision, first.revision); assert.equal(f.calls.length, 1);
  evidence.transcript.push({ role: 'assistant', text: 'The client is a reusable integration API' });
  await f.refresh({ evidence }); assert.equal(f.calls.length, 2);
  for (const intent of ['refresh', 'reset']) await f.refresh({ evidence, intent });
  assert.equal(f.calls.length, 4); assert.equal(f.calls.at(-1).data.workspace, undefined);
});

test('insufficient nonempty dialogue is cached as unknown with empty goal, short and no invented automatic name', async t => {
  const f = await fixture(t, false); f.goal(() => ({ goal: '', goalShort: '', title: 'Invented' }));
  const first = await f.refresh({ evidence: dialogue('hello') });
  assert.equal(first.record.goal.kind, 'unknown'); assert.equal(first.record.goal.provenance.kind, 'conversation');
  assert.equal(goalText(first.record.goal), ''); assert.equal(first.record.goal.short, undefined); assert.equal(first.record.title, 'Workspace');
  assert.equal((await f.refresh({ evidence: dialogue('hello') })).revision, first.revision); assert.equal(f.calls.length, 1);
  await f.refresh({ evidence: dialogue('hello again') }); assert.equal(f.calls.length, 2);
  await f.refresh({ evidence: dialogue('hello again'), intent: 'refresh' }); assert.equal(f.calls.length, 3);
});

test('sparse joining-agent context retains shared conversation purpose and name including reset, and caches its empty proposal', async t => {
  const f = await fixture(t, false), first = await f.refresh({ evidence: dialogue() });
  const joined = await f.store.join({ workspaceId: 'w', sessionId: 'other', cwd: f.root, membershipToken: 'other', boundary, title: 'Other' });
  const authority = { sessionId: 'other', workspaceId: 'w', membershipToken: 'other', evidenceGeneration: joined.record.evidenceGeneration };
  f.goal(() => ({ goal: '', goalShort: '', title: 'Invented' }));
  const input = { authority, evidence: dialogue('hello') };
  assert.equal((await f.refresh(input)).revision, first.revision); assert.equal(f.calls.at(-1).data.workspace.goal, 'Deliver library');
  assert.equal((await f.refresh(input)).revision, first.revision); assert.equal(f.calls.length, 2);
  assert.equal((await f.refresh({ ...input, intent: 'reset' })).revision, first.revision);
  assert.equal(f.calls.at(-1).data.workspace, undefined);
  assert.equal((await f.refresh({ evidence: { compactionSummary: null, transcript: [] } })).revision, first.revision);
});

test('documents take precedence, unreadable documents never fall back, and dialogue cannot replace document purpose', async t => {
  const f = await fixture(t, false); await f.refresh({ evidence: dialogue() });
  await f.store.addReferences(f.authority, { documents: [{ ...f.identity, description: 'Plan' }] });
  f.goal(() => ({ goal: 'Document purpose', goalShort: 'Document purpose' }));
  const document = await f.refresh({ evidence: dialogue() });
  assert.equal(document.record.goal.provenance.kind, 'documents'); assert.ok(f.calls.at(-1).data.sources);
  f.select(() => ({ sources: [] }));
  assert.equal((await f.refresh({ evidence: dialogue('Different assignment'), intent: 'refresh' })).revision, document.revision);
  assert.equal(f.calls.at(-1).kind, 'select');
  f.select(() => ({ sources: [f.identity] })); await writeFile(f.path, Buffer.from([0xc3, 0x28]));
  await assert.rejects(f.refresh({ evidence: dialogue(), intent: 'refresh' }), /Cannot read/);
  assert.equal((await f.workspace()).revision, document.revision);
});

for (const mutation of ['shared', 'move', 'reset', 'cancel', 'host']) test(`conversation refuses ${mutation} changes under original CAS and authority`, async t => {
  const f = await fixture(t, false);
  f.goal(async () => {
    if (mutation === 'shared') await f.store.addReferences(f.authority, { issues: [{ url: 'https://example.com/new', description: 'Concurrent' }] });
    if (mutation === 'move') {
      await f.store.createWorkspace({ id: 'other', creationToken: 'other', title: 'Other' });
      await f.store.join({ workspaceId: 'other', sessionId: 's', cwd: f.root, membershipToken: 'new', boundary, title: 'Moved', expected: f.authority });
    }
    if (mutation === 'reset') { const a = await f.store.readAgent('s'); await f.store.resetEvidence(f.authority, boundary, a.revision); }
    if (mutation === 'cancel') f.controller.abort(new Error('cancelled'));
    if (mutation === 'host') f.host(async () => { throw new Error('evidence edited or host invalidated'); });
    return { goal: 'Must not apply', goalShort: 'Must not apply' };
  });
  await assert.rejects(f.refresh({ evidence: dialogue() })); assert.equal(goalText((await f.workspace()).record.goal), '');
});

test('conversation is copied before awaits and manual workspace names stay protected', async t => {
  const f = await fixture(t, false), evidence = dialogue();
  await f.store.setWorkspaceTitle(f.authority, 'Manual', (await f.workspace()).revision);
  let once = true; f.host(async () => { if (once) { once = false; evidence.transcript[0].text = 'Mutated'; } });
  const result = await f.refresh({ evidence });
  assert.equal(f.calls[0].data.transcript[0].text, dialogue().transcript[0].text); assert.equal(result.record.title, 'Manual');
});

test('conversation short overflow regenerates on ordinary checks and reset clears stale same-text short', async t => {
  const f = await fixture(t, false);
  f.goal(() => ({ goal: 'Shared library', goalShort: '界'.repeat(13), title: 'Library' }));
  const first = await f.refresh({ evidence: dialogue() });
  assert.equal(first.record.goal.short, undefined); assert.equal(first.record.goal.text, 'Shared library');
  f.goal(() => ({ goal: 'Shared library', goalShort: 'Shared library' }));
  assert.equal((await f.refresh({ evidence: dialogue() })).record.goal.short, 'Shared library'); assert.equal(f.calls.length, 2);
  const before = await f.workspace();
  f.goal(() => ({ goal: 'Shared library', goalShort: '界'.repeat(13) }));
  const reset = await f.refresh({ evidence: dialogue(), intent: 'reset' });
  assert.equal(reset.record.goal.text, before.record.goal.text);
  assert.equal(reset.record.goal.short, undefined);
  assert.deepEqual(reset.record.goal.provenance, before.record.goal.provenance);
});

test('unknown and known conversation caches survive refresher replacement without fake document sources', async t => {
  for (const goal of ['', 'Library']) {
    const f = await fixture(t, false); f.goal(() => ({ goal, goalShort: goal }));
    const first = await f.refresh({ evidence: dialogue() });
    const refresher = new PlanRefresher({ store: f.store, options, registry: { find() { throw new Error('Must use persisted cache'); } } });
    const result = await refresher.refresh({ authority: f.authority, evidence: dialogue(), signal: f.controller.signal, assertCurrent: async () => {} });
    assert.equal(result.revision, first.revision);
  }
});

test('selection cache survives source errors but failed goal is never treated as applied', async t => {
  const f = await fixture(t); await writeFile(f.path, Buffer.from([0xc3, 0x28]));
  await assert.rejects(f.refresh(), /Cannot read plan source/);
  assert.deepEqual(f.calls.map(c => c.kind), ['select']);
  await writeFile(f.path, 'Restored'); f.goal(() => { throw new Error('inference failed'); });
  await assert.rejects(f.refresh(), /inference failed/);
  assert.equal((await f.workspace()).record.goal.kind, 'unevaluated');
  f.goal(() => ({ goal: 'Recovered', goalShort: 'Recovered' })); await f.refresh();
  assert.deepEqual(f.calls.map(c => c.kind), ['select', 'goal', 'goal']);
  assert.equal(goalText((await f.workspace()).record.goal), 'Recovered');
});

test('oversized selected source fails visibly without goal request or retry', async t => {
  const f = await fixture(t); await writeFile(f.path, 'x'.repeat(MAX_PLAN_SOURCE_BYTES + 1));
  await assert.rejects(f.refresh(), /256|limit/i);
  assert.deepEqual(f.calls.map(c => c.kind), ['select']); assert.equal(goalText((await f.workspace()).record.goal), '');
});

for (const mutation of ['bytes', 'remove', 'description', 'root', 'shared', 'plan', 'move', 'reset', 'cancel', 'host']) {
  test(`refuses ${mutation} change during goal inference without overwriting concurrent state`, async t => {
    const f = await fixture(t);
    const before = await f.workspace();
    // A prior valid fingerprint matching initial capture cannot prove stability during forced inference.
    const capture = await capturePlanSources(before.record, [f.identity]);
    await f.store.applyPlan(f.authority, { goal: evaluatedGoal('Prior', [f.identity], capture.fingerprint) }, before.revision);
    f.goal(async () => {
      const w = await f.workspace();
      if (mutation === 'bytes') await writeFile(f.path, 'Different after capture');
      if (mutation === 'remove') await f.store.removeReference(f.authority, { workspaceId: 'w', revision: w.revision, identity: referenceIdentity(f.identity) });
      if (mutation === 'description') await f.store.describeReference(f.authority, f.identity, 'Changed', w.revision);
      if (mutation === 'root') {
        const other = join(f.base, 'other'); await mkdir(other); await writeFile(join(other, 'plan.md'), 'Deliver a library and client');
        w.record.checkouts[f.identity.checkout].directory = other;
        await writeFile(w.path, stringify(serializeWorkspaceV1(w.record)));
      }
      if (mutation === 'shared') await f.store.addReferences(f.authority, { issues: [{ url: 'https://example.com/new', description: 'Concurrent' }] });
      if (mutation === 'plan') await f.store.applyPlan(f.authority, { goal: evaluatedGoal('Concurrent plan', [f.identity], capture.fingerprint) }, w.revision);
      if (mutation === 'move') {
        await f.store.createWorkspace({ id: 'other', creationToken: 'other', title: 'Other' });
        await f.store.join({ workspaceId: 'other', sessionId: 's', cwd: f.root, membershipToken: 'new', boundary, title: 'Moved', expected: f.authority });
      }
      if (mutation === 'reset') { const a = await f.store.readAgent('s'); await f.store.resetEvidence(f.authority, boundary, a.revision); }
      if (mutation === 'cancel') f.controller.abort(new Error('cancelled'));
      if (mutation === 'host') f.host(async () => { throw new Error('host invalidated'); });
      return { goal: 'Must not apply', goalShort: 'Must not apply' };
    });
    await assert.rejects(f.refresh({ intent: 'refresh' }));
    assert.notEqual(goalText((await f.workspace()).record.goal), 'Must not apply');
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
  const result = await f.refresh(); assert.equal(result.record.title, 'My title'); assert.equal(goalText(result.record.goal), 'Valid plan goal');
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
  const result = await f.refresh({ intent: 'refresh' });
  assert.deepEqual(result.record.goal.provenance.sources, [other]);
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
      await f.store.applyPlan(f.authority, { goal: evaluatedGoal('Concurrent winner', [f.identity], capture.fingerprint) }, w.revision);
    }
  });
  await assert.rejects(f.refresh(), /revision/i);
  assert.equal(goalText((await f.workspace()).record.goal), 'Concurrent winner');
});

test('host guard after filesystem recheck rejects effects even after valid inference', async t => {
  const f = await fixture(t); let afterInference = false; let guards = 0;
  f.goal(() => { afterInference = true; return { goal: 'Stale result', goalShort: 'Stale result' }; });
  f.host(async () => { if (afterInference && ++guards === 2) throw new Error('host invalidated after recheck'); });
  await assert.rejects(f.refresh(), /after recheck/);
  assert.equal(goalText((await f.workspace()).record.goal), '');
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
  await writeFile(replacement.path, stringify(serializeWorkspaceV1(replacement.record)));
  await writeFile(a.path, stringify(replacementAgent));
  await f.store.addReferences(authority, { documents: [{ ...f.identity, description: 'Implementation plan' }, { ...f.identity, path: 'code.ts', description: 'Not Markdown' }] });
  f.select(() => ({ sources: [f.identity] })); await f.refresh({ authority });
  assert.deepEqual(f.calls.map(c => c.kind), ['select', 'goal', 'select', 'goal']);
});


for (const short of [undefined, '']) test(`${short === undefined ? 'missing' : 'empty'} derived goal short is regenerated without reselecting unchanged source metadata`, async t => {
  const f = await fixture(t); await f.refresh(); const before = await f.workspace();
  const persisted = serializeWorkspaceV1(before.record);
  if (short === undefined) delete persisted.goalShort; else persisted.goalShort = short;
  await writeFile(before.path, stringify(persisted));
  f.calls.length = 0; const result = await f.refresh();
  assert.deepEqual(f.calls.map(c => c.kind), ['goal']);
  assert.equal(result.record.goal.short, 'Deliver library');
});

test('oversized Goal short preserves the full Goal, title and provenance, then regenerates on an ordinary check', async t => {
  const f = await fixture(t); await f.refresh();
  await writeFile(f.path, 'A changed intended workspace purpose');
  const goal = 'Deliver the complete library and client capability with a stable public integration surface';
  f.goal(() => ({ goal, goalShort: '界'.repeat(13), title: 'Library integration' }));
  f.calls.length = 0;
  const events = [];
  const degraded = await f.refresh({ onDiagnostic: event => events.push(event) });
  assert.equal(goalText(degraded.record.goal), goal); assert.equal(degraded.record.title, 'Library integration');
  assert.equal(degraded.record.goal.short, undefined);
  assert.deepEqual(degraded.record.goal.provenance.sources, [f.identity]);
  const capture = await capturePlanSources(degraded.record, [f.identity]);
  assert.equal(degraded.record.goal.provenance.fingerprint, capture.fingerprint);
  assert.deepEqual(f.calls.map(c => c.kind), ['goal']);
  assert.deepEqual(events.at(-1), { stage: 'goal', outcome: 'applied' });
  f.goal(() => ({ goal, goalShort: 'Library integration' }));
  const recovered = await f.refresh();
  assert.equal(recovered.record.goal.short, 'Library integration');
  assert.deepEqual(f.calls.map(c => c.kind), ['goal', 'goal']);
  assert.equal((await f.refresh()).revision, recovered.revision);
  assert.deepEqual(f.calls.map(c => c.kind), ['goal', 'goal']);
});

test('oversized Goal short leaves an unchanged valid short intact rather than overwriting it', async t => {
  const f = await fixture(t); const first = await f.refresh();
  f.goal(() => ({ goal: first.record.goal.text, goalShort: 'x'.repeat(26) }));
  const result = await f.refresh({ intent: 'refresh' });
  assert.equal(result.record.goal.short, first.record.goal.short);
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
  events.length = 0; f.select(() => ({ sources: [] })); await f.refresh({ intent: 'refresh', onDiagnostic });
  assert.deepEqual(events, [
    { stage: 'selection', outcome: 'no_sources', selectedCount: 0, cacheHit: false },
    { stage: 'goal', outcome: 'unchanged' },
  ]);
  assert.equal((await f.workspace()).record.goal.short, 'Deliver library');
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
