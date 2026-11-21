import assert from 'node:assert/strict';
import test from 'node:test';
import { createJiti } from 'jiti';

const jiti = createJiti(import.meta.url);
const { NativeController } = await jiti.import('../extensions/stream-checkpoint/native.ts');
const { loadRustNative } = await jiti.import('../extensions/stream-checkpoint/native-rust.ts');
const { StreamCli } = await jiti.import('../extensions/stream-checkpoint/streams.ts');

const uuid = '11111111-2222-4333-8444-555555555555';

test('loadRustNative writes streams.session label and projects Pi session name under HC_TERMINAL_ID', async () => {
  const cliCalls = [];
  const setNames = [];
  const env = { HC_TERMINAL_ID: 'term-9' };
  const session = {
    host_id: 'host',
    harness: 'pi',
    id: uuid,
    stream_id: 'effort',
    title: { text: 'Catalog', manual: false },
    status: 'active',
    recap: null,
    state: null,
    remaining: null,
    directory: null,
    references: {},
    artifacts: {},
    revision: 1,
    updated_at_ms: 0,
  };
  const cli = new StreamCli(
    '/tmp',
    async (program, args) => {
      cliCalls.push({ program, args });
      if (program === 'streams' && args[0] === 'session' && args[1] === 'show') {
        return { code: 0, stdout: JSON.stringify(session), stderr: '' };
      }
      return { code: 0, stdout: '', stderr: '' };
    },
    env
  );
  let shown = '';
  const pi = {
    getSessionName: () => shown,
    setSessionName: (name) => {
      shown = name;
      setNames.push(name);
    },
  };
  const ctx = {
    cwd: '/tmp',
    hasUI: true,
    mode: 'tui',
    sessionManager: {
      getSessionId: () => uuid,
      getSessionName: () => shown,
    },
  };
  const adapter = await loadRustNative({
    pi,
    ctx,
    sessionId: uuid,
    cwd: '/tmp',
    env,
    cli,
    piNamingSession: () => true,
    assertCurrent: () => {},
    warning: (msg) => assert.fail(`Unexpected warning: ${msg}`),
  });

  await adapter.start(new AbortController().signal);
  assert.deepEqual(cliCalls, [
    { program: 'streams', args: ['session', 'show', `pi/${uuid}`, '--json'] },
    { program: 'hc', args: ['label', 'term-9', `streams.session=host/pi/${uuid}`] },
  ]);
  assert.deepEqual(setNames, ['Catalog']);
  await adapter.dispose();
});

test('NativeController fences UI lifetime and routes open/close to adapter', async (t) => {
  const prevHc = process.env.HC_TERMINAL_ID;
  process.env.HC_TERMINAL_ID = 'term-controller';
  t.after(() => {
    if (prevHc === undefined) delete process.env.HC_TERMINAL_ID;
    else process.env.HC_TERMINAL_ID = prevHc;
  });

  const events = [];
  const ctx = {
    cwd: '/tmp',
    hasUI: true,
    mode: 'tui',
    sessionManager: {
      getSessionId: () => uuid,
      buildSessionProjection: () => ({ messages: [{ role: 'user', content: 'hi' }] }),
    },
  };
  const controller = new NativeController({
    pi: {},
    ctx,
    cli: new StreamCli('/tmp', async () => ({ code: 0, stdout: '{}', stderr: '' })),
    warning: (msg) => events.push(['warn', msg]),
    factory: async () => ({
      start: async () => {
        events.push(['start']);
      },
      open: async () => {
        events.push(['open']);
        return { opened: true };
      },
      close: async () => {
        events.push(['close']);
        return { closed: true };
      },
      dispose: async () => {
        events.push(['dispose']);
      },
    }),
  });

  await controller.open();
  await controller.close();
  controller.stop();
  await controller.idle();

  assert.deepEqual(events, [['start'], ['open'], ['close'], ['dispose']]);
});
