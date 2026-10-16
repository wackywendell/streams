import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readFile, writeFile, stat, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import * as records from '../dist/records.js';

const source = { checkout: 'repo', path: 'plan.md' };
const wire = (patch = {}) => ({ version: 1, id: 'w', creationToken: 'c', mutationToken: 'm', title: 'Workspace', titleMode: 'auto', goal: '', planSources: [], sourceFingerprint: null, checkouts: { repo: { directory: '/repo' } }, documents: [{ ...source, description: 'Plan' }], issues: [], prs: [], exclusions: [], ...patch });

test('Goal boundary distinguishes unevaluated, evaluated unknown and established purpose', () => {
  const empty = records.parseWorkspaceV1(wire());
  assert.deepEqual(empty.goal, { kind: 'unevaluated' });
  const unknown = records.parseWorkspaceV1(wire({ goal: '   ', goalShort: '', planSources: [source], sourceFingerprint: 'bytes' }));
  assert.deepEqual(unknown.goal, { kind: 'unknown', provenance: { kind: 'documents', sources: [source], fingerprint: 'bytes' } });
  const known = records.parseWorkspaceV1(wire({ goal: '  Deliver library  ', goalShort: '  ', planSources: [source], sourceFingerprint: 'bytes' }));
  assert.deepEqual(known.goal, { kind: 'known', text: '  Deliver library  ', provenance: { kind: 'documents', sources: [source], fingerprint: 'bytes' } });
  for (const record of [empty, unknown, known]) {
    assert.deepEqual(records.parseWorkspaceV1(records.serializeWorkspaceV1(record)), record);
    assert.throws(() => records.validateWorkspace(records.serializeWorkspaceV1(record)));
    assert.throws(() => records.parseWorkspaceV1(record));
    assert.equal(Object.hasOwn(record, 'planSources'), false);
  }
});

test('wire validation precedes blank normalization and retains cross-record constraints', () => {
  for (const patch of [
    { goal: ' ' }, { goalShort: 'No purpose' }, { goalShort: 'x'.repeat(26) },
    { goal: 'bad\ntext' }, { planSources: [source] }, { sourceFingerprint: 'orphan' },
    { planSources: [source], sourceFingerprint: '' },
    { planSources: [source, source], sourceFingerprint: 'bytes' },
    { planSources: [{ ...source, path: 'absent.md' }], sourceFingerprint: 'bytes' },
    { planSources: [{ ...source, checkout: 'unknown' }], sourceFingerprint: 'bytes' },
    { goal: { kind: 'unevaluated' } }, { extra: true },
  ]) assert.throws(() => records.parseWorkspaceV1(wire(patch)), JSON.stringify(patch));
  const domain = records.parseWorkspaceV1(wire());
  for (const goal of [
    { kind: 'unknown', provenance: { kind: 'documents', sources: [], fingerprint: 'bytes' } },
    { kind: 'unknown', provenance: { kind: 'documents', sources: [source], fingerprint: 'bytes' }, short: '' },
    { kind: 'known', text: ' ', provenance: { kind: 'documents', sources: [source], fingerprint: 'bytes' } },
    { kind: 'known', text: 'Purpose', provenance: { kind: 'documents', sources: [source] } },
    { kind: 'unevaluated', sources: [] },
  ]) assert.throws(() => records.validateWorkspace({ ...domain, goal }));
});

test('Goal constructors own safe strings and immutable nonempty provenance', () => {
  const input = [{ ...source }];
  const goal = records.evaluatedGoal('Purpose', input, 'bytes', 'Purpose');
  input[0].path = 'changed.md'; input.push(source);
  assert.deepEqual(goal.provenance.sources, [source]);
  assert.ok(Object.isFrozen(goal)); assert.ok(Object.isFrozen(goal.provenance)); assert.ok(Object.isFrozen(goal.provenance.sources)); assert.ok(Object.isFrozen(goal.provenance.sources[0]));
  for (const [text, sources, fingerprint, short] of [
    ['Purpose', [], 'bytes'], ['Purpose', [source], null], ['bad\ntext', [source], 'bytes'],
    ['Purpose', [source], 'bytes', 'x'.repeat(26)], ['Purpose', [source], 'bytes', 'bad\nshort'],
  ]) assert.throws(() => records.evaluatedGoal(text, sources, fingerprint, short));
});

