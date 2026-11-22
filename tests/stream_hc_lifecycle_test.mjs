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

  calls.length = 0;
  await withHc.writeSessionLabel('host/pi/sess-1', undefined, 'host/stream-9');
  assert.deepEqual(calls, [
    {
      program: 'hc',
      args: ['label', 'term-42', 'streams.session=host/pi/sess-1', 'streams.stream=host/stream-9'],
    },
  ]);
});

test('enrollmentCandidates resolves pre-bound stream and ranks space/parent candidates first', async () => {
  const makeStreamJson = (id, title, updated_at_ms, status = 'active') =>
    JSON.stringify({
      host_id: 'h1',
      id,
      title: { text: title, manual: false },
      goal: { text: `${title} goal`, short: null, manual: false },
      status,
      revision: 1,
      updated_at_ms,
    });

  // 1. Pre-bound streams.stream on current terminal returns attachedStream directly.
  const preboundRunner = async (program, args) => {
    if (program === 'hc' && args[0] === 'get' && args[1] === 't-self') {
      return {
        code: 0,
        stdout: JSON.stringify({ address: 't-self', labels: { 'streams.stream': 'h1/st-pre' } }),
        stderr: '',
      };
    }
    if (args[0] === 'stream' && args[1] === 'show' && args[2] === 'h1/st-pre') {
      return { code: 0, stdout: makeStreamJson('st-pre', 'Prebound', 100), stderr: '' };
    }
    return { code: 1, stdout: '', stderr: 'unexpected' };
  };
  const preboundCli = new StreamCli('/tmp', preboundRunner, { HC_TERMINAL_ID: 't-self' });
  const prebound = await preboundCli.enrollmentCandidates();
  assert.equal(prebound.attachedStream?.id, 'st-pre');
  assert.deepEqual(prebound.candidates, []);

  // 2. Sibling in same herdr.space and parent terminal are annotated and ranked first.
  const discoveryRunner = async (program, args) => {
    if (program === 'hc' && args[0] === 'get' && args[1] === 't-new') {
      return {
        code: 0,
        stdout: JSON.stringify({
          address: 't-new',
          parent: 't-parent',
          labels: { 'herdr.space': 'sp-1' },
        }),
        stderr: '',
      };
    }
    if (program === 'hc' && args[0] === 'get' && args[1] === 't-parent') {
      return {
        code: 0,
        stdout: JSON.stringify({
          address: 't-parent',
          labels: { 'herdr.space': 'sp-1', 'streams.stream': 'h1/st-parent' },
        }),
        stderr: '',
      };
    }
    if (program === 'hc' && args[0] === 'ls') {
      return {
        code: 0,
        stdout: JSON.stringify([{ address: 't-new' }, { address: 't-sib' }]),
        stderr: '',
      };
    }
    if (program === 'hc' && args[0] === 'get' && args[1] === 't-sib') {
      return {
        code: 0,
        stdout: JSON.stringify({
          address: 't-sib',
          labels: { 'herdr.space': 'sp-1', 'streams.stream': 'h1/st-sib' },
        }),
        stderr: '',
      };
    }
    if (args[0] === 'stream' && args[1] === 'list') {
      return {
        code: 0,
        stdout: JSON.stringify({
          items: [
            JSON.parse(makeStreamJson('st-other', 'Other Recent', 900)),
            JSON.parse(makeStreamJson('st-parent', 'Parent Work', 200)),
          ],
          unreadable: [],
        }),
        stderr: '',
      };
    }
    if (args[0] === 'stream' && args[1] === 'show' && args[2] === 'h1/st-sib') {
      return { code: 0, stdout: makeStreamJson('st-sib', 'Sibling Done', 150, 'done'), stderr: '' };
    }
    return { code: 1, stdout: '', stderr: `unexpected ${program} ${args.join(' ')}` };
  };
  const discoveryCli = new StreamCli('/tmp', discoveryRunner, { HC_TERMINAL_ID: 't-new' });
  const discovered = await discoveryCli.enrollmentCandidates();
  assert.equal(discovered.attachedStream, null);
  assert.deepEqual(
    discovered.candidates.map((c) => ({
      key: c.key,
      in_current_space: Boolean(c.in_current_space),
      from_parent_terminal: Boolean(c.from_parent_terminal),
    })),
    [
      { key: 'h1/st-parent', in_current_space: true, from_parent_terminal: true },
      { key: 'h1/st-sib', in_current_space: true, from_parent_terminal: false },
      { key: 'h1/st-other', in_current_space: false, from_parent_terminal: false },
    ]
  );
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
