import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { createJiti } from 'jiti';
import { testCli } from './helpers/stream-cli.mjs';

const jiti = createJiti(import.meta.url);
const { executeLab } = await jiti.import('../extensions/stream-checkpoint/lab.ts');

const model = { provider: 'test', id: 'm', contextWindow: 100000 };
const selection = { provider: 'test', id: 'm' };
const response = (changes) => ({
  role: 'assistant',
  api: 'openai-responses',
  provider: 'test',
  model: 'm',
  timestamp: 1,
  stopReason: 'toolUse',
  usage: {
    input: 10,
    output: 4,
    cacheRead: 0,
    cacheWrite: 0,
    totalTokens: 14,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
  },
  content: [{ type: 'toolCall', id: 'call', name: 'record_card', arguments: { changes } }],
});

test('executeLab capture -> prepare -> run -> replay -> list -> inspect end to end against format-5 CLI', async (t) => {
  const root = mkdtempSync(join(tmpdir(), 'streams-prompt-lab-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));

  const workDir = join(root, 'work');
  const agentDir = join(root, 'pi-agent');
  const labRoot = join(root, 'lab');
  mkdirSync(workDir, { recursive: true });

  const cli = testCli(workDir);
  const createdStream = await cli.createStream({
    title: 'Lab Effort',
    titleMode: 'automatic',
    goal: 'Test prompt lab',
    goalMode: 'automatic',
  });
  const sessId = 'lab-session-1';
  await cli.createSession(sessId, {
    stream: createdStream.key,
    title: 'Lab Lane',
    titleMode: 'automatic',
    directory: workDir,
  });

  const transcriptFile = join(agentDir, 'sessions', `${sessId}.jsonl`);
  mkdirSync(dirname(transcriptFile), { recursive: true });
  writeFileSync(
    transcriptFile,
    [
      { type: 'session', version: 3, id: sessId, cwd: workDir, timestamp: new Date(0).toISOString() },
      {
        type: 'message',
        id: 'm1',
        parentId: null,
        timestamp: new Date(0).toISOString(),
        message: { role: 'user', content: 'Verify lab workflow', timestamp: 0 },
      },
    ]
      .map((e) => JSON.stringify(e))
      .join('\n') + '\n'
  );

  const env = {
    root: labRoot,
    agentDir,
    client: cli,
    runtime: {
      model,
      modelRegistry: {
        find: (p, id) => (p === 'test' && id === 'm' ? model : undefined),
        complete: async (_m, payload) => {
          const req = JSON.parse(payload.messages.at(-1).content);
          return req.task === 'session'
            ? response({ next: { text: 'Lab verified', short: 'Verified' } })
            : response({ goal: { text: 'Lab goal verified', short: null } });
        },
      },
    },
  };
  const signal = new AbortController().signal;

  const captured = await executeLab(
    { operation: 'capture', target: { kind: 'stream', id: createdStream.key } },
    env,
    signal
  );
  assert.equal(captured.status, 'completed');
  assert.equal(captured.samples.length, 1);
  assert.equal(captured.samples[0].available, true);

  const prepared = await executeLab(
    { operation: 'prepare', capture_id: captured.id, sample: 0, task: 'session' },
    env,
    signal
  );
  assert.equal(prepared.task, 'session');

  const ran = await executeLab(
    { operation: 'run', request_id: prepared.id, model: selection },
    env,
    signal
  );
  assert.equal(ran.status, 'completed');

  const replayed = await executeLab(
    { operation: 'replay', capture_id: captured.id, model: selection },
    env,
    signal
  );
  assert.equal(replayed.completed, 1);
  assert.equal(replayed.failed, 0);

  const listed = await executeLab({ operation: 'list', kind: 'run' }, env, signal);
  assert.equal(listed.artifacts.length, 2);

  const inspected = await executeLab(
    { operation: 'inspect', kind: 'run', id: replayed.id, section: 'output' },
    env,
    signal
  );
  assert.match(inspected.text, /Lab verified/);
});
