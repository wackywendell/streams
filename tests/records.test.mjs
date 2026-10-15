import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, rm, writeFile, readFile, stat, symlink, open, readdir, realpath, rename } from 'node:fs/promises';
import fsPromises from 'node:fs/promises';
import { syncBuiltinESMExports } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { randomUUID, createHash } from 'node:crypto';
import { spawn, spawnSync } from 'node:child_process';
import ts from 'typescript';
import { fileURLToPath } from 'node:url';
import { evaluatedGoal, unevaluatedGoal, serializeWorkspaceV1, goalText, RecordStore, validateWorkspace, validateAgent, applyOwnUpdate, applySharedAdditions, discoverCheckout, referenceIdentity, resolveWorkspaceDocument, applyReferenceRemoval } from '../dist/records.js';
import { composeView, checkoutLabels, selectReference } from '../dist/presentation.js';

const boundary = { compactionDigest: 'compact', transcriptLength: 2, prefixDigest: 'prefix' };
async function fixture(t) {
  const dir = await realpath(await mkdtemp(join(tmpdir(), 'records-')));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const cwd = join(dir, 'repo'); await mkdir(join(cwd, '.git'), { recursive: true });
  const store = new RecordStore(join(dir, 'state'));
  const workspace = await store.createWorkspace({ id: randomUUID(), creationToken: randomUUID(), title: 'Shared' });
  return { dir, cwd, store, workspace };
}
function joinInput(workspace, cwd, sessionId = '../custom/session') {
  return { workspaceId: workspace.record.id, sessionId, cwd, boundary, membershipToken: randomUUID(), title: 'Own assignment' };
}
const authority = a => ({ sessionId: a.record.sessionId, workspaceId: a.record.workspaceId, membershipToken: a.record.membershipToken, evidenceGeneration: a.record.evidenceGeneration });
async function retry(fn) { for (let n = 0; ; n++) { try { return await fn(); } catch (e) { if (!/lock exists/.test(e.message) || n > 100) throw e; await new Promise(r => setTimeout(r, 2)); } } }

for (const field of ['sessionId', 'workspaceId', 'membershipToken', 'checkout', 'cwd']) test(`checked membership rejects ${field} changes during paired read`, async t => {
  const { store, workspace, cwd } = await fixture(t);
  const agent = await store.join(joinInput(workspace, cwd));
  const readWorkspace = store.readWorkspace.bind(store);
  store.readWorkspace = async id => {
    const result = await readWorkspace(id);
    await writeFile(agent.path, JSON.stringify({ ...agent.record, [field]: field === 'cwd' ? join(cwd, 'subdir') : 'changed' }));
    return result;
  };
  await assert.rejects(store.readMembership(agent.record.sessionId), /membership|identity/i);
});

for (const expected of [false, true]) for (const change of ['progress', 'evidence']) test(`checked membership ${expected ? 'effect' : 'display'} read handles ${change} changes`, async t => {
  const { store, workspace, cwd } = await fixture(t);
  const agent = await store.join(joinInput(workspace, cwd));
  let latest;
  const readWorkspace = store.readWorkspace.bind(store);
  store.readWorkspace = async id => {
    const result = await readWorkspace(id);
    latest = change === 'progress'
      ? await store.updateOwn(authority(agent), { now: 'Latest progress' }, agent.revision)
      : await store.resetEvidence(authority(agent), boundary, agent.revision);
    return result;
  };
  const read = store.readMembership(agent.record.sessionId, expected ? authority(agent) : undefined);
  if (expected && change === 'evidence') await assert.rejects(read, /evidence generation/i);
  else {
    const result = await read;
    assert.deepEqual(result.agent, latest);
    assert.equal(result.workspace.record.id, workspace.record.id);
  }
});

test('checked membership rejects stale initial authority and malformed or mismatched durable pairs', async t => {
  const { store, workspace, cwd } = await fixture(t);
  const agent = await store.join(joinInput(workspace, cwd));
  for (const patch of [{ sessionId: 'other' }, { workspaceId: 'other' }, { membershipToken: 'other' }, { evidenceGeneration: 'other' }, { extra: 'unknown' }]) {
    await assert.rejects(store.readMembership(agent.record.sessionId, { ...authority(agent), ...patch }));
  }
  for (const patch of [{ checkout: 'unknown' }, { cwd: '/outside' }, { now: 42 }]) {
    await writeFile(agent.path, JSON.stringify({ ...agent.record, ...patch }));
    await assert.rejects(store.readMembership(agent.record.sessionId), /membership|checkout|outside|text/i);
  }
  await writeFile(agent.path, JSON.stringify(agent.record));
  const current = await store.readWorkspace(workspace.record.id);
  await writeFile(current.path, JSON.stringify({ ...serializeWorkspaceV1(current.record), id: 'other' }));
  await assert.rejects(store.readMembership(agent.record.sessionId), /identity/i);
});

