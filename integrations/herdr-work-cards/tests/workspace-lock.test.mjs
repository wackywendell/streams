import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readdir, readFile, stat, rm, writeFile, symlink, mkdir, unlink, open } from 'node:fs/promises';
import { createServer } from 'node:net';
import { spawn, execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { once } from 'node:events';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { withWorkspaceLock } from '../dist/herdr/workspace-lock.js';

const marker = 'work-cards workspace lock v1\n';
const moduleUrl = new URL('../dist/herdr/workspace-lock.js', import.meta.url).href;
const lockPath = socket => join(join(socket, '..'), `.work-cards-${createHash('sha256').update(`${socket}\0w`).digest('hex')}.lock`);

async function assertPersistent(root) {
  const names = (await readdir(root)).filter(name => name.endsWith('.lock'));
  assert.equal(names.length, 1);
  const identity = await stat(join(root, names[0]));
  assert.ok(identity.isFile());
  assert.equal(identity.uid, process.getuid());
}

async function directory(t) {
  const root = await mkdtemp(join(tmpdir(), 'wc-lock-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  return { root, socket: join(root, 'h.sock') };
}

function worker(t, socket, body, env = process.env) {
  const child = spawn(process.execPath, ['--input-type=module', '-e', `
    import { withWorkspaceLock } from ${JSON.stringify(moduleUrl)};
    const socket = ${JSON.stringify(socket)};
    const keepAlive = setInterval(() => {}, 1000);
    ${body}
  `], { env, stdio: ['ignore', 'pipe', 'pipe', 'ipc'] });
  let stderr = '';
  child.stderr.on('data', data => { stderr += data; });
  const closed = once(child, 'close');
  t.after(async () => { child.kill('SIGKILL'); await closed; });
  const message = () => Promise.race([
    once(child, 'message').then(([value]) => value),
    closed.then(([code]) => { throw new Error(`Worker exited ${code}: ${stderr}`); }),
  ]);
  return { child, closed, message };
}

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
  const names = await readdir(f.root), path = join(f.root, names[0]);
  const owner = await readFile(path, 'utf8'), identity = await stat(path), started = performance.now();
  await assert.rejects(withWorkspaceLock(f.socket, 'w', async () => { calls++; }, { waitMs: 30 }), /lock.*timed out/i);
  assert.ok(performance.now() - started >= 25);
  assert.ok(performance.now() - started < 500);
  assert.equal(calls, 0); assert.deepEqual(await readdir(f.root), names);
  assert.equal(await readFile(path, 'utf8'), owner);
  assert.equal((await stat(path)).ino, identity.ino);
});

test('cancellation while waiting and before acquisition never invokes callback', async t => {
  const f = await fixture(t), controller = new AbortController(); let calls = 0;
  const waiting = withWorkspaceLock(f.socket, 'w', async () => { calls++; }, { waitMs: 1000, signal: controller.signal });
  // Observe rejection immediately, including on the pre-fix implementation.
  const rejected = assert.rejects(waiting, /abort/i);
  await Promise.all([rejected, delay(30).then(() => controller.abort())]);
  await f.release();
  await assert.rejects(withWorkspaceLock(f.socket, 'w', async () => { calls++; }, { signal: controller.signal }), /abort/i);
  assert.equal(calls, 0); await assertPersistent(f.root);
});

test('waiter acquires after release, and callback EEXIST failure runs exactly once and releases', async t => {
  const f = await fixture(t); let calls = 0;
  const failure = Object.assign(new Error('callback lock exists'), { code: 'EEXIST' });
  const waiting = withWorkspaceLock(f.socket, 'w', async () => { calls++; throw failure; }, { waitMs: 1000 });
  const rejected = assert.rejects(waiting, error => error === failure);
  await Promise.all([rejected, delay(30).then(async () => { assert.equal(calls, 0); await f.release(); })]);
  assert.equal(calls, 1); await assertPersistent(f.root);
  await withWorkspaceLock(f.socket, 'w', async () => { calls++; });
  assert.equal(calls, 2); await assertPersistent(f.root);
});

test('cancellation immediately before the callback releases the acquired lock', async t => {
  const f = await fixture(t); await f.release();
  const controller = new AbortController(); let checks = 0, calls = 0;
  controller.signal.throwIfAborted = function() {
    if (++checks === 4) controller.abort();
    AbortSignal.prototype.throwIfAborted.call(this);
  };
  await assert.rejects(withWorkspaceLock(f.socket, 'w', async () => { calls++; }, { signal: controller.signal }), /abort/i);
  assert.equal(checks, 4); assert.equal(calls, 0);
  await assertPersistent(f.root);
});

test('non-contention filesystem errors fail without invoking callback', async t => {
  const root = await mkdtemp(join(tmpdir(), 'wc-lock-missing-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  let calls = 0;
  await assert.rejects(withWorkspaceLock(join(root, 'missing', 'h.sock'), 'w', async () => { calls++; }, { waitMs: 1000 }), { code: 'ENOENT' });
  assert.equal(calls, 0);
});

async function pythonWrapper(root, source) {
  const executable = execFileSync('python3', ['-I', '-S', '-c', 'import sys; print(sys.executable)'], { encoding: 'utf8' }).trim();
  await writeFile(join(root, 'python3'), `#!/bin/sh\n${source(executable)}\n`, { mode: 0o700 });
  return { ...process.env, PATH: root };
}

async function eventually(check) {
  for (let attempt = 0; attempt < 200; attempt++) {
    if (await check()) return;
    await delay(10);
  }
  assert.fail('condition did not become true');
}

function gone(pid) {
  try {
    // An exited orphan can briefly remain a zombie, but cannot retain fd3.
    return execFileSync('/bin/ps', ['-o', 'stat=', '-p', String(pid)], { encoding: 'utf8' }).trim().startsWith('Z');
  } catch (error) { if (error.status === 1) return true; throw error; }
}

test('uncontended short wait starts inside Python, with isolated environment', async t => {
  const f = await directory(t);
  const env = await pythonWrapper(f.root, python => `
    [ -z "\${PYTHONHOME+x}" ] && [ -z "\${PYTHONPATH+x}" ] || exit 8
    /bin/sleep 0.15
    exec ${JSON.stringify(python)} "$@"`);
  env.PYTHONHOME = '/does/not/exist'; env.PYTHONPATH = '/does/not/exist';
  const w = worker(t, f.socket, `
    await withWorkspaceLock(socket, 'w', async () => {}, { waitMs: 30 });
    process.send('acquired'); clearInterval(keepAlive); process.disconnect();`, env);
  assert.equal(await w.message(), 'acquired');
  assert.equal((await w.closed)[0], 0);
  await assertPersistent(f.root);
});

test('minimal PATH falls back to a standard Python installation', async t => {
  const f = await directory(t);
  const w = worker(t, f.socket, `
    await withWorkspaceLock(socket, 'w', async () => {});
    process.send('acquired'); clearInterval(keepAlive); process.disconnect();`, { ...process.env, PATH: f.root });
  assert.equal(await w.message(), 'acquired');
});

test('same-process contenders serialize without replacing the lock inode', async t => {
  const f = await directory(t); let active = 0, calls = 0;
  await withWorkspaceLock(f.socket, 'w', async () => {});
  const before = await stat(lockPath(f.socket));
  await Promise.all(Array.from({ length: 8 }, () => withWorkspaceLock(f.socket, 'w', async () => {
    assert.equal(++active, 1); calls++; await delay(10); active--;
  }, { waitMs: 3000 })));
  const after = await stat(lockPath(f.socket));
  assert.equal(calls, 8); assert.equal(before.ino, after.ino); assert.equal(before.dev, after.dev);
  await assertPersistent(f.root);
});

test('fresh multi-process contenders atomically create and serialize on one inode', async t => {
  const f = await directory(t); let entries = 0;
  const workers = Array.from({ length: 6 }, () => worker(t, f.socket, `
    const { open, unlink } = await import('node:fs/promises');
    await withWorkspaceLock(socket, 'w', async () => {
      const guard = await open(${JSON.stringify(join(f.root, 'critical'))}, 'wx');
      await new Promise(resolve => setTimeout(resolve, 30));
      await guard.close(); await unlink(${JSON.stringify(join(f.root, 'critical'))});
      process.send('entry');
    }, { waitMs: 3000 });
    clearInterval(keepAlive); process.disconnect();`));
  for (const { child } of workers) child.on('message', message => {
    assert.equal(message, 'entry'); entries++;
  });
  for (const w of workers) assert.equal((await w.closed)[0], 0);
  assert.equal(entries, 6);
  await assertPersistent(f.root);
  // The persistent inode exists even while the lock is idle.
  await assert.rejects(open(lockPath(f.socket), 'wx'), { code: 'EEXIST' });
});

test('independent process excludes contenders and SIGKILL releases without recovery edits', async t => {
  const f = await directory(t);
  const owner = worker(t, f.socket, `
    await withWorkspaceLock(socket, 'w', async () => {
      process.send('held'); await new Promise(() => {});
    });`);
  assert.equal(await owner.message(), 'held');
  const before = await stat(lockPath(f.socket)); let calls = 0;
  await assert.rejects(withWorkspaceLock(f.socket, 'w', async () => { calls++; }), /lock.*busy/i);
  owner.child.kill('SIGKILL'); await owner.closed;
  await withWorkspaceLock(f.socket, 'w', async () => { calls++; }, { waitMs: 30 });
  assert.equal(calls, 1); assert.equal((await stat(lockPath(f.socket))).ino, before.ino);
});

test('parent death while waiting makes helper exit on stdin EOF', async t => {
  const f = await fixture(t), pidFile = join(f.root, 'helper.pid');
  const env = await pythonWrapper(f.root, python => `echo $$ > ${JSON.stringify(pidFile)}\nexec ${JSON.stringify(python)} "$@"`);
  const waiter = worker(t, f.socket, `await withWorkspaceLock(socket, 'w', async () => process.send('unexpected'), { waitMs: 60000 });`, env);
  await eventually(async () => { try { return Number(await readFile(pidFile, 'utf8')) > 0; } catch { return false; } });
  const pid = Number(await readFile(pidFile, 'utf8'));
  await delay(100);
  waiter.child.kill('SIGKILL'); await waiter.closed;
  await eventually(() => gone(pid));
  await f.release();
  await withWorkspaceLock(f.socket, 'w', async () => {}, { waitMs: 30 });
});

test('abort kills and reaps a helper that already holds the inherited lock', async t => {
  const f = await directory(t), pidFile = join(f.root, 'helper.pid');
  const env = await pythonWrapper(f.root, python => `exec ${JSON.stringify(python)} -I -S -c 'import fcntl, os, signal; fcntl.flock(3, fcntl.LOCK_EX); open(${JSON.stringify(pidFile)}, "w").write(str(os.getpid())); os.write(2, b"ready\\n"); signal.pause()'`);
  const w = worker(t, f.socket, `
    const controller = new AbortController();
    process.on('message', () => controller.abort());
    try { await withWorkspaceLock(socket, 'w', async () => process.send('unexpected'), { signal: controller.signal }); }
    catch (error) { process.send(error.name); }
    clearInterval(keepAlive); process.disconnect();`, env);
  await eventually(async () => { try { return Number(await readFile(pidFile, 'utf8')) > 0; } catch { return false; } });
  const pid = Number(await readFile(pidFile, 'utf8'));
  const result = w.message(); w.child.send('abort');
  assert.equal(await result, 'AbortError');
  assert.ok(gone(pid));
  await withWorkspaceLock(f.socket, 'w', async () => {});
});

test('helper startup has its own finite deadline and cannot run callback', async t => {
  const f = await directory(t), pidFile = join(f.root, 'helper.pid');
  const env = await pythonWrapper(f.root, python => `exec ${JSON.stringify(python)} -I -S -c 'import os, signal; open(${JSON.stringify(pidFile)}, "w").write(str(os.getpid())); signal.pause()'`);
  const started = performance.now();
  const w = worker(t, f.socket, `
    try { await withWorkspaceLock(socket, 'w', async () => process.send('unexpected'), { waitMs: 30 }); }
    catch (error) { process.send(error.message); }
    clearInterval(keepAlive); process.disconnect();`, env);
  assert.match(await w.message(), /helper startup timed out/i);
  assert.ok(performance.now() - started < 10000);
  assert.ok(gone(Number(await readFile(pidFile, 'utf8'))));
  await withWorkspaceLock(f.socket, 'w', async () => {});
});

test('empty, marker-format and arbitrary contents have no authority and remain unchanged', async t => {
  const f = await directory(t), path = lockPath(f.socket); let calls = 0;
  for (const contents of ['', marker, 'pid=999999999\n', 'pid=1\n', 'work-cards workspace lock v2\n', marker + 'extra']) {
    await writeFile(path, contents); const identity = await stat(path);
    await withWorkspaceLock(f.socket, 'w', async () => {
      calls++;
      await assert.rejects(withWorkspaceLock(f.socket, 'w', async () => assert.fail('concurrent entry')), /lock.*busy/i);
    }, { waitMs: 30 });
    assert.equal(await readFile(path, 'utf8'), contents);
    const after = await stat(path);
    assert.equal(after.ino, identity.ino); assert.equal(after.dev, identity.dev);
    assert.equal(after.mode, identity.mode);
    await assertPersistent(f.root);
  }
  assert.equal(calls, 6);
});

test('lock ownership must match the caller UID', async t => {
  const f = await directory(t);
  await withWorkspaceLock(f.socket, 'w', async () => {});
  const w = worker(t, f.socket, `
    const uid = process.getuid();
    process.getuid = () => uid + 1;
    try { await withWorkspaceLock(socket, 'w', async () => process.send('unexpected')); }
    catch (error) { process.send(error.message); }
    clearInterval(keepAlive); process.disconnect();`);
  assert.match(await w.message(), /unsafe.*Inspect/i);
  await assertPersistent(f.root);
});

test('symlinks, directories, FIFOs and sockets fail closed without callback', async t => {
  const f = await directory(t), path = lockPath(f.socket), target = join(f.root, 'target'); let calls = 0;
  await writeFile(target, marker); await symlink(target, path);
  await assert.rejects(withWorkspaceLock(f.socket, 'w', async () => { calls++; }), /unsafe.*Inspect/i);
  assert.equal(await readFile(target, 'utf8'), marker);
  await unlink(path); await mkdir(path);
  await assert.rejects(withWorkspaceLock(f.socket, 'w', async () => { calls++; }), /unsafe.*Inspect/i);
  await rm(path, { recursive: true }); execFileSync('mkfifo', [path]);
  await assert.rejects(withWorkspaceLock(f.socket, 'w', async () => { calls++; }), /unsafe.*Inspect/i);
  await unlink(path);
  const server = createServer();
  try {
    server.listen(path); await once(server, 'listening');
    await assert.rejects(withWorkspaceLock(f.socket, 'w', async () => { calls++; }), /unsafe.*Inspect/i);
  } finally { await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve())); }
  assert.equal(calls, 0);
});

test('path replacement while waiting prevents callback even after flock succeeds', async t => {
  const f = await fixture(t); let calls = 0;
  const waiting = withWorkspaceLock(f.socket, 'w', async () => { calls++; }, { waitMs: 1000 });
  const rejected = assert.rejects(waiting, /unsafe.*Inspect/i);
  await delay(100);
  await unlink(lockPath(f.socket)); await writeFile(lockPath(f.socket), marker);
  await f.release(); await rejected;
  assert.equal(calls, 0);
  await withWorkspaceLock(f.socket, 'w', async () => {});
});
