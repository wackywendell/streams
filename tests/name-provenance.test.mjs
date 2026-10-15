import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, open, readFile, readdir, realpath, rename, rm, stat, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { NameStore, decideName } from '../dist/herdr/name-provenance.js';

const owner = { terminalId: 'terminal-1', nonce: 'owner-1' };
const successor = { terminalId: 'terminal-2', nonce: 'owner-2' };
async function fixture(t) {
  const dir = await realpath(await mkdtemp(join(tmpdir(), 'name-provenance-')));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const root = join(dir, 'state'); await mkdir(root);
  const store = new NameStore(root);
  const scope = await store.scope({ socket: '/tmp/test.sock', workspaceId: 'workspace-1', bindingNonce: 'binding-1', kind: 'tab', surfaceId: 'tab-1' });
  return { dir, root, store, scope };
}
const automatic = (scope) => ({ version: 1, scope, mode: 'auto', owner, lastApplied: 'Automatic' });
const decide = (scope, provenance = null, extra = {}) => decideName({ scope, provenance, owner, currentLabel: 'Automatic', desiredName: 'Next', defaultLabelProven: false, ...extra });

test('scopes isolate stores, binding lifetimes, sockets, workspace, kind and surface; aliases share identity', async t => {
  const { dir, root, store, scope } = await fixture(t);
  await symlink(root, join(dir, 'alias'));
  const input = { ...scope }; delete input.storePath;
  const alias = new NameStore(join(dir, 'alias'));
  assert.deepEqual(await alias.scope(input), scope);
  assert.equal(store.path(scope), alias.path(scope));
  const paths = new Set([store.path(scope)]);
  for (const change of [{ socket: '/tmp/other.sock' }, { workspaceId: 'other' }, { bindingNonce: 'other' }, { kind: 'workspace' }, { surfaceId: 'other' }]) paths.add(store.path(await store.scope({ ...input, ...change })));
  const otherRoot = join(dir, 'other'); await mkdir(otherRoot); const other = new NameStore(otherRoot);
  paths.add(other.path(await other.scope(input)));
  assert.equal(paths.size, 7);
  // Pi and editor use the identical tab scope: no role or owner participates in identity.
  assert.equal(store.path(scope), alias.path(await alias.scope(input)));
  assert.match(store.path(scope), /\/\.native-names\/[a-f0-9]{64}\.json$/);
});

test('missing read creates nothing and only proven blank/numeric defaults permit adoption', async t => {
  const { root, store, scope } = await fixture(t);
  assert.equal(await store.read(scope), null); assert.deepEqual(await readdir(root), []);
  for (const label of ['', '   ', '1', '002']) {
    assert.equal(decide(scope, null, { currentLabel: label, defaultLabelProven: true }).kind, 'rename');
    assert.equal(decide(scope, null, { currentLabel: label }).kind, 'manual');
  }
  assert.deepEqual(decide(scope, null, { currentLabel: '1', desiredName: '1', defaultLabelProven: true }), { kind: 'noop' });
  const manual = decide(scope, null, { currentLabel: 'User title', defaultLabelProven: true });
  assert.equal(manual.kind, 'manual'); assert.equal(manual.provenance.owner, null); assert.equal(manual.provenance.lastApplied, null);
  assert.equal(decide(scope, null, { currentLabel: 'Next', desiredName: 'Next' }).kind, 'manual');
});

test('verified new first owner inherits unchanged automatic label without predecessor liveness', async t => {
  const { scope } = await fixture(t); const prior = automatic(scope);
  for (const nextOwner of [successor, { ...owner, nonce: 'restarted' }]) {
    const result = decide(scope, prior, { owner: nextOwner });
    assert.equal(result.kind, 'rename'); assert.equal(result.name, 'Next');
    assert.deepEqual(result.afterSuccess.owner, nextOwner); assert.equal(result.afterSuccess.lastApplied, 'Next');
    assert.equal(prior.lastApplied, 'Automatic'); assert.deepEqual(prior.owner, owner);
  }
  assert.deepEqual(decide(scope, prior, { desiredName: 'Automatic', owner: successor }), { kind: 'noop' });
});

test('observed mismatch pins manual mode permanently, even if old automatic label returns', async t => {
  const { scope } = await fixture(t);
  const pin = decide(scope, automatic(scope), { currentLabel: 'User override' });
  assert.equal(pin.kind, 'manual'); assert.equal(pin.provenance.mode, 'manual');
  assert.equal(pin.provenance.lastApplied, 'Automatic'); assert.deepEqual(pin.provenance.owner, owner);
  const again = decide(scope, pin.provenance, { currentLabel: 'Automatic', defaultLabelProven: true });
  assert.equal(again.kind, 'manual'); assert.deepEqual(again.provenance, pin.provenance);
});