test('reserved creation is idempotent only for matching creation tokens; custom sessions are hashed', async t => {
  const { store, workspace, cwd } = await fixture(t);
  assert.deepEqual(await store.createWorkspace({ id: workspace.record.id, creationToken: workspace.record.creationToken, title: 'Ignored retry' }), workspace);
  await assert.rejects(store.createWorkspace({ id: workspace.record.id, creationToken: 'other', title: 'No' }), /token|conflict/);
  const input = joinInput(workspace, cwd); const a = await store.join(input);
  assert.match(a.path, new RegExp(createHash('sha256').update(input.sessionId).digest('hex') + '\\.yaml$'));
  assert.deepEqual(await store.join(input), a);
  assert.equal(Object.keys((await store.readWorkspace(workspace.record.id)).record.checkouts).length, 1);
  assert.equal(a.record.displayAttempt, undefined);
});

test('strict schemas and helper authority preserve manual titles and existing reference classification', async t => {
  const { store, workspace, cwd } = await fixture(t); const a = await store.join(joinInput(workspace, cwd));
  const w = (await store.readWorkspace(workspace.record.id)).record;
  for (const patch of [{ extra: 1 }, { version: 2 }, { title: 'bad\n' }, { documents: [{ checkout: a.record.checkout, path: '../bad', description: '' }] }]) assert.throws(() => validateWorkspace({ ...w, ...patch }));
  for (const patch of [{ sessionId: 'x' }, { cwd: '/x' }, { membershipToken: 'x' }, { documents: [] }]) assert.throws(() => applyOwnUpdate(a.record, patch));
  assert.throws(() => validateAgent({ ...a.record, boundary: { ...boundary, transcriptLength: -1 } }));
  assert.equal(applyOwnUpdate({ ...a.record, titleMode: 'manual' }, { title: 'Auto', now: 'Done' }).title, a.record.title);
  assert.throws(() => applySharedAdditions(w, { checkouts: {} }));
  const first = applySharedAdditions(w, { issues: [{ url: 'https://example.com/a', description: 'original' }] });
  const second = applySharedAdditions(first, { prs: [{ url: 'https://example.com/a', description: 'replacement' }] });
  assert.deepEqual(second, first);
});

test('concurrent joins and shared additions merge; removal exclusions persist until explicit re-add', async t => {
  const { store, workspace, cwd } = await fixture(t);
  const agents = await Promise.all(['one', 'two'].map(id => retry(() => store.join(joinInput(workspace, cwd, id)))));
  const summaries = await Promise.all(agents.map((a, i) => retry(() => store.updateOwn(authority(a), { now: `Agent ${i}`, status: i ? 'waiting' : 'active' }, a.revision))));
  assert.deepEqual(summaries.map(a => a.record.now), ['Agent 0', 'Agent 1']);
  assert.equal((await store.readAgent('one')).record.status, 'active'); assert.equal((await store.readAgent('two')).record.status, 'waiting');
  await Promise.all(agents.map((a, i) => retry(() => store.addReferences(authority(a), { issues: [{ url: `https://example.com/${i}`, description: '' }] }))));
  let w = await store.readWorkspace(workspace.record.id); assert.equal(w.record.issues.length, 2);
  const selection = selectReference(w, '/0'); assert.equal(selection.kind, 'unique');
  await store.removeReference(authority(agents[0]), selection.selection);
  await store.addReferences(authority(agents[1]), { issues: [{ url: 'https://example.com/0', description: 'old evidence' }] });
  w = await store.readWorkspace(workspace.record.id); assert.equal(w.record.issues.length, 1);
  await assert.rejects(store.removeReference(authority(agents[0]), selection.selection), /revision|stale/i);
  await store.addReferences(authority(agents[0]), { prs: [{ url: 'https://example.com/0', description: 'explicit' }] }, { explicit: true });
  w = await store.readWorkspace(workspace.record.id); assert.equal(w.record.prs.length, 1); assert.equal(w.record.exclusions.length, 0);
});

test('moving preserves own work, never copies refs; stale membership and ABA evidence are refused', async t => {
  const { store, workspace, cwd } = await fixture(t); let a = await store.join(joinInput(workspace, cwd)); const old = authority(a);
  a = await store.updateOwn(old, { now: 'Current work', remaining: 'Next', statusShort: 'Work in progress' }, a.revision);
  await store.addReferences(old, { issues: [{ url: 'https://example.com/source', description: '' }] });
  const destination = await store.createWorkspace({ id: randomUUID(), creationToken: randomUUID(), title: 'Destination' });
  a = await store.join({ ...joinInput(destination, cwd), expected: old });
  assert.equal(a.record.now, 'Current work'); assert.equal(a.record.statusShort, 'Work in progress'); assert.equal((await store.readWorkspace(destination.record.id)).record.issues.length, 0);
  await assert.rejects(store.addReferences(old, {}), /membership|workspace|stale/i);
  const current = authority(a); a = await store.resetEvidence(current, boundary, a.revision);
  await assert.rejects(store.addReferences(current, {}), /generation|stale/i);
  await assert.rejects(store.join({ ...joinInput(workspace, cwd), expected: old }), /membership|workspace|stale/i);
  a = await store.join({ ...joinInput(workspace, cwd), membershipToken: old.membershipToken, expected: authority(a) });
  assert.notEqual(a.record.evidenceGeneration, old.evidenceGeneration);
  await assert.rejects(store.addReferences(old, {}), /generation|stale/i);
});

