import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, realpath, rm, open, writeFile, symlink } from 'node:fs/promises';
import { promisify } from 'node:util';
import { execFile } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { StreamsNativeState } from '../dist/herdr/streams-native-state.js';
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
  for (const value of ['{', JSON.stringify({ ...ready, phase: 'invalid' })]) {
    await writeFile(path, value); await assert.rejects(state.read(key));
  }
  await rm(path); const other = join(root, 'other'); await writeFile(other, JSON.stringify(ready)); await symlink(other, path); await assert.rejects(state.read(key));
});
