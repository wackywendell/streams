import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { createJiti } from 'jiti';
import { testCli } from './helpers/stream-cli.mjs';

const jiti = createJiti(import.meta.url);
const { buildCaptureInventory, captureSessions } = await jiti.import(
  '../extensions/stream-checkpoint/capture.ts'
);
const { replayCapture } = await jiti.import('../extensions/stream-checkpoint/replay.ts');

const model = { provider: 'fixture', id: 'deterministic', contextWindow: 100000 };
const response = (args) => ({
  role: 'assistant',
  content: [{ type: 'toolCall', id: 'call', name: 'record_card', arguments: args }],
  api: 'fixture',
  provider: model.provider,
  model: model.id,
  stopReason: 'toolUse',
  usage: {
    input: 10,
    output: 5,
    cacheRead: 0,
    cacheWrite: 0,
    totalTokens: 15,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
  },
  timestamp: 0,
});
const runtime = (generate) => ({
  model,
  modelRegistry: {
    find: (provider, id) => (provider === model.provider && id === model.id ? model : undefined),
    complete: async (_model, payload) =>
      generate(JSON.parse(payload.messages.at(-1).content)),
  },
});

function writeTranscript(agentDir, id, cwd) {
  const file = join(agentDir, 'sessions', `${id}.jsonl`);
  mkdirSync(dirname(file), { recursive: true });
  writeFileSync(
    file,
    [
      { type: 'session', version: 3, id, cwd, timestamp: new Date(0).toISOString() },
      {
        type: 'message',
        id: 'm1',
        parentId: null,
        timestamp: new Date(0).toISOString(),
        message: { role: 'user', content: 'Investigate the format-5 replay boundary', timestamp: 0 },
      },
    ]
      .map((entry) => JSON.stringify(entry))
      .join('\n') + '\n'
  );
  return file;
}

