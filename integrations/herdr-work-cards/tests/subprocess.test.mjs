import test from 'node:test';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { runProcess } from '../dist/herdr/subprocess.js';

const env = process.env;
async function ready(path) {
  const deadline = performance.now() + 5000;
  for (;;) {
    try { return await readFile(path, 'utf8'); }
    catch (error) { if (error.code !== 'ENOENT' || performance.now() > deadline) throw error; }
    await delay(5);
  }
}
function fixturePid(value) {
  const pid = Number(value);
  assert.ok(Number.isSafeInteger(pid) && pid > 0, `Expected a positive safe PID, got ${JSON.stringify(value)}`);
  return pid;
}
async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'streams-process-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  return root;
}

test('PID fixtures reject empty and invalid publication before signalling', async t => {
  const root = await fixture(t), marker = join(root, 'ready');
  await writeFile(marker, '');
  const value = await ready(marker);
  assert.equal(value, '');
  assert.throws(() => fixturePid(value), /positive safe PID/);
  for (const invalid of ['0', '-1', '1.5', 'NaN', 'Infinity', '9007199254740992']) {
    assert.throws(() => fixturePid(invalid), /positive safe PID/);
  }
  assert.equal(fixturePid(String(process.pid)), process.pid);
});

test('bounded process captures output and reports nonzero exit without retry', async () => {
  assert.equal(await runProcess(process.execPath, ['-e', 'console.log("hello")'], { env, timeoutMs: 5000 }), 'hello\n');
  await assert.rejects(runProcess(process.execPath, ['-e', 'console.error("failure");process.exitCode=2'], { env, timeoutMs: 5000 }), /failure/);
  await assert.rejects(runProcess(process.execPath, ['-e', 'console.log("x".repeat(100))'], { env, timeoutMs: 5000, maxBuffer: 20 }), /output limit/);
});

test('termination errors preserve the first abort and kill error and wait for close', async t => {
  const root = await fixture(t), marker = join(root, 'ready'), go = join(root, 'go');
  const controller = new AbortController(), reason = Object.assign(new Error('caller cancelled'), { code: 'ABORT_ERR' });
  const denied = Object.assign(new Error('permission denied'), { code: 'EPERM' });
  const later = Object.assign(new Error('later termination error'), { code: 'EIO' });
  const kill = process.kill.bind(process); let calls = 0, pid, settled = false;
  t.mock.method(process, 'kill', (target, signal) => {
    if (target < 0 && signal === 'SIGKILL') throw ++calls === 1 ? denied : later;
    return kill(target, signal);
  });
  t.after(() => { if (pid) { try { kill(-pid, 'SIGKILL'); } catch {} } });
  const script = 'const fs=require("fs"),marker=process.argv[1];fs.writeFileSync(marker+".tmp",String(process.pid));fs.renameSync(marker+".tmp",marker);let n=0;const timer=setInterval(()=>{if(fs.existsSync(process.argv[2])){console.log("x".repeat(100));if(++n===3)clearInterval(timer);}},20)';
  const pending = runProcess(process.execPath, ['-e', script, marker, go], { env, timeoutMs: 5000, maxBuffer: 20, signal: controller.signal });
  const checked = assert.rejects(pending, error => {
    settled = true;
    assert.ok(error instanceof AggregateError);
    assert.deepEqual(error.errors, [reason, denied]);
    assert.equal(error.errors[0], reason); assert.equal(error.errors[0].code, 'ABORT_ERR');
    assert.equal(error.errors[1], denied); assert.equal(error.errors[1].code, 'EPERM');
    assert.equal(error.cause, reason);
    assert.match(error.message, /caller cancelled.*termination failed.*permission denied.*effects may be partial/);
    return true;
  });
  pid = fixturePid(await ready(marker)); controller.abort(reason);
  await delay(10); assert.equal(settled, false);
  await writeFile(go, 'continue'); await checked;
  assert.ok(calls >= 2);
  assert.throws(() => kill(pid, 0), { code: 'ESRCH' }); pid = undefined;
});