test('one Goal transition preserves unchanged derivatives and invalidates selected-source loss', () => {
  const original = records.evaluatedGoal('Purpose', [source], 'bytes', 'Short purpose');
  const same = records.transitionGoal(original, { kind: 'evaluate', goal: records.evaluatedGoal('Purpose', [source], 'changed') });
  assert.equal(same.short, 'Short purpose'); assert.equal(same.provenance.fingerprint, 'changed');
  const changed = records.transitionGoal(original, { kind: 'evaluate', goal: records.evaluatedGoal('Other purpose', [source], 'bytes') });
  assert.equal(changed.short, undefined);
  assert.equal(records.transitionGoal(original, { kind: 'source_removed', identity: { ...source, path: 'other.md' } }), original);
  for (const goal of [original, records.evaluatedGoal('', [source], 'bytes')]) {
    assert.deepEqual(records.transitionGoal(goal, { kind: 'source_removed', identity: source }), { kind: 'unevaluated' });
  }
});

test('conversation Goal transitions preserve unchanged short text and invalidate derivatives on changed purpose', () => {
  const origin = { sessionId: 's', digest: 'a'.repeat(64) };
  const original = records.conversationGoal('Purpose', origin, 'Short');
  const same = records.transitionGoal(original, { kind: 'evaluate', goal: records.conversationGoal('Purpose', { ...origin, sessionId: 'other' }) });
  assert.equal(same.short, 'Short'); assert.equal(same.provenance.sessionId, 'other');
  const changed = records.transitionGoal(original, { kind: 'evaluate', goal: records.conversationGoal('New purpose', origin) });
  assert.equal(changed.short, undefined);
});

test('explicit evaluation policy disables same-purpose short inheritance without changing Goal storage', () => {
  for (const make of [
    (short) => records.evaluatedGoal('Purpose', [source], 'bytes', short),
    (short) => records.conversationGoal('Purpose', { sessionId: 's', digest: 'a'.repeat(64) }, short),
  ]) {
    const previous = make('Stale purpose');
    const proposal = make();
    assert.equal(records.transitionGoal(previous, { kind: 'evaluate', goal: proposal }).short, 'Stale purpose');
    assert.equal(records.transitionGoal(previous, { kind: 'evaluate', goal: proposal, preserveShort: true }).short, 'Stale purpose');
    assert.deepEqual(records.transitionGoal(previous, { kind: 'evaluate', goal: proposal, preserveShort: false }), proposal);
    assert.equal(records.transitionGoal(previous, { kind: 'evaluate', goal: make('Fresh purpose'), preserveShort: false }).short, 'Fresh purpose');
    assert.equal(Object.hasOwn(proposal, 'preserveShort'), false);
  }
});

test('normalized legacy storage remains byte-identical on reads and no-op writes', async t => {
  const root = await mkdtemp(join(tmpdir(), 'goal-boundary-')); t.after(() => rm(root, { recursive: true, force: true }));
  const store = new records.RecordStore(join(root, 'records'));
  await store.createWorkspace({ id: 'w', creationToken: 'c', title: 'Workspace' });
  const cwd = join(root, 'repo'); await mkdir(cwd);
  const agent = await store.join({ workspaceId: 'w', sessionId: 's', membershipToken: 'member', boundary: { compactionDigest: 'd', transcriptLength: 0, prefixDigest: 'p' }, cwd, title: 'Task' });
  const auth = records.agentAuthority(agent.record);
  const identity = { checkout: agent.record.checkout, path: 'plan.md' };
  let workspace = await store.addReferences(auth, { documents: [{ ...identity, description: 'Plan' }] });
  const persisted = records.serializeWorkspaceV1(workspace.record);
  const bytes = JSON.stringify({ ...persisted, goal: '  ', goalShort: '', planSources: [identity], sourceFingerprint: 'bytes' }, null, 4);
  await writeFile(workspace.path, bytes);
  workspace = await store.readWorkspace('w'); const mtime = (await stat(workspace.path, { bigint: true })).mtimeNs;
  const noop = await store.applyPlan(auth, { goal: workspace.record.goal }, workspace.revision);
  assert.deepEqual(noop, workspace); assert.equal(await readFile(workspace.path, 'utf8'), bytes);
  assert.equal((await stat(workspace.path, { bigint: true })).mtimeNs, mtime);
  const changed = await store.setWorkspaceTitle(auth, 'Manual', workspace.revision);
  const canonical = JSON.parse(JSON.stringify(records.serializeWorkspaceV1(changed.record)));
  assert.equal(canonical.goal, ''); assert.equal(Object.hasOwn(canonical, 'goalShort'), false);
  assert.deepEqual(canonical.planSources, [identity]);
});