test('topology deletion refuses any agent and checkout references/exclusions; own deletion explicit', async t => {
  const { store, workspace, cwd } = await fixture(t); const a = await store.join(joinInput(workspace, cwd));
  let w = await store.readWorkspace(workspace.record.id);
  await assert.rejects(store.deleteWorkspace(w.record.id, w.revision), /agent|refer/);
  await assert.rejects(store.removeCheckout(authority(a), a.record.checkout, w.revision), /used|agent/);
  await store.deleteOwn(authority(a), a.revision);
  w = await store.readWorkspace(workspace.record.id); await store.deleteWorkspace(w.record.id, w.revision);
  await assert.rejects(store.readWorkspace(w.record.id), { code: 'ENOENT' });
});

test('checkout discovery deduplicates aliases but preserves worktrees and actual subdirectories', async t => {
  const { store, workspace, cwd, dir } = await fixture(t);
  await mkdir(join(cwd, 'sub')); await symlink(cwd, join(dir, 'alias'));
  assert.equal(await discoverCheckout(join(cwd, 'sub')), cwd);
  const a = await store.join(joinInput(workspace, join(cwd, 'sub'), 'a'));
  const b = await store.join(joinInput(workspace, join(dir, 'alias'), 'b')); assert.equal(a.record.checkout, b.record.checkout);
  assert.equal(a.record.cwd, join(cwd, 'sub'));
  const tree = join(dir, 'tree'); await mkdir(tree); await writeFile(join(tree, '.git'), 'gitdir: elsewhere');
  const c = await store.join(joinInput(workspace, tree, 'c')); assert.notEqual(a.record.checkout, c.record.checkout);
  const jj = join(cwd, 'nested'); await mkdir(join(jj, '.jj'), { recursive: true }); await mkdir(join(jj, 'sub'));
  assert.equal(await discoverCheckout(join(jj, 'sub')), jj);
  const outside = join(dir, 'outside-repo'); await mkdir(outside); assert.equal(await discoverCheckout(outside), outside);
});

test('literal unique selection ignores descriptions, supports broken files, and carries revision identity', async t => {
  const { store, workspace, cwd } = await fixture(t); const a = await store.join(joinInput(workspace, cwd));
  await store.addReferences(authority(a), { documents: [{ checkout: a.record.checkout, path: 'missing.md', description: 'ONLY_DESCRIPTION' }], issues: [{ url: 'https://example.com/missing', description: '' }] });
  const w = await store.readWorkspace(workspace.record.id);
  assert.equal(selectReference(w, '').kind, 'empty'); assert.equal(selectReference(w, 'ONLY_DESCRIPTION').kind, 'none'); assert.equal(selectReference(w, 'missing').kind, 'ambiguous');
  const selected = selectReference(w, 'missing.md'); assert.equal(selected.kind, 'unique');
  assert.equal(selected.selection.identity, referenceIdentity(w.record.documents[0]));
  const removed = applyReferenceRemoval(w, selected.selection);
  assert.equal(removed.documents.length, 0); assert.equal(w.record.documents.length, 1);
  assert.throws(() => applyReferenceRemoval({ ...w, revision: 'other' }, selected.selection), /revision|stale/i);
  assert.equal(selectReference(w, 'MISSING.md').kind, 'none');
  await store.removeReference(authority(a), selected.selection);
  await assert.rejects(store.removeCheckout(authority(a), a.record.checkout, (await store.readWorkspace(w.record.id)).revision), /used|agent|exclusion/);
});

test('raw revision and no-op preserve comments/mtime; cancellation at publication preserves state', async t => {
  const { store, workspace, cwd } = await fixture(t); const a = await store.join(joinInput(workspace, cwd));
  await writeFile(a.path, '# comment\n' + await readFile(a.path, 'utf8')); const current = await store.readAgent(a.record.sessionId);
  const before = await readFile(a.path); const mtime = (await stat(a.path, { bigint: true })).mtimeNs;
  assert.equal(current.revision, createHash('sha256').update(before).digest('hex'));
  await store.updateOwn(authority(a), {}, current.revision); assert.deepEqual(await readFile(a.path), before); assert.equal((await stat(a.path, { bigint: true })).mtimeNs, mtime);
  const handle = await open(a.path, 'r'); const proto = Object.getPrototypeOf(handle); const sync = proto.sync; await handle.close();
  const controller = new AbortController(); t.mock.method(proto, 'sync', async function () { await sync.call(this); controller.abort(); });
  await assert.rejects(store.updateOwn(authority(a), { now: 'Cancelled' }, current.revision, controller.signal), { name: 'AbortError' });
  assert.deepEqual(await readFile(a.path), before); assert.equal((await readdir(join(store.directory, 'agents'))).some(n => /tmp|lock/.test(n)), false);
});