test('rename intent does not persist; verified success can publish provenance with raw CAS/no-op preservation', async t => {
  const { store, scope } = await fixture(t);
  const intent = decide(scope, null, { currentLabel: '', defaultLabelProven: true });
  assert.equal(await store.read(scope), null);
  const saved = await store.write(scope, intent.afterSuccess, null);
  const bytes = Buffer.from(JSON.stringify(saved.record, null, 3) + '\n'); await writeFile(saved.path, bytes);
  const current = await store.read(scope); assert.equal(current.revision, createHash('sha256').update(bytes).digest('hex'));
  const mtime = (await stat(saved.path, { bigint: true })).mtimeNs;
  const unchanged = await store.write(scope, saved.record, current.revision);
  assert.equal(unchanged.revision, current.revision); assert.deepEqual(await readFile(saved.path), bytes);
  assert.equal((await stat(saved.path, { bigint: true })).mtimeNs, mtime);
  await assert.rejects(store.write(scope, { ...saved.record, lastApplied: 'Other' }, saved.revision), /revision|conflict/i);
  await assert.rejects(store.write(scope, saved.record, null), /revision|conflict|exist/i);
  const updated = await store.write(scope, { ...saved.record, lastApplied: 'Other' }, current.revision);
  assert.equal((await store.read(scope)).record.lastApplied, 'Other'); assert.notEqual(updated.revision, current.revision);
  assert.equal((await stat(updated.path)).mode & 0o777, 0o600);
  assert.deepEqual(await readdir(join(store.directory, '.native-names')), [updated.path.split('/').at(-1)]);
});

test('malformed, unknown-field, wrong-scope and invalid provenance are rejected without overwrite', async t => {
  const { store, scope } = await fixture(t); const record = automatic(scope); const saved = await store.write(scope, record, null);
  for (const bad of ['{', Buffer.from([255]), JSON.stringify(record).replace('"version":1', '"version":2,"version":1'), JSON.stringify({ ...record, extra: true }), JSON.stringify({ ...record, scope: { ...scope, surfaceId: 'elsewhere' } }), JSON.stringify({ ...record, owner: null }), JSON.stringify({ ...record, lastApplied: 'bad\nname' })]) {
    await writeFile(saved.path, bad); const bytes = await readFile(saved.path);
    await assert.rejects(store.read(scope)); await assert.rejects(store.write(scope, record, null));
    assert.deepEqual(await readFile(saved.path), bytes);
  }
  for (const change of [{ surfaceId: '../escape' }, { bindingNonce: 'a/b' }, { kind: 'pane' }, { socket: 'relative.sock' }, { socket: '/tmp/../escape' }, { workspaceId: 'x\u001b' }, { owner: 'unexpected' }]) {
    const input = { ...scope, ...change }; delete input.storePath;
    await assert.rejects(store.scope(input));
  }
  assert.throws(() => decide(scope, { ...record, scope: { ...scope, bindingNonce: 'stale' } }));
  assert.throws(() => decide(scope, record, { owner: { ...owner, extra: true } }));
});

test('state root must exist and remain canonical; cache directory/file symlinks fail closed', async t => {
  const { dir, root, store, scope } = await fixture(t);
  const input = { ...scope }; delete input.storePath;
  await assert.rejects(new NameStore(join(dir, 'missing')).scope(input), { code: 'ENOENT' });
  const fileRoot = join(dir, 'not-directory'); await writeFile(fileRoot, 'x');
  await assert.rejects(new NameStore(fileRoot).scope(input), /directory/);
  const saved = await store.write(scope, automatic(scope), null); const bytes = await readFile(saved.path);
  await rm(saved.path); const target = join(dir, 'target'); await writeFile(target, bytes); await symlink(target, saved.path);
  await assert.rejects(store.read(scope), /ELOOP|symlink|regular/); await assert.rejects(store.write(scope, automatic(scope), null));
  await rm(saved.path); await mkdir(saved.path); await assert.rejects(store.read(scope), /regular/);
  await rm(join(root, '.native-names'), { recursive: true }); await mkdir(join(dir, 'cache')); await symlink(join(dir, 'cache'), join(root, '.native-names'));
  await assert.rejects(store.read(scope), /directory|symlink/); await assert.rejects(store.write(scope, automatic(scope), null));
  await rename(root, join(dir, 'old')); await symlink(join(dir, 'old'), root);
  await assert.rejects(store.read(scope), /identity|canonical/);
});