test('conversation Goal has attributed immutable provenance, flat round trips and document-independent removal', () => {
  const provenance = { sessionId: 'session', digest: 'a'.repeat(64) };
  for (const purpose of ['', 'Build a shared library']) {
    const goal = records.conversationGoal(purpose, provenance, purpose ? 'Shared library' : '');
    provenance.sessionId = 'changed';
    assert.equal(goal.provenance.kind, 'conversation');
    assert.ok(Object.isFrozen(goal.provenance));
    assert.equal(goal.provenance.sessionId, purpose ? 'changed' : 'session');
    const domain = records.validateWorkspace({ ...records.parseWorkspaceV1(wire()), goal });
    const persisted = records.serializeWorkspaceV1(domain);
    assert.deepEqual(persisted.planSources, []); assert.equal(persisted.sourceFingerprint, null);
    assert.deepEqual(persisted.conversationProvenance, { sessionId: goal.provenance.sessionId, digest: provenance.digest });
    assert.deepEqual(records.parseWorkspaceV1(persisted), domain);
    assert.equal(records.transitionGoal(goal, { kind: 'source_removed', identity: source }), goal);
    for (const patch of [
      { planSources: [source], sourceFingerprint: 'bytes' },
      { sourceFingerprint: 'orphan' },
      { conversationProvenance: { digest: provenance.digest } },
      { conversationProvenance: { sessionId: 's', digest: '' } },
      { conversationProvenance: { sessionId: 's', digest: 'not-a-digest' } },
      { conversationProvenance: null },
    ]) assert.throws(() => records.parseWorkspaceV1({ ...persisted, ...patch }));
    for (const invalid of [
      { ...goal, provenance: null },
      { ...goal, provenance: { ...goal.provenance, sources: [source] } },
      { ...goal, sources: [source], fingerprint: 'bytes' },
    ]) assert.throws(() => records.parseGoal(invalid));
  }
});

test('Goal TypeScript API forbids invalid structural combinations and mutation', () => {
  const filename = fileURLToPath(new URL('../src/goal-typecheck.ts', import.meta.url));
  const source = `import type { Goal, WorkspaceRecord } from './records.js';
    declare const workspace: WorkspaceRecord;
    // @ts-expect-error Goal is not raw text.
    workspace.goal = 'purpose';
    // @ts-expect-error No parallel provenance on domain records.
    workspace.planSources;
    // @ts-expect-error Unknown requires provenance.
    const absent: Goal = { kind: 'unknown' };
    // @ts-expect-error Known requires nonempty source tuple.
    const empty: Goal = { kind: 'known', text: 'Purpose', provenance: { kind: 'documents', sources: [], fingerprint: 'bytes' } };
    // @ts-expect-error Unknown cannot carry derived purpose.
    const short: Goal = { kind: 'unknown', provenance: { kind: 'conversation', sessionId: 's', digest: 'b' }, short:'x' };
    const mixed = { kind: 'unknown' as const, provenance: { kind: 'documents' as const, sources: [{checkout:'repo',path:'plan.md'}] as const, fingerprint:'b' }, short:'x' };
    // @ts-expect-error Structural assignment cannot smuggle known fields into unknown.
    const structural: Goal = mixed;
    // @ts-expect-error Known requires text.
    const missing: Goal = { kind: 'known', provenance: { kind: 'conversation', sessionId: 's', digest: 'b' } };
    const mixedOrigin = { kind: 'known' as const, text: 'Purpose', provenance: { kind: 'conversation' as const, sessionId: 's', digest: 'b', sources: [{checkout:'repo',path:'plan.md'}] as const } };
    // @ts-expect-error Conversation cannot structurally carry document identities.
    const invalidOrigin: Goal = mixedOrigin;
    if (workspace.goal.kind === 'known' && workspace.goal.provenance.kind === 'documents') {
      // @ts-expect-error Goal is readonly.
      workspace.goal.text = 'changed';
      // @ts-expect-error Source identities are readonly.
      workspace.goal.provenance.sources[0].path = 'changed';
      // @ts-expect-error Provenance cannot be emptied.
      workspace.goal.provenance.sources.pop();
    }`;
  const config = ts.readConfigFile(fileURLToPath(new URL('../tsconfig.json', import.meta.url)), ts.sys.readFile);
  const { options } = ts.parseJsonConfigFileContent(config.config, ts.sys, fileURLToPath(new URL('..', import.meta.url)));
  const host = ts.createCompilerHost(options), getSourceFile = host.getSourceFile.bind(host);
  host.getSourceFile = (path, ...args) => path === filename ? ts.createSourceFile(path, source, options.target, true) : getSourceFile(path, ...args);
  const diagnostics = ts.getPreEmitDiagnostics(ts.createProgram([filename], { ...options, noEmit: true }, host));
  assert.equal(diagnostics.length, 0, diagnostics.map(d => ts.flattenDiagnosticMessageText(d.messageText, '\n')).join('\n'));
});