test('finite child output remains diagnosed when the parent stalls and group termination fails', async t => {
  const root = await fixture(t), marker = join(root, 'exited');
  const denied = Object.assign(new Error('group permission denied'), { code: 'EPERM' });
  const kill = process.kill.bind(process);
  t.mock.method(process, 'kill', (target, signal) => {
    if (target < 0 && signal === 'SIGKILL') throw denied;
    return kill(target, signal);
  });
  const script = 'process.on("exit",()=>require("fs").writeFileSync(process.argv[1],"exiting"));console.log("x".repeat(100))';
  const pending = runProcess(process.execPath, ['-e', script, marker], { env, timeoutMs: 5000, maxBuffer: 20 });
  const checked = assert.rejects(pending, error => {
    assert.ok(error instanceof AggregateError);
    assert.equal(error.errors.length, 2); assert.equal(error.cause, error.errors[0]);
    assert.match(error.errors[0].message, /output limit/);
    assert.equal(error.errors[1], denied); assert.equal(error.errors[1].code, 'EPERM');
    assert.match(error.message, /output limit.*termination failed.*group permission denied.*effects may be partial/);
    return true;
  });
  const clock = new Int32Array(new SharedArrayBuffer(4)), deadline = performance.now() + 5000;
  while (!existsSync(marker) && performance.now() < deadline) Atomics.wait(clock, 0, 0, 10);
  assert.ok(existsSync(marker));
  Atomics.wait(clock, 0, 0, 25);
  await checked;
});

test('a failed deadline termination retains both the timeout and original kill code', async t => {
  const denied = Object.assign(new Error('termination not permitted'), { code: 'EACCES' });
  const kill = process.kill.bind(process);
  t.mock.method(process, 'kill', (target, signal) => {
    if (target < 0 && signal === 'SIGKILL') throw denied;
    return kill(target, signal);
  });
  await assert.rejects(runProcess(process.execPath, ['-e', 'setTimeout(()=>{},200)'], { env, timeoutMs: 50 }), error => {
    assert.ok(error instanceof AggregateError);
    assert.equal(error.errors.length, 2); assert.equal(error.cause, error.errors[0]);
    assert.match(error.errors[0].message, /timed out/);
    assert.equal(error.errors[1], denied); assert.equal(error.errors[1].code, 'EACCES');
    assert.match(error.message, /timed out.*termination failed.*termination not permitted.*effects may be partial/);
    return true;
  });
});

test('pre-aborted process never starts and missing executable settles', async t => {
  const root = await fixture(t), marker = join(root, 'started'), controller = new AbortController();
  controller.abort();
  assert.throws(() => runProcess(process.execPath, ['-e', 'require("fs").writeFileSync(process.argv[1],"started")', marker], { env, timeoutMs: 5000, signal: controller.signal }), /abort/i);
  await assert.rejects(stat(marker), { code: 'ENOENT' });
  await assert.rejects(runProcess(join(root, 'nonexistent'), [], { env, timeoutMs: 100 }), { code: 'ENOENT' });
});

test('abort waits for a SIGTERM-resistant child to close', async t => {
  const root = await fixture(t), marker = join(root, 'ready'), controller = new AbortController();
  const pending = runProcess(process.execPath, ['-e', 'process.on("SIGTERM",()=>{});const fs=require("fs"),marker=process.argv[1];fs.writeFileSync(marker+".tmp",String(process.pid));fs.renameSync(marker+".tmp",marker);setInterval(()=>{},1000)', marker], { env, timeoutMs: 5000, signal: controller.signal });
  const rejected = assert.rejects(pending, /abort/i);
  const pid = fixturePid(await ready(marker)); controller.abort(); await rejected;
  assert.throws(() => process.kill(pid, 0), { code: 'ESRCH' });
});

test('deadline kills a command even when SIGTERM is ignored', async () => {
  const start = performance.now();
  await assert.rejects(runProcess(process.execPath, ['-e', 'process.on("SIGTERM",()=>{});setInterval(()=>{},1000)'], { env, timeoutMs: 150 }), /timed out/);
  assert.ok(performance.now() - start < 2000);
});

test('abort terminates descendants before they can perform delayed effects', async t => {
  const root = await fixture(t), marker = join(root, 'descendant-ready'), effect = join(root, 'effect'), controller = new AbortController();
  const descendant = 'process.on("SIGTERM",()=>{});const fs=require("fs");fs.writeFileSync(process.argv[1],"ready");setTimeout(()=>fs.writeFileSync(process.argv[2],"effect"),600);setInterval(()=>{},1000)';
  const leader = 'require("child_process").spawn(process.execPath,["-e",process.argv[1],process.argv[2],process.argv[3]],{stdio:"inherit"});setInterval(()=>{},1000)';
  const pending = runProcess(process.execPath, ['-e', leader, descendant, marker, effect], { env, timeoutMs: 5000, signal: controller.signal });
  const rejected = assert.rejects(pending, /abort/i);
  await ready(marker); controller.abort(); await rejected;
  await delay(700);
  await assert.rejects(stat(effect), { code: 'ENOENT' });
});