test('interrupted join leaves only a valid unused checkout and retry completes membership', async t => {
  const { store, workspace, cwd } = await fixture(t); const input = joinInput(workspace, cwd);
  const agentPath = store.agentPath(input.sessionId); await writeFile(agentPath + '.lock', 'owner');
  await assert.rejects(store.join(input), /lock exists/); await rm(agentPath + '.lock');
  const a = await store.join(input); assert.equal(a.record.workspaceId, workspace.record.id);
  assert.equal(Object.keys((await store.readWorkspace(workspace.record.id)).record.checkouts).length, 1);
});

test('record reads reject symlinks and FIFO without blocking', async t => {
  const { store, workspace, dir } = await fixture(t); const path = workspace.path;
  const bytes = await readFile(path); await rm(path); await writeFile(join(dir, 'target'), bytes); await symlink(join(dir, 'target'), path);
  await assert.rejects(store.readWorkspace(workspace.record.id), /regular|symlink|ELOOP/); await rm(path);
  assert.equal(spawnSync('mkfifo', [path]).status, 0);
  const code = `import { RecordStore } from ${JSON.stringify(new URL('../dist/records.js', import.meta.url).href)}; import assert from 'node:assert/strict'; await assert.rejects(new RecordStore(${JSON.stringify(store.directory)}).readWorkspace(${JSON.stringify(workspace.record.id)}), /regular/);`;
  const child = spawnSync(process.execPath, ['--input-type=module', '-e', code], { timeout: 3000, encoding: 'utf8' }); assert.equal(child.status, 0, child.stderr || child.error?.message);
});

test('pure composition validates membership and qualifies duplicate basenames/root/ancestor edges', async t => {
  const { store, workspace, cwd } = await fixture(t); const a = await store.join(joinInput(workspace, cwd)); let w = await store.readWorkspace(workspace.record.id);
  await store.addReferences(authority(a), { documents: [{ checkout: a.record.checkout, path: 'docs/plan.md', description: '' }] }); w = await store.readWorkspace(workspace.record.id);
  const view = composeView(w, a); assert.equal(view.documents[0].label, 'docs/plan.md'); assert.equal(view.documents[0].target, join(cwd, 'docs/plan.md'));
  assert.throws(() => composeView(w, { ...a, record: { ...a.record, workspaceId: randomUUID() } }), /membership|workspace/);
  const labels = checkoutLabels({ a: { directory: '/backend/common' }, b: { directory: '/frontend/common' }, c: { directory: '/' }, d: { directory: '/backend' } });
  assert.equal(labels.a, 'backend/common'); assert.equal(labels.b, 'frontend/common'); assert.equal(labels.c, '/'); assert.equal(new Set(Object.values(labels)).size, 4);
});

test('boundary resets generate fresh tokens even when returning to identical prefix evidence', async t => {
  const { store, workspace, cwd } = await fixture(t); let a = await store.join(joinInput(workspace, cwd));
  const first = authority(a);
  a = await store.resetEvidence(first, { ...boundary, transcriptLength: 3 }, a.revision);
  const second = authority(a);
  a = await store.resetEvidence(second, boundary, a.revision);
  assert.notEqual(a.record.evidenceGeneration, first.evidenceGeneration);
  assert.notEqual(a.record.evidenceGeneration, second.evidenceGeneration);
  await assert.rejects(store.addReferences(first, {}), /generation|stale/);
  await assert.rejects(store.addReferences(second, {}), /generation|stale/);
});

test('interruption after checkout publication cannot publish a dangling agent and retry reuses checkout', async t => {
  const { store, workspace, cwd } = await fixture(t); const input = joinInput(workspace, cwd);
  const handle = await open(workspace.path, 'r'); const proto = Object.getPrototypeOf(handle); const sync = proto.sync; await handle.close();
  const controller = new AbortController(); let writes = 0;
  const mock = t.mock.method(proto, 'sync', async function () { await sync.call(this); if (++writes === 2) controller.abort(); });
  await assert.rejects(store.join(input, controller.signal), { name: 'AbortError' }); mock.mock.restore();
  assert.equal(Object.keys((await store.readWorkspace(workspace.record.id)).record.checkouts).length, 1);
  await assert.rejects(store.readAgent(input.sessionId), { code: 'ENOENT' });
  const a = await store.join(input); assert.equal(a.record.workspaceId, workspace.record.id);
  assert.equal(Object.keys((await store.readWorkspace(workspace.record.id)).record.checkouts).length, 1);
});

test('join versus deletion is serialized without dangling membership; concurrent remove/add cannot resurrect exclusions', async t => {
  const { store, workspace, cwd } = await fixture(t); const input = joinInput(workspace, cwd);
  await Promise.allSettled([retry(() => store.join(input)), retry(() => store.deleteWorkspace(workspace.record.id, workspace.revision))]);
  const agents = await store.listAgents(); const workspaces = await store.listWorkspaces();
  for (const a of agents) assert(workspaces.some(w => w.record.id === a.record.workspaceId && w.record.checkouts[a.record.checkout]));
  if (!agents.length) {
    assert.equal(workspaces.length, 0);
    await store.createWorkspace({ id: workspace.record.id, creationToken: workspace.record.creationToken, title: 'Shared' });
    agents.push(await store.join(input));
  }
  const a = agents[0]; await store.addReferences(authority(a), { issues: [{ url: 'https://example.com/race', description: '' }] });
  const selected = selectReference(await store.readWorkspace(a.record.workspaceId), '/race').selection;
  await Promise.all([retry(() => store.removeReference(authority(a), selected)), retry(() => store.addReferences(authority(a), { issues: [{ url: 'https://example.com/race', description: 'ignored overwrite' }] }))]);
  const w = await store.readWorkspace(a.record.workspaceId); assert.equal(w.record.issues.length, 0); assert.equal(w.record.exclusions.length, 1);
});

