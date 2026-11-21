import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createJiti } from 'jiti';
import { SessionManager } from '@earendil-works/pi-coding-agent';

const jiti = createJiti(import.meta.url);
const { captureSessions } = await jiti.import('../extensions/stream-checkpoint/capture.ts');
const { replayCapture, comparison } = await jiti.import('../extensions/stream-checkpoint/replay.ts');

const model = { provider: 'test', id: 'm', contextWindow: 100000 };
const response = (args) => ({
  role: 'assistant',
  api: 'openai-responses',
  provider: 'test',
  model: 'm',
  timestamp: 1,
  stopReason: 'toolUse',
  usage: {
    input: 5,
    output: 5,
    cacheRead: 0,
    cacheWrite: 0,
    totalTokens: 10,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
  },
  content: [{ type: 'toolCall', id: 'r', name: 'record_card', arguments: args }],
});
const runtime = (complete) => ({
  model,
  modelRegistry: {
    find: (p, id) => (p === model.provider && id === model.id ? model : undefined),
    complete,
  },
});

function makeCapture(t, opts = {}) {
  const dir = mkdtempSync(join(tmpdir(), 'streams-prompt-replay-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const manager = SessionManager.inMemory(dir, { id: 'sess-1' });
  manager.appendMessage({ role: 'user', content: 'Refactor the CLI by entity', timestamp: 1 });
  const stream = {
    host_id: 'host',
    id: 'effort',
    title: { text: 'Streams', manual: opts.streamTitleManual ?? false },
    goal: { text: 'Old goal', short: 'Old', manual: opts.streamGoalManual ?? false },
    status: 'active',
    revision: 1,
    updated_at_ms: 0,
  };
  const session = {
    host_id: 'host',
    harness: 'pi',
    id: 'sess-1',
    stream_id: 'effort',
    title: { text: 'CLI', manual: opts.sessionTitleManual ?? false },
    status: 'active',
    recap: { text: 'Old recap', short: null },
    state: { text: 'Old state', short: null },
    remaining: { text: 'Old remaining', short: null },
    directory: { path: dir, repository: null },
    references: {},
    artifacts: {},
    revision: 1,
    updated_at_ms: 0,
  };
  const inventory = {
    version: 2,
    host_id: 'host',
    streams: [{ id: 'effort', key: 'host/effort', title: 'Streams' }],
    target: { kind: 'stream', id: 'effort', title: 'Streams' },
    samples: [
      {
        host_id: 'host',
        harness: 'pi',
        native_session_id: 'sess-1',
        open: true,
        transcript_file: null,
        unavailable: null,
        catalog: {
          stream,
          session,
          naming: opts.naming ?? { stream: 'automatic', session: 'automatic' },
        },
      },
    ],
  };
  const root = join(dir, 'debug');
  const { capture } = captureSessions(inventory, root, manager);
  return { dir, root, capture };
}

test('replay reset --auto clears manual marks before regenerating', async (t) => {
  const { root, capture } = makeCapture(t, {
    streamTitleManual: true,
    streamGoalManual: true,
    sessionTitleManual: true,
  });
  const tasks = [];
  const { run, directory } = await replayCapture(
    capture,
    root,
    runtime(async (_m, payload) => {
      const req = JSON.parse(payload.messages.at(-1).content);
      tasks.push(req.task);
      if (req.task === 'session') {
        return response({ changes: { title: 'Fresh CLI', state: 'Regenerated state' } });
      }
      return response({ changes: { title: 'Fresh Effort', goal: 'Fresh goal' } });
    }),
    model,
    { kind: 'reset', target: 'all', auto: true },
    new AbortController().signal
  );

  assert.equal(run.samples[0].status, 'completed', run.samples[0].error);
  assert.deepEqual(tasks, ['session', 'stream']);
  assert.equal(run.samples[0].after.session.title.text, 'Fresh CLI');
  assert.equal(run.samples[0].after.session.title.manual, false);
  assert.equal(run.samples[0].after.stream.title.text, 'Fresh Effort');
  assert.equal(run.samples[0].after.stream.goal.text, 'Fresh goal');
  assert.match(readFileSync(join(directory, 'comparison.md'), 'utf8'), /reset all --auto/);
  assert.match(comparison(run), /simulated/i);
});

test('replay skips stream task when stream title and goal are both manual (without --auto)', async (t) => {
  const { root, capture } = makeCapture(t, {
    streamTitleManual: true,
    streamGoalManual: true,
  });
  const tasks = [];
  const { run } = await replayCapture(
    capture,
    root,
    runtime(async (_m, payload) => {
      const req = JSON.parse(payload.messages.at(-1).content);
      tasks.push(req.task);
      return response({ changes: { state: 'Session updated' } });
    }),
    model,
    { kind: 'report', force: true },
    new AbortController().signal
  );

  assert.equal(run.samples[0].status, 'completed');
  assert.deepEqual(tasks, ['session']);
  assert.equal(run.samples[0].after.stream.title.text, 'Streams');
  assert.equal(run.samples[0].after.stream.goal.text, 'Old goal');
});

test('replay respects --reset session targeting without touching stream', async (t) => {
  const { root, capture } = makeCapture(t);
  const tasks = [];
  const { run } = await replayCapture(
    capture,
    root,
    runtime(async (_m, payload) => {
      const req = JSON.parse(payload.messages.at(-1).content);
      tasks.push(req.task);
      return response({ changes: { state: 'Only session' } });
    }),
    model,
    { kind: 'reset', target: 'session', auto: false },
    new AbortController().signal
  );

  assert.equal(run.samples[0].status, 'completed');
  assert.deepEqual(tasks, ['session']);
  assert.equal(run.samples[0].after.session.state.text, 'Only session');
  assert.equal(run.samples[0].after.stream.goal.text, 'Old goal');
});
