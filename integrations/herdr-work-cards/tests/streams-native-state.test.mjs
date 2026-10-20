import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, realpath, rm, open, readFile, writeFile, symlink } from 'node:fs/promises';
import { promisify } from 'node:util';
import { execFile } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { StreamsNativeState, isReservedStreamsAttempt } from '../dist/herdr/streams-native-state.js';
const key = 'a'.repeat(64), pending = { version: 1, key, phase: 'pending', nonce: 'nonce' };
const ready = { ...pending, phase: 'ready', paneId: 'w:p2', terminalId: 'term2', tabId: 'w:t1' };
async function fixture(t) {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'streams-evidence-'))); t.after(() => rm(root, { recursive: true, force: true }));
  return { root, state: new StreamsNativeState(root) };
}
test('ancestor aliases allow evidence while leaf symlinks remain forbidden', async t => {
  const { root } = await fixture(t), alias = `${root}-alias`; await symlink(root, alias);
  t.after(() => rm(alias, { force: true }));
  const state = new StreamsNativeState(join(alias, 'state')); await state.write(pending);
  assert.deepEqual(await new StreamsNativeState(join(root, 'state')).read(key), pending);
  await symlink(join(root, 'state'), join(root, 'leaf'));
  await assert.rejects(new StreamsNativeState(join(root, 'leaf')).write(pending), /leaf symlink/);
});
test('readiness atomically completes a pending launch; reload retains exact tuple', async t => {
  const { root, state } = await fixture(t); assert.equal(await state.read(key), null);
  await state.write(pending); assert.deepEqual(await state.read(key), pending);
  await state.write(ready); assert.deepEqual(await new StreamsNativeState(root).read(key), ready);
});
test('pending evidence is either unlaunched or fully reserved; complete phases retain identity', async t => {
  const { root, state } = await fixture(t), path = join(root, `${key}.json`);
  for (const value of [pending, { ...ready, phase: 'pending' }, ready, { ...ready, phase: 'closed' }]) {
    await writeFile(path, JSON.stringify(value));
    const parsed = await state.read(key);
    assert.deepEqual(parsed, value);
    assert.equal(isReservedStreamsAttempt(parsed), value.phase === 'pending' && value.paneId !== undefined);
  }
});
test('partial, empty and missing complete identities fail closed without replacing evidence', async t => {
  const { root, state } = await fixture(t), path = join(root, `${key}.json`), fields = ['paneId', 'terminalId', 'tabId'];
  const identities = Array.from({ length: 7 }, (_, mask) => Object.fromEntries(fields.filter((_, i) => mask & (1 << i)).map(k => [k, ready[k]])));
  identities.push(...fields.flatMap(k => ['', null, 123].map(value => ({ paneId: ready.paneId, terminalId: ready.terminalId, tabId: ready.tabId, [k]: value }))));
  for (const phase of ['pending', 'ready', 'closed']) for (const identity of identities) {
    if (phase === 'pending' && Object.keys(identity).length === 0) continue;
    const bytes = JSON.stringify({ ...pending, phase, ...identity });
    await writeFile(path, bytes);
    await assert.rejects(state.read(key), /Incomplete native launch evidence/);
    await assert.rejects(state.write(ready), /Incomplete native launch evidence/);
    assert.equal(await readFile(path, 'utf8'), bytes);
  }
});
test('parsed unknown fields survive round trips and remain part of rollback CAS', async t => {
  const { root, state } = await fixture(t), path = join(root, `${key}.json`);
  const original = { ...pending, extension: { nested: ['preserve', { value: 1 }] } };
  await writeFile(path, JSON.stringify(original));
  const parsed = await state.read(key); assert.deepEqual(parsed, original);
  await state.write({ ...parsed, ...ready });
  assert.deepEqual(await state.read(key), { ...original, ...ready });
  const changed = { ...original, extension: { nested: ['preserve', { value: 2 }] } };
  const bytes = JSON.stringify(changed); await writeFile(path, bytes);
  await assert.rejects(state.discardUnlaunched(parsed, null), /reservation changed/);
  assert.equal(await readFile(path, 'utf8'), bytes);
  await state.discardUnlaunched(await state.read(key), null);
  assert.equal(await state.read(key), null);
});
test('post-rename directory sync failure retains recorded launch across processes', async t => {
  const { root, state } = await fixture(t); await state.write(pending);
  const handle = await open(root, 'r'), proto = Object.getPrototypeOf(handle), sync = proto.sync; await handle.close();
  proto.sync = async function() { if ((await this.stat()).isDirectory()) throw Error('directory fsync unavailable'); return sync.call(this); };
  try { await assert.rejects(state.write(ready), /fsync unavailable/); } finally { proto.sync = sync; }
  assert.equal(JSON.parse(await (await import('node:fs/promises')).readFile(join(root, `${key}.json`), 'utf8')).phase, 'ready');
  assert.deepEqual(await state.read(key), ready);
  const module = new URL('../dist/herdr/streams-native-state.js', import.meta.url).href;
  const { stdout } = await promisify(execFile)(process.execPath, ['--input-type=module', '-e', `import {StreamsNativeState} from ${JSON.stringify(module)}; console.log((await new StreamsNativeState(${JSON.stringify(root)}).read(${JSON.stringify(key)})).phase);`]);
  assert.equal(stdout.trim(), 'ready');
});
test('failed reservation sync leaves pending evidence and never authorizes a launch', async t => {
  const { root, state } = await fixture(t);
  const handle = await open(root, 'r'), proto = Object.getPrototypeOf(handle), sync = proto.sync; await handle.close();
  proto.sync = async function() { if ((await this.stat()).isDirectory()) throw Error('directory fsync unavailable'); return sync.call(this); };
  try { await assert.rejects(state.write(pending), /fsync unavailable/); } finally { proto.sync = sync; }
  assert.deepEqual(await new StreamsNativeState(root).read(key), pending);
});
test('malformed, invalid-schema and symlink evidence fails closed', async t => {
  const { root, state } = await fixture(t), path = join(root, `${key}.json`);
  await writeFile(path, '{'); await assert.rejects(state.read(key), SyntaxError);
  for (const value of [null, [], false, 1, { ...ready, phase: 'invalid' }, ...[undefined, null, 123, '', 'invalid nonce'].map(nonce => ({ ...ready, nonce }))]) {
    await writeFile(path, JSON.stringify(value)); await assert.rejects(state.read(key), /Malformed native launch evidence/);
  }
  await rm(path); const other = join(root, 'other'); await writeFile(other, JSON.stringify(ready)); await symlink(other, path); await assert.rejects(state.read(key));
});