test('exclusions alone prevent checkout deletion; fresh registration never intentionally recycles a key', async t => {
  const { store, workspace, cwd, dir } = await fixture(t); const a = await store.join(joinInput(workspace, cwd));
  const other = join(dir, 'other'); await mkdir(other);
  let w = await store.registerCheckout(authority(a), other, (await store.readWorkspace(workspace.record.id)).revision);
  const key = Object.keys(w.record.checkouts).find(k => k !== a.record.checkout);
  w = await store.addReferences(authority(a), { documents: [{ checkout: key, path: 'broken', description: '' }] });
  w = await store.removeReference(authority(a), selectReference(w, 'broken').selection);
  await assert.rejects(store.removeCheckout(authority(a), key, w.revision), /exclusion/);
  w = await store.addReferences(authority(a), { documents: [{ checkout: key, path: 'broken', description: '' }] }, { explicit: true });
  await assert.rejects(store.removeCheckout(authority(a), key, w.revision), /reference/);
  const unused = join(dir, 'unused'); await mkdir(unused); w = await store.registerCheckout(authority(a), unused, w.revision);
  const unusedKey = Object.keys(w.record.checkouts).find(k => w.record.checkouts[k].directory === unused);
  w = await store.removeCheckout(authority(a), unusedKey, w.revision);
  w = await store.registerCheckout(authority(a), unused, w.revision);
  assert(!Object.hasOwn(w.record.checkouts, unusedKey));
});

test('document opening rejects escapes, symlink relocation, non-files and unknown checkout roots', async t => {
  const { store, workspace, cwd, dir } = await fixture(t); const a = await store.join(joinInput(workspace, cwd));
  const w = (await store.readWorkspace(workspace.record.id)).record;
  await writeFile(join(dir, 'outside'), 'outside'); await symlink(join(dir, 'outside'), join(cwd, 'escape')); await writeFile(join(cwd, 'inside'), 'inside');
  assert.equal(await resolveWorkspaceDocument(w, { checkout: a.record.checkout, path: 'inside' }), join(cwd, 'inside'));
  await assert.rejects(resolveWorkspaceDocument(w, { checkout: a.record.checkout, path: 'escape' }), /outside/);
  await assert.rejects(resolveWorkspaceDocument(w, { checkout: a.record.checkout, path: '.git' }), /regular/);
  await assert.rejects(resolveWorkspaceDocument(w, { checkout: 'missing', path: 'inside' }), /checkout/);
  await assert.rejects(resolveWorkspaceDocument(w, { checkout: a.record.checkout, path: '../outside' }), /path/);
  await rename(cwd, join(dir, 'old-root')); await symlink(join(dir, 'old-root'), cwd);
  await assert.rejects(resolveWorkspaceDocument(w, { checkout: a.record.checkout, path: 'inside' }), /canonical directory changed/);
});

test('foreground titles/descriptions and plan provenance obey CAS and helper permissions', async t => {
  const { store, workspace, cwd } = await fixture(t); let a = await store.join(joinInput(workspace, cwd)); const auth = authority(a);
  a = await store.setOwnTitle(auth, 'Manual own', a.revision); a = await store.updateOwn(auth, { title: 'Ignored auto' }, a.revision); assert.equal(a.record.title, 'Manual own');
  let w = await store.readWorkspace(workspace.record.id); w = await store.setWorkspaceTitle(auth, 'Manual shared', w.revision);
  const doc = { checkout: a.record.checkout, path: 'plan.md', description: 'first' };
  w = await store.addReferences(auth, { documents: [doc] });
  w = await store.describeReference(auth, { checkout: doc.checkout, path: doc.path }, 'foreground', w.revision);
  w = await store.addReferences(auth, { documents: [{ ...doc, description: 'ignored helper' }] }); assert.equal(w.record.documents[0].description, 'foreground');
  const source = { checkout: doc.checkout, path: doc.path };
  w = await store.applyPlan(auth, { goal: evaluatedGoal('Plan goal', [source], 'bytes-and-identities'), title: 'Ignored auto' }, w.revision);
  assert.equal(w.record.title, 'Manual shared'); assert.equal(composeView(w, a).goal, 'Plan goal');
  await assert.rejects(store.applyPlan(auth, { goal: { kind: 'known', text: 'No provenance', sources: [], fingerprint: 'x' } }, w.revision), /provenance/);
  w = await store.removeReference(auth, selectReference(w, 'plan.md').selection); assert.equal(goalText(w.record.goal), ''); assert.equal(w.record.goal.kind, 'unevaluated');
});