test('capture from real format-5 CLI and in-memory replay never modify live catalog records', async (t) => {
  const root = mkdtempSync(join(tmpdir(), 'streams-rust-replay-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));

  const workDir = join(root, 'work');
  const agentDir = join(root, 'pi-agent');
  mkdirSync(workDir, { recursive: true });
  const cli = testCli(workDir);

  const createdStream = await cli.createStream({
    title: 'Work',
    titleMode: 'automatic',
    goal: 'Old goal',
    goalMode: 'automatic',
  });
  const id = '00000000-0000-4000-8000-000000000001';
  await cli.createSession(id, {
    stream: createdStream.key,
    title: 'Lane',
    titleMode: 'automatic',
    directory: workDir,
  });
  const file = writeTranscript(agentDir, id, workDir);

  const inventory = await buildCaptureInventory(cli, createdStream.key, agentDir);
  assert.equal(inventory.samples.length, 1);
  assert.equal(inventory.samples[0].transcript_file, file);

  const { capture } = captureSessions(inventory, join(root, 'private'));
  const tasks = [];
  const { run } = await replayCapture(
    capture,
    join(root, 'private'),
    runtime((input) => {
      tasks.push(input.task);
      if (input.task === 'session') {
        return response({ changes: { state: 'Simulated session state', state_short: 'Simulated' } });
      }
      assert.equal(input.catalog.sessions[0].state.text, 'Simulated session state');
      return response({ changes: { goal: 'Simulated effort goal', goal_short: 'Sim goal' } });
    }),
    model,
    { kind: 'report', force: true },
    new AbortController().signal
  );

  assert.equal(run.samples[0].status, 'completed', run.samples[0].error);
  assert.deepEqual(tasks, ['session', 'stream']);
  assert.equal(run.samples[0].after.session.state.text, 'Simulated session state');
  assert.equal(run.samples[0].after.stream.goal.text, 'Simulated effort goal');

  // Verify live catalog was NOT modified by replay!
  const live = await cli.context(id);
  assert.equal(live.session.state, null);
  assert.equal(live.stream.goal.text, 'Old goal');
});

test('replay preserves dropped Session references and applies valid independent updates', async (t) => {
  const root = mkdtempSync(join(tmpdir(), 'streams-rust-replay-refs-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));

  const workDir = join(root, 'work');
  const agentDir = join(root, 'pi-agent');
  mkdirSync(workDir, { recursive: true });
  const cli = testCli(workDir);

  const createdStream = await cli.createStream({ title: 'Work', titleMode: 'manual' });
  const id = '00000000-0000-4000-8000-000000000004';
  const createdSession = await cli.createSession(id, {
    stream: createdStream.key,
    title: 'Lane',
    titleMode: 'automatic',
    directory: workDir,
  });
  await cli.editSession(createdSession.key, {
    changes: {
      references: {
        'https://example.org/retired': {
          kind: 'link',
          name: 'Retired',
          purpose: 'Old purpose',
          role: 'dependency',
          status: 'dropped',
          manual: true,
        },
        'https://example.org/brief': {
          kind: 'issue',
          name: 'Brief',
          purpose: 'Original purpose',
          role: 'brief',
          status: 'active',
          manual: false,
        },
      },
    },
  });
  writeTranscript(agentDir, id, workDir);

  const inventory = await buildCaptureInventory(cli, createdStream.key, agentDir);
  const { capture } = captureSessions(inventory, join(root, 'private'));

  const { run } = await replayCapture(
    capture,
    join(root, 'private'),
    runtime((input) =>
      input.task === 'session'
        ? response({
            changes: {},
            reference_proposals: [
              {
                key: 'https://example.org/retired',
                kind: 'link',
                name: 'Retired',
                purpose: 'Try restore',
                role: 'dependency',
                status: 'active',
              },
              {
                key: 'https://example.org/brief',
                kind: 'issue',
                name: 'Brief',
                purpose: 'Updated purpose',
                role: 'brief',
                status: 'active',
              },
            ],
          })
        : response({ changes: {} })
    ),
    model,
    { kind: 'report', force: true },
    new AbortController().signal
  );

  const sample = run.samples[0];
  assert.equal(sample.status, 'completed', sample.error);
  // Dropped/manual reference was NOT restored; automatic reference WAS updated.
  assert.equal(sample.after.session.references['https://example.org/retired'].status, 'dropped');
  assert.equal(sample.after.session.references['https://example.org/brief'].purpose, 'Updated purpose');
});

test('unattached replay runs enroll before session and stream in memory', async (t) => {
  const root = mkdtempSync(join(tmpdir(), 'streams-rust-replay-enroll-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));

  const id = '00000000-0000-4000-8000-000000000002';
  const file = writeTranscript(join(root, 'pi-agent'), id, root);
  const inventory = {
    version: 2,
    host_id: 'host',
    streams: [],
    target: { kind: 'stream', id: 'unattached', title: 'Unattached' },
    samples: [
      {
        host_id: 'host',
        harness: 'pi',
        native_session_id: id,
        open: null,
        transcript_file: file,
        unavailable: null,
        catalog: {
          stream: null,
          session: null,
          naming: { stream: 'automatic', session: 'automatic' },
        },
      },
    ],
  };
  const { capture } = captureSessions(inventory, join(root, 'private'));
  const tasks = [];
  const { run } = await replayCapture(
    capture,
    join(root, 'private'),
    runtime((input) => {
      tasks.push(input.task);
      if (input.task === 'enroll') {
        return response({ meaningful: true, stream: { title: 'Effort' }, session: { title: 'Lane' } });
      }
      if (input.task === 'session') return response({ changes: { state: 'New state' } });
      return response({ changes: { goal: 'New goal' } });
    }),
    model,
    { kind: 'report', force: true },
    new AbortController().signal
  );

  assert.equal(run.samples[0].status, 'completed', run.samples[0].error);
  assert.deepEqual(tasks, ['enroll', 'session', 'stream']);
  assert.equal(run.samples[0].after.session.state.text, 'New state');
  assert.equal(run.samples[0].after.stream.goal.text, 'New goal');
  assert.equal(readdirSync(root).includes('data'), false);
});
