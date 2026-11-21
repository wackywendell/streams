import assert from 'node:assert/strict';
import test from 'node:test';
import { createJiti } from 'jiti';

const jiti = createJiti(import.meta.url);
const { CardWatchChild } = await jiti.import('../extensions/stream-checkpoint/child.ts');
const { SessionScope } = await jiti.import('../extensions/stream-checkpoint/session-scope.ts');
const { StreamCli } = await jiti.import('../extensions/stream-checkpoint/streams.ts');

test('SessionScope invalidates fences on session switch and stop', () => {
  const scope = new SessionScope();
  let id = 'sess-a';
  const ctx = { sessionManager: { getSessionId: () => id } };
  scope.use(ctx);
  const fenceA = scope.fence();
  assert.equal(fenceA.current(), true);
  assert.equal(fenceA.live(), true);
  assert.equal(fenceA.signal.aborted, false);

  id = 'sess-b';
  assert.equal(fenceA.current(), false);
  scope.invalidate();
  assert.equal(fenceA.signal.aborted, true);

  scope.use(ctx);
  const fenceB = scope.fence();
  assert.equal(fenceB.current(), true);
  scope.stop();
  assert.equal(scope.stopped, true);
  assert.equal(fenceB.signal.aborted, true);
  assert.equal(fenceB.live(), false);
});

test('writeSessionLabel writes streams.session via hc only when HC_TERMINAL_ID is set', async () => {
  const calls = [];
  const runner = async (program, args) => {
    calls.push({ program, args });
    return { code: 0, stdout: '', stderr: '' };
  };

  const withoutHc = new StreamCli('/tmp', runner, { PATH: '/usr/bin' });
  await withoutHc.writeSessionLabel('host/pi/sess-1');
  assert.deepEqual(calls, []);

  const withHc = new StreamCli('/tmp', runner, { HC_TERMINAL_ID: 'term-42' });
  await withHc.writeSessionLabel('host/pi/sess-1');
  assert.deepEqual(calls, [
    { program: 'hc', args: ['label', 'term-42', 'streams.session=host/pi/sess-1'] },
  ]);
});

test('CardWatchChild stop() kills a wedged child after timeout and never hangs', async () => {
  let killed = false;
  const listeners = new Map();
  const child = new CardWatchChild({
    sessionId: 'sess-wedged',
    cwd: '/tmp',
    env: { ...process.env, STREAMS_BIN_PATH: '/bin/sh' },
    stopTimeoutMs: 25,
    spawn: () => ({
      exitCode: null,
      killed: false,
      stdin: { on: () => {}, end: () => {} },
      stderr: { setEncoding: () => ({ on: () => {} }), on: () => {} },
      on: (ev, fn) => {
        const list = listeners.get(ev) ?? [];
        list.push(fn);
        listeners.set(ev, list);
      },
      kill: () => {
        killed = true;
        listeners.get('close')?.forEach((fn) => fn(null, 'SIGTERM'));
      },
    }),
  });

  child.start();
  await child.stop();
  assert.equal(killed, true);
  assert.equal(child.running, false);
});