test('applyPlan accepts an unevaluated Goal in its TypeScript API', () => {
  const filename = fileURLToPath(new URL('../src/apply-plan-typecheck.ts', import.meta.url));
  const source = `import { RecordStore, type Authority } from './records.js';
    declare const store: RecordStore, authority: Authority, revision: string;
    store.applyPlan(authority, { goal: { kind: 'unevaluated' } }, revision);`;
  const configPath = fileURLToPath(new URL('../tsconfig.json', import.meta.url));
  const config = ts.readConfigFile(configPath, ts.sys.readFile);
  const { options } = ts.parseJsonConfigFileContent(config.config, ts.sys, fileURLToPath(new URL('..', import.meta.url)));
  const host = ts.createCompilerHost(options);
  const getSourceFile = host.getSourceFile.bind(host);
  host.getSourceFile = (path, ...args) => path === filename ? ts.createSourceFile(path, source, options.target, true) : getSourceFile(path, ...args);
  const program = ts.createProgram([filename], { ...options, noEmit: true }, host);
  const diagnostics = ts.getPreEmitDiagnostics(program);
  assert.equal(diagnostics.length, 0, diagnostics.map(d => ts.flattenDiagnosticMessageText(d.messageText, '\n')).join('\n'));
});

test('empty plan clears provenance without removing references or changing manual titles', async t => {
  const { store, workspace, cwd } = await fixture(t); let a = await store.join(joinInput(workspace, cwd)); const auth = authority(a);
  const source = { checkout: a.record.checkout, path: 'plan.md' };
  let w = await store.addReferences(auth, {
    documents: [{ ...source, description: 'plan' }],
    issues: [{ url: 'https://example.com/issue', description: 'issue' }],
    prs: [{ url: 'https://example.com/pr', description: 'PR' }],
  });
  w = await store.setWorkspaceTitle(auth, 'Manual shared', w.revision);
  w = await store.applyPlan(auth, { goal: evaluatedGoal('Plan goal', [source], 'plan-bytes') }, w.revision);
  w = await store.describeReference(auth, source, 'not a plan', w.revision);
  const planned = w;
  for (const input of [
    { goal: { kind: 'known', text: 'Still a goal', sources: [], fingerprint: 'bytes' } },
    { goal: { kind: 'unknown', sources: [source], fingerprint: null } },
    { goal: { kind: 'unknown', sources: [], fingerprint: 'orphaned' } },
  ]) await assert.rejects(store.applyPlan(auth, input, w.revision));
  const empty = { goal: unevaluatedGoal(), title: 'Ignored automatic title' };
  await assert.rejects(store.applyPlan(auth, empty, workspace.revision), /revision|stale/i);
  await assert.rejects(store.applyPlan({ ...auth, membershipToken: 'stale' }, empty, w.revision), /membership|stale/i);
  a = await store.resetEvidence(auth, boundary, a.revision);
  await assert.rejects(store.applyPlan(auth, empty, w.revision), /generation|stale/i);
  assert.deepEqual(await store.readWorkspace(workspace.record.id), planned);
  w = await store.applyPlan(authority(a), empty, w.revision);
  assert.deepEqual(w.record, { ...planned.record, mutationToken: w.record.mutationToken, goal: unevaluatedGoal() });
  assert.notEqual(w.revision, planned.revision);
  assert.equal(composeView(w, a).goal, '');
  assert.deepEqual(await store.readWorkspace(workspace.record.id), w);
});

test('malformed YAML, record identity mismatches, parent symlinks and held locks fail closed', async t => {
  const { store, workspace, cwd } = await fixture(t); const a = await store.join(joinInput(workspace, cwd)); const bytes = await readFile(a.path);
  for (const malformed of [Buffer.from([0xff]), Buffer.from('version: 1\nversion: 1\n'), Buffer.from('[')]) { await writeFile(a.path, malformed); await assert.rejects(store.readAgent(a.record.sessionId)); }
  await writeFile(a.path, bytes); await writeFile(a.path + '.lock', 'owner');
  await assert.rejects(store.updateOwn(authority(a), {}, a.revision), /lock exists/); assert.equal(await readFile(a.path + '.lock', 'utf8'), 'owner');
  const controller = new AbortController(); controller.abort(); await assert.rejects(store.updateOwn(authority(a), {}, a.revision, controller.signal), { name: 'AbortError' });
  assert.equal(await readFile(a.path + '.lock', 'utf8'), 'owner');
  const aliasStore = new RecordStore(join(store.directory, 'alias')); await mkdir(aliasStore.directory); await symlink(join(store.directory, 'agents'), join(aliasStore.directory, 'agents'));
  await assert.rejects(aliasStore.readAgent(a.record.sessionId), /directory|symlink/);
});

test('exact remove/re-add ABA cannot revive an old removal selection', async t => {
  const { store, workspace, cwd } = await fixture(t); const a = await store.join(joinInput(workspace, cwd));
  const refs = { issues: [{ url: 'https://example.com/aba', description: '' }] };
  let w = await store.addReferences(authority(a), refs); const old = selectReference(w, '/aba').selection;
  w = await store.removeReference(authority(a), old);
  w = await store.addReferences(authority(a), refs, { explicit: true });
  assert.notEqual(w.revision, old.revision);
  await assert.rejects(store.removeReference(authority(a), old), /revision|stale/i);
});

