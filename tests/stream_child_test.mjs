import assert from 'node:assert/strict';
import test from 'node:test';
import { createJiti } from 'jiti';

const jiti = createJiti(import.meta.url);
const { CardWatchChild } = await jiti.import('../extensions/stream-checkpoint/child.ts');

function fakeProc(onEnd) {
  const listeners = new Map();
  let stderrListener;
  const proc = {
    exitCode: null,
    killed: false,
    stdin: {
      on: () => {},
      end: () => {
        onEnd?.(proc, emit);
      },
    },
    stderr: {
      setEncoding: () => proc.stderr,
      on: (ev, fn) => {
        if (ev === 'data') stderrListener = fn;
      },
    },
    on: (ev, fn) => {
      const list = listeners.get(ev) ?? [];
      list.push(fn);
      listeners.set(ev, list);
    },
    kill: () => {
      proc.killed = true;
      emit('close', null, 'SIGTERM');
    },
    emitStderr: (chunk) => stderrListener?.(chunk),
    emit: (ev, ...args) => emit(ev, ...args),
  };
  function emit(ev, ...args) {
    listeners.get(ev)?.forEach((fn) => fn(...args));
  }
  return proc;
}

test('CardWatchChild spawns `streams card watch pi/<ID>` with plugin root and shuts down cleanly on stop()', async () => {
  const calls = [];
  const child = new CardWatchChild({
    sessionId: 'sess-1',
    cwd: '/tmp',
    pluginRoot: '/plugins/work-cards',
    env: { ...process.env, STREAMS_BIN_PATH: '/bin/sh' },
    spawn: (_cmd, args, opts) => {
      calls.push({ args, opts });
      return fakeProc((_p, emit) => setImmediate(() => emit('close', 0, null)));
    },
  });

  child.start();
  assert.equal(calls.length, 1);
  assert.deepEqual(calls[0].args, [
    'card',
    'watch',
    'pi/sess-1',
    '--plugin-root',
    '/plugins/work-cards',
  ]);
  assert.equal(child.running, true);

  await child.stop();
  assert.equal(child.running, false);
  await child.stop();
});

test('CardWatchChild ignores duplicate start() calls while running and does not start after stop()', async () => {
  let spawns = 0;
  const child = new CardWatchChild({
    sessionId: 'sess-2',
    cwd: '/tmp',
    env: { ...process.env, STREAMS_BIN_PATH: '/bin/sh' },
    spawn: () => {
      spawns++;
      return fakeProc((_p, emit) => setImmediate(() => emit('close', 0, null)));
    },
  });

  child.start();
  child.start();
  assert.equal(spawns, 1);
  await child.stop();
  child.start();
  assert.equal(spawns, 1);
});

test('CardWatchChild reports stderr lines via warning callback on unexpected exit', async () => {
  const warnings = [];
  let proc;
  const child = new CardWatchChild({
    sessionId: 'sess-3',
    cwd: '/tmp',
    env: { ...process.env, STREAMS_BIN_PATH: '/bin/sh' },
    warning: (msg) => warnings.push(msg),
    spawn: () => {
      proc = fakeProc();
      return proc;
    },
  });

  child.start();
  proc.emitStderr('watcher error\n');
  proc.emit('close', 1, null);
  assert.equal(warnings.length, 1);
  assert.match(warnings[0], /watcher error/);
  await child.stop();
});
