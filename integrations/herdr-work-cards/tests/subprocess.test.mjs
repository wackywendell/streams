import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, stat } from 'node:fs/promises';
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
async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'streams-process-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  return root;
}

test('bounded process captures output and reports nonzero exit without retry', async () => {
  assert.equal(await runProcess(process.execPath, ['-e', 'console.log("hello")'], { env, timeoutMs: 5000 }), 'hello\n');
  await assert.rejects(runProcess(process.execPath, ['-e', 'console.error("failure");process.exitCode=2'], { env, timeoutMs: 5000 }), /failure/);
  await assert.rejects(runProcess(process.execPath, ['-e', 'console.log("x".repeat(100))'], { env, timeoutMs: 5000, maxBuffer: 20 }), /output limit/);
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
  const pending = runProcess(process.execPath, ['-e', 'process.on("SIGTERM",()=>{});require("fs").writeFileSync(process.argv[1],String(process.pid));setInterval(()=>{},1000)', marker], { env, timeoutMs: 5000, signal: controller.signal });
  const rejected = assert.rejects(pending, /abort/i);
  const pid = Number(await ready(marker)); controller.abort(); await rejected;
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