test('foreground reference identities reject extra fields and malformed paths/URLs', async t => {
  const { store, workspace, cwd } = await fixture(t); const a = await store.join(joinInput(workspace, cwd));
  const w = await store.addReferences(authority(a), { issues: [{ url: 'https://example.com/id', description: '' }] });
  for (const identity of [{ url: 'https://example.com/id', extra: true }, { checkout: a.record.checkout, path: '../escape' }, { url: 'javascript:evil' }]) {
    await assert.rejects(store.describeReference(authority(a), identity, '', w.revision));
  }
});

test('separate cooperating processes merge additions without losing another writer', async t => {
  const { store, workspace, cwd } = await fixture(t); const a = await store.join(joinInput(workspace, cwd));
  const moduleUrl = new URL('../dist/records.js', import.meta.url).href;
  await Promise.all(Array.from({ length: 4 }, (_, i) => new Promise((resolve, reject) => {
    const code = `import { RecordStore } from ${JSON.stringify(moduleUrl)};
      const store = new RecordStore(${JSON.stringify(store.directory)});
      for (let n = 0; ; n++) {
        try { await store.addReferences(${JSON.stringify(authority(a))}, {issues: [{url: 'https://example.com/process/${i}', description: ''}]}); break; }
        catch (e) { if (!/lock exists/.test(e.message) || n > 100) throw e; await new Promise(r => setTimeout(r, 2)); }
      }`;
    const child = spawn(process.execPath, ['--input-type=module', '-e', code], { stdio: ['ignore', 'ignore', 'pipe'] });
    let stderr = ''; child.stderr.on('data', bytes => { stderr += bytes; }); child.on('error', reject); child.on('exit', status => status === 0 ? resolve() : reject(new Error(stderr)));
  })));
  assert.equal((await store.readWorkspace(workspace.record.id)).record.issues.length, 4);
});

test('new record publication never clobbers a file appearing after the absence check', async t => {
  const { store, workspace } = await fixture(t); const id = randomUUID(); const path = store.workspacePath(id);
  const competingBytes = JSON.stringify({ ...serializeWorkspaceV1(workspace.record), id, creationToken: 'different-reservation' });
  const handle = await open(workspace.path, 'r'); const proto = Object.getPrototypeOf(handle); const sync = proto.sync; await handle.close();
  t.mock.method(proto, 'sync', async function () { await sync.call(this); await writeFile(path, competingBytes); });
  await assert.rejects(store.createWorkspace({ id, creationToken: 'mine', title: 'Do not overwrite' }), /EEXIST|conflict/);
  assert.equal(await readFile(path, 'utf8'), competingBytes);
});

test('document resolution keeps its captured root when the checkout becomes an outside symlink', async t => {
  const { store, workspace, cwd, dir } = await fixture(t);
  const a = await store.join(joinInput(workspace, cwd));
  const w = (await store.readWorkspace(workspace.record.id)).record;
  const outside = join(dir, 'outside'); await mkdir(outside); await writeFile(join(outside, 'plan.md'), 'outside');
  const originalRealpath = fsPromises.realpath; let switched = false;
  const mock = t.mock.method(fsPromises, 'realpath', async function (path, ...args) {
    const result = await originalRealpath(path, ...args);
    if (path === cwd && !switched) {
      switched = true;
      await rename(cwd, join(dir, 'original-root'));
      await symlink(outside, cwd);
    }
    return result;
  });
  syncBuiltinESMExports();
  try {
    await assert.rejects(resolveWorkspaceDocument(w, { checkout: a.record.checkout, path: 'plan.md' }), /outside/);
    assert.equal(switched, true);
  } finally { mock.mock.restore(); syncBuiltinESMExports(); }
});

test('join uses one canonical cwd even when its input alias switches during discovery', async t => {
  const { store, workspace, cwd, dir } = await fixture(t);
  const outside = join(dir, 'outside'); await mkdir(join(outside, '.git'), { recursive: true });
  const alias = join(dir, 'alias'); await symlink(cwd, alias);
  const originalRealpath = fsPromises.realpath; let switched = false;
  const mock = t.mock.method(fsPromises, 'realpath', async function (path, ...args) {
    const result = await originalRealpath(path, ...args);
    if (path === alias && !switched) {
      switched = true;
      await rm(alias); await symlink(outside, alias);
    }
    return result;
  });
  syncBuiltinESMExports();
  try {
    const a = await store.join(joinInput(workspace, alias));
    const persisted = await store.readAgent(a.record.sessionId);
    const w = await store.readWorkspace(workspace.record.id);
    assert.equal(switched, true);
    assert.equal(persisted.record.cwd, cwd);
    assert.equal(w.record.checkouts[persisted.record.checkout].directory, cwd);
    assert.doesNotThrow(() => composeView(w, persisted));
  } finally { mock.mock.restore(); syncBuiltinESMExports(); }
});


