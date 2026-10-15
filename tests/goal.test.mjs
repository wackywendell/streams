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
  assert.deepEqual(unknown.goal, { kind: 'unknown', sources: [source], fingerprint: 'bytes' });
  const known = records.parseWorkspaceV1(wire({ goal: '  Deliver library  ', goalShort: '  ', planSources: [source], sourceFingerprint: 'bytes' }));
  assert.deepEqual(known.goal, { kind: 'known', text: '  Deliver library  ', sources: [source], fingerprint: 'bytes' });
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
    { kind: 'unknown', sources: [], fingerprint: 'bytes' },
    { kind: 'unknown', sources: [source], fingerprint: 'bytes', short: '' },
    { kind: 'known', text: ' ', sources: [source], fingerprint: 'bytes' },
    { kind: 'known', text: 'Purpose', sources: [source] },
    { kind: 'unevaluated', sources: [] },
  ]) assert.throws(() => records.validateWorkspace({ ...domain, goal }));
});

test('Goal constructors own safe strings and immutable nonempty provenance', () => {
  const input = [{ ...source }];
  const goal = records.evaluatedGoal('Purpose', input, 'bytes', 'Purpose');
  input[0].path = 'changed.md'; input.push(source);
  assert.deepEqual(goal.sources, [source]);
  assert.ok(Object.isFrozen(goal)); assert.ok(Object.isFrozen(goal.sources)); assert.ok(Object.isFrozen(goal.sources[0]));
  for (const [text, sources, fingerprint, short] of [
    ['Purpose', [], 'bytes'], ['Purpose', [source], null], ['bad\ntext', [source], 'bytes'],
    ['Purpose', [source], 'bytes', 'x'.repeat(26)], ['Purpose', [source], 'bytes', 'bad\nshort'],
  ]) assert.throws(() => records.evaluatedGoal(text, sources, fingerprint, short));
});

test('one Goal transition preserves unchanged derivatives and invalidates selected-source loss', () => {
  const original = records.evaluatedGoal('Purpose', [source], 'bytes', 'Short purpose');
  const same = records.transitionGoal(original, { kind: 'evaluate', goal: records.evaluatedGoal('Purpose', [source], 'changed') });
  assert.equal(same.short, 'Short purpose'); assert.equal(same.fingerprint, 'changed');
  const changed = records.transitionGoal(original, { kind: 'evaluate', goal: records.evaluatedGoal('Other purpose', [source], 'bytes') });
  assert.equal(changed.short, undefined);
  assert.equal(records.transitionGoal(original, { kind: 'source_removed', identity: { ...source, path: 'other.md' } }), original);
  for (const goal of [original, records.evaluatedGoal('', [source], 'bytes')]) {
    assert.deepEqual(records.transitionGoal(goal, { kind: 'source_removed', identity: source }), { kind: 'unevaluated' });
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
    const empty: Goal = { kind: 'known', text: 'Purpose', sources: [], fingerprint: 'bytes' };
    // @ts-expect-error Unknown cannot carry derived purpose.
    const short: Goal = { kind: 'unknown', sources: [{checkout:'repo',path:'plan.md'}], fingerprint:'b', short:'x' };
    const mixed = { kind: 'unknown' as const, sources: [{checkout:'repo',path:'plan.md'}] as const, fingerprint:'b', short:'x' };
    // @ts-expect-error Structural assignment cannot smuggle known fields into unknown.
    const structural: Goal = mixed;
    // @ts-expect-error Known requires text.
    const missing: Goal = { kind: 'known', sources: [{checkout:'repo',path:'plan.md'}], fingerprint:'b' };
    if (workspace.goal.kind === 'known') {
      // @ts-expect-error Goal is readonly.
      workspace.goal.text = 'changed';
      // @ts-expect-error Source identities are readonly.
      workspace.goal.sources[0].path = 'changed';
      // @ts-expect-error Provenance cannot be emptied.
      workspace.goal.sources.pop();
    }`;
  const config = ts.readConfigFile(fileURLToPath(new URL('../tsconfig.json', import.meta.url)), ts.sys.readFile);
  const { options } = ts.parseJsonConfigFileContent(config.config, ts.sys, fileURLToPath(new URL('..', import.meta.url)));
  const host = ts.createCompilerHost(options), getSourceFile = host.getSourceFile.bind(host);
  host.getSourceFile = (path, ...args) => path === filename ? ts.createSourceFile(path, source, options.target, true) : getSourceFile(path, ...args);
  const diagnostics = ts.getPreEmitDiagnostics(ts.createProgram([filename], { ...options, noEmit: true }, host));
  assert.equal(diagnostics.length, 0, diagnostics.map(d => ts.flattenDiagnosticMessageText(d.messageText, '\n')).join('\n'));
});
