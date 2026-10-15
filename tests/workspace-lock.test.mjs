import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { withWorkspaceLock } from '../dist/herdr/workspace-lock.js';

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'wc-lock-'));
  const socket = join(root, 'h.sock');
  let release, acquired;
  const ready = new Promise(resolve => { acquired = resolve; });
  const gate = new Promise(resolve => { release = resolve; });
  const holder = withWorkspaceLock(socket, 'w', async () => { acquired(); await gate; });
  await ready;
  t.after(async () => { release(); await holder; await rm(root, { recursive: true, force: true }); });
  return { root, socket, release: async () => { release(); await holder; } };
}

test('default lock contention fails immediately without entering or releasing another owner', async t => {
  const f = await fixture(t); let calls = 0;
  await assert.rejects(withWorkspaceLock(f.socket, 'w', async () => { calls++; }), /lock.*Inspect/i);
  assert.equal(calls, 0); assert.equal((await readdir(f.root)).length, 1);
});

test('bounded lock wait times out and leaves the holder untouched', async t => {
  const f = await fixture(t); let calls = 0;
  await assert.rejects(withWorkspaceLock(f.socket, 'w', async () => { calls++; }, { waitMs: 30 }), /lock.*timed out/i);
  assert.equal(calls, 0); assert.equal((await readdir(f.root)).length, 1);
});

test('cancellation while waiting and before acquisition never invokes callback', async t => {
  const f = await fixture(t), controller = new AbortController(); let calls = 0;
  const waiting = withWorkspaceLock(f.socket, 'w', async () => { calls++; }, { waitMs: 1000, signal: controller.signal });
  // Observe rejection immediately, including on the pre-fix implementation.
  const rejected = assert.rejects(waiting, /abort/i);
  await Promise.all([rejected, delay(30).then(() => controller.abort())]);
  await f.release();
  await assert.rejects(withWorkspaceLock(f.socket, 'w', async () => { calls++; }, { signal: controller.signal }), /abort/i);
  assert.equal(calls, 0); assert.deepEqual(await readdir(f.root), []);
});

test('waiter acquires after release, and callback EEXIST failure runs exactly once and releases', async t => {
  const f = await fixture(t); let calls = 0;
  const failure = Object.assign(new Error('callback lock exists'), { code: 'EEXIST' });
  const waiting = withWorkspaceLock(f.socket, 'w', async () => { calls++; throw failure; }, { waitMs: 1000 });
  const rejected = assert.rejects(waiting, error => error === failure);
  await Promise.all([rejected, delay(30).then(async () => { assert.equal(calls, 0); await f.release(); })]);
  assert.equal(calls, 1); assert.deepEqual(await readdir(f.root), []);
  await withWorkspaceLock(f.socket, 'w', async () => { calls++; });
  assert.equal(calls, 2); assert.deepEqual(await readdir(f.root), []);
});

test('cancellation immediately before the callback releases the acquired lock', async t => {
  const f = await fixture(t); await f.release();
  const controller = new AbortController(); let checks = 0, calls = 0;
  controller.signal.throwIfAborted = function() {
    if (++checks === 2) controller.abort();
    AbortSignal.prototype.throwIfAborted.call(this);
  };
  await assert.rejects(withWorkspaceLock(f.socket, 'w', async () => { calls++; }, { signal: controller.signal }), /abort/i);
  assert.equal(checks, 2); assert.equal(calls, 0);
  assert.deepEqual(await readdir(f.root), []);
});

test('non-contention filesystem errors fail without invoking callback', async t => {
  const root = await mkdtemp(join(tmpdir(), 'wc-lock-missing-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  let calls = 0;
  await assert.rejects(withWorkspaceLock(join(root, 'missing', 'h.sock'), 'w', async () => { calls++; }, { waitMs: 1000 }), { code: 'ENOENT' });
  assert.equal(calls, 0);
});