test('derived sidebar fields preserve absence and enforce display width and Unicode', async t => {
  const { store, workspace, cwd } = await fixture(t); const a = await store.join(joinInput(workspace, cwd));
  assert.equal(Object.hasOwn(a.record, 'statusShort'), false);
  assert.equal(Object.hasOwn((await store.readWorkspace(workspace.record.id)).record.goal, 'short'), false);
  const short = applyOwnUpdate(a.record, { statusShort: '界'.repeat(11) + 'x' });
  assert.equal(short.statusShort, '界'.repeat(11) + 'x');
  assert.equal(applyOwnUpdate(short, { now: 'Different' }).statusShort, undefined);
  assert.equal(applyOwnUpdate(short, { now: short.now }).statusShort, short.statusShort);
  assert.equal(applyOwnUpdate(short, { status: 'done', statusShort: 'Ready' }).statusShort, 'Ready');
  for (const value of ['界'.repeat(12), 'x'.repeat(24), 'bad\ntext', '\ud800', 'bad\u2028line', '\u001b[31mx']) assert.throws(() => applyOwnUpdate(a.record, { statusShort: value }));
  for (const value of ['e\u0301'.repeat(23), '😀'.repeat(11), '']) assert.equal(applyOwnUpdate(a.record, { statusShort: value }).statusShort, value);
  assert.throws(() => validateWorkspace({ ...workspace.record, goal: { kind: 'unevaluated', short: 'No provenance' } }));
});

test('plan changes invalidate derived purpose; old records and no-ops keep bytes', async t => {
  const { store, workspace, cwd } = await fixture(t); const a = await store.join(joinInput(workspace, cwd)); const auth = authority(a);
  const source = { checkout: a.record.checkout, path: 'README.md', description: 'Purpose' };
  let w = await store.addReferences(auth, { documents: [source] });
  const sources = [{ checkout: source.checkout, path: source.path }];
  const input = { goal: evaluatedGoal('Deliver usable cards', sources, 'bytes', 'Usable cards') };
  w = await store.applyPlan(auth, input, w.revision); assert.equal(w.record.goal.short, 'Usable cards');
  assert.throws(() => validateWorkspace({ ...w.record, goal: { ...w.record.goal, short: '界'.repeat(13) } }));
  w = await store.applyPlan(auth, { goal: evaluatedGoal(input.goal.text, sources, 'bytes', '界'.repeat(12) + 'x') }, w.revision);
  w = await store.applyPlan(auth, { goal: evaluatedGoal('Changed purpose', sources, 'bytes') }, w.revision); assert.equal(w.record.goal.short, undefined);
  w = await store.applyPlan(auth, input, w.revision);
  w = await store.removeReference(auth, { workspaceId: w.record.id, revision: w.revision, identity: referenceIdentity(source) });
  assert.equal(w.record.goal.short, undefined);
});

test('shared proposal labels use snapshot descriptions and kinds while additions merge', async t => {
  const { store, workspace, cwd } = await fixture(t); const a = await store.join(joinInput(workspace, cwd)); const auth = authority(a);
  const link = { url: 'https://example.com/issue', description: 'Full issue description', label: 'Issue purpose' };
  let w = await store.addReferences(auth, { issues: [link] }); const baseline = w;
  assert.throws(() => applySharedAdditions(w.record, { prs: [{ ...link, url: 'https://example.com/new', label: ' ' }] }));
  assert.throws(() => applySharedAdditions(w.record, { prs: [{ ...link, url: 'https://example.com/new', label: '界'.repeat(31) }] }));
  const proposal = { additions: { prs: [{ ...link, label: 'Smuggled', description: 'Overwrite' }, { url: 'https://example.com/pr', description: 'New PR', label: 'New capability' }] }, labels: [{ url: link.url, label: 'Short purpose' }] };
  w = await store.updateSharedReferences(auth, baseline, proposal);
  assert.deepEqual(w.record.issues, [{ ...link, label: 'Short purpose' }]); assert.equal(w.record.prs.length, 1);
  const bytes = await readFile(w.path); const mtime = (await stat(w.path, { bigint: true })).mtimeNs;
  w = await store.updateSharedReferences(auth, baseline, proposal);
  assert.deepEqual(await readFile(w.path), bytes); assert.equal((await stat(w.path, { bigint: true })).mtimeNs, mtime);
  w = await store.describeReference(auth, { url: link.url }, 'Edited full description', w.revision); assert.equal(w.record.issues[0].label, undefined);
  w = await store.updateSharedReferences(auth, baseline, { labels: [{ url: link.url, label: 'Stale' }] });
  assert.equal(w.record.issues[0].label, undefined);
  w = await store.removeReference(auth, { workspaceId: w.record.id, revision: w.revision, identity: referenceIdentity(link) });
  w = await store.updateSharedReferences(auth, baseline, proposal); assert.deepEqual(w.record.issues, []);
  w = await store.addReferences(auth, { prs: [link] }, { explicit: true });
  w = await store.updateSharedReferences(auth, baseline, { labels: [{ url: link.url, label: 'Wrong kind' }] }); assert.equal(w.record.prs.at(-1).label, 'Issue purpose');
  await assert.rejects(store.updateSharedReferences(auth, baseline, { labels: [{ url: 'https://example.com/unknown', label: 'Unknown' }] }));
});