test('FIFO reads are nonblocking, including a nonregular cache directory', async t => {
  const { root, store, scope } = await fixture(t); await mkdir(join(root, '.native-names'));
  assert.equal(spawnSync('mkfifo', [store.path(scope)]).status, 0);
  const code = `import {NameStore} from ${JSON.stringify(new URL('../dist/herdr/name-provenance.js', import.meta.url).href)}; import assert from 'node:assert/strict'; await assert.rejects(new NameStore(${JSON.stringify(root)}).read(${JSON.stringify(scope)}), /regular/);`;
  const child = spawnSync(process.execPath, ['--input-type=module', '-e', code], { timeout: 3000, encoding: 'utf8' }); assert.equal(child.status, 0, child.stderr || child.error?.message);
  await rm(join(root, '.native-names'), { recursive: true }); assert.equal(spawnSync('mkfifo', [join(root, '.native-names')]).status, 0);
  await assert.rejects(store.read(scope), /directory/);
});

test('cancellation before publication preserves old bytes and cleans private temporary files', async t => {
  const { store, scope } = await fixture(t); const saved = await store.write(scope, automatic(scope), null); const bytes = await readFile(saved.path);
  const file = await open(saved.path, 'r'); const proto = Object.getPrototypeOf(file); const sync = proto.sync; await file.close();
  const controller = new AbortController(); const mock = t.mock.method(proto, 'sync', async function () { await sync.call(this); controller.abort(); });
  await assert.rejects(store.write(scope, { ...saved.record, lastApplied: 'Cancelled' }, saved.revision, controller.signal), { name: 'AbortError' }); mock.mock.restore();
  assert.deepEqual(await readFile(saved.path), bytes); assert.equal((await readdir(join(store.directory, '.native-names'))).length, 1);
  const fresh = await store.scope({ socket: scope.socket, workspaceId: scope.workspaceId, bindingNonce: scope.bindingNonce, kind: 'tab', surfaceId: 'fresh' });
  const cancelled = new AbortController(); cancelled.abort(); await assert.rejects(store.write(fresh, automatic(fresh), null, cancelled.signal), { name: 'AbortError' }); assert.equal(await store.read(fresh), null);
});

test('concurrent missing-cache creates never clobber and readers only see complete records', async t => {
  const { store, scope } = await fixture(t);
  const results = await Promise.allSettled([store.write(scope, automatic(scope), null), store.write(scope, { ...automatic(scope), lastApplied: 'Competitor' }, null)]);
  assert.equal(results.filter(r => r.status === 'fulfilled').length, 1);
  const saved = await store.read(scope); assert(['Automatic', 'Competitor'].includes(saved.record.lastApplied));
  const reads = Array.from({ length: 15 }, () => store.read(scope));
  await store.write(scope, { ...saved.record, lastApplied: 'Updated' }, saved.revision);
  for (const r of await Promise.all(reads)) assert([saved.record.lastApplied, 'Updated'].includes(r.record.lastApplied));
});

test('a file appearing at publication is not clobbered; cancellation also blocks first publication', async t => {
  const { root, store, scope } = await fixture(t);
  const probe = await open(join(root, 'probe'), 'wx'); const proto = Object.getPrototypeOf(probe); const sync = proto.sync; await probe.close();
  const competitor = JSON.stringify({ ...automatic(scope), lastApplied: 'Competing writer' });
  let mock = t.mock.method(proto, 'sync', async function () {
    await sync.call(this);
    const temporary = (await readdir(join(root, '.native-names'))).find(n => n.endsWith('.tmp'));
    assert(temporary); assert.equal((await stat(join(root, '.native-names', temporary))).mode & 0o777, 0o600);
    await writeFile(store.path(scope), competitor, { flag: 'wx' });
  });
  await assert.rejects(store.write(scope, automatic(scope), null), { code: 'EEXIST' }); mock.mock.restore();
  assert.equal(await readFile(store.path(scope), 'utf8'), competitor);
  await rm(store.path(scope));
  const controller = new AbortController();
  mock = t.mock.method(proto, 'sync', async function () { await sync.call(this); controller.abort(); });
  await assert.rejects(store.write(scope, automatic(scope), null, controller.signal), { name: 'AbortError' }); mock.mock.restore();
  assert.equal(await store.read(scope), null); assert.deepEqual(await readdir(join(root, '.native-names')), []);
});

test('native-success/cache-failure retries conservatively with old or missing provenance', async t => {
  const { store, scope } = await fixture(t); const prior = await store.write(scope, automatic(scope), null);
  const intent = decide(scope, prior.record); assert.equal(intent.kind, 'rename');
  const controller = new AbortController(); controller.abort();
  // Native success is simulated by the next observation, not by this unwired module.
  await assert.rejects(store.write(scope, intent.afterSuccess, prior.revision, controller.signal));
  const observed = decide(scope, (await store.read(scope)).record, { currentLabel: intent.name, desiredName: 'Later' });
  assert.equal(observed.kind, 'manual');
  assert.equal(decide(scope, null, { currentLabel: intent.name, desiredName: 'Later' }).kind, 'manual');
});
