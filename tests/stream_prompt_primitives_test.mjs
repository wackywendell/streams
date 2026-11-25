import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtempSync, readFileSync, writeFileSync, rmSync, statSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createJiti } from 'jiti';
import { SessionManager } from '@earendil-works/pi-coding-agent';

const jiti = createJiti(import.meta.url);
const load = (name) =>
  jiti.import(new URL(`../extensions/stream-checkpoint/${name}.ts`, import.meta.url).pathname);
const { captureSessions } = await load('capture');
const { prepare, run } = await load('prompt-lab');
const { executeLab } = await load('lab');
const { loadPrepared, savePrepared } = await load('lab-artifacts');

const model = { provider: 'test', id: 'model', contextWindow: 100000 };
const selection = { provider: model.provider, id: model.id };
const response = (changes) => ({
  role: 'assistant',
  api: 'openai-responses',
  provider: 'test',
  model: 'model',
  timestamp: 1,
  stopReason: 'toolUse',
  usage: {
    input: 0,
    output: 0,
    cacheRead: 0,
    cacheWrite: 0,
    totalTokens: 0,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
  },
  content: [{ type: 'toolCall', id: 'r', name: 'record_card', arguments: { changes } }],
});
const runtime = (complete) => ({
  model,
  modelRegistry: {
    find: (p, id) => (p === model.provider && id === model.id ? model : undefined),
    complete,
  },
});

function fixture() {
  const directory = mkdtempSync(join(tmpdir(), 'streams-primitives-'));
  const root = join(directory, 'debug');
  const manager = SessionManager.inMemory(directory, { id: 'native' });
  const stream = {
    host_id: 'host',
    id: 'effort',
    title: { text: 'Work', manual: false },
    goal: { text: 'Goal', short: null, manual: false },
    status: 'active',
    revision: 1,
    updated_at_ms: 0,
  };
  const session = {
    host_id: 'host',
    harness: 'pi',
    id: 'native',
    stream_id: 'effort',
    title: { text: 'Lane', manual: false },
    status: 'active',
    recap: null,
    state: { text: 'Before', short: null },
    remaining: { text: 'Next', short: null },
    directory: { path: directory, repository: null },
    references: {},
    artifacts: {},
    revision: 1,
    updated_at_ms: 0,
  };
  const catalog = {
    stream,
    session,
    naming: { stream: 'automatic', session: 'automatic' },
  };
  manager.appendMessage({
    role: 'user',
    content: [
      { type: 'text', text: 'Synthetic evidence' },
      { type: 'image', data: 'AA==', mimeType: 'image/png' },
    ],
    timestamp: 1,
  });
  manager.appendMessage({
    ...response({}),
    content: [{ type: 'toolCall', id: 'call', name: 'bash', arguments: { command: 'synthetic' } }],
  });
  manager.appendMessage({
    role: 'toolResult',
    toolCallId: 'call',
    toolName: 'bash',
    content: [{ type: 'text', text: 'Full tool evidence' }],
    isError: false,
    timestamp: 2,
  });
  manager.appendMessage({
    ...response({}),
    stopReason: 'stop',
    content: [{ type: 'text', text: 'Synthetic completion' }],
  });
  manager.appendMessage({
    ...response({}),
    content: [
      {
        type: 'toolCall',
        id: 'pending',
        name: 'bash',
        arguments: { command: 'Incomplete synthetic evidence' },
      },
    ],
  });
  manager.appendMessage({
    role: 'toolResult',
    toolCallId: 'orphan',
    toolName: 'bash',
    content: [{ type: 'text', text: 'Orphan synthetic result' }],
    isError: false,
    timestamp: 3,
  });

  const inventory = {
    version: 2,
    host_id: 'host',
    streams: [{ id: 'effort', key: 'host/effort', title: 'Work' }],
    target: { kind: 'stream', id: 'effort', title: 'Work' },
    samples: [
      {
        host_id: 'host',
        harness: 'pi',
        native_session_id: 'native',
        open: true,
        catalog,
        transcript_file: null,
        unavailable: null,
      },
    ],
  };
  const { capture } = captureSessions(inventory, root, manager);
  const client = {
    getConfig: async () => ({
      naming: { stream: 'automatic', session: 'automatic' },
      pi: { reporting: { debug_commands: true } },
    }),
    listStreams: async () => ({ items: [stream], unreadable: [], more: false }),
    showStream: async () => stream,
    listStreamSessions: async () => ({ items: [session], unreadable: [], more: false }),
  };
  return {
    directory,
    root,
    manager,
    capture,
    client,
    inventory,
    dispose: () => rmSync(directory, { recursive: true, force: true }),
  };
}
const signal = () => new AbortController().signal;

test('one full recording prepares independently trimmed/full requests and prompt variants without mutating capture', () => {
  const f = fixture();
  try {
    const original = structuredClone(f.capture);
    const task = { sample: 0, type: 'session' };
    const a = prepare(f.capture, task, 'Prompt A');
    const b = prepare(f.capture, task, 'Prompt B');
    const full = prepare(f.capture, task, undefined, false);
    assert.deepEqual(a.input, b.input);
    assert.equal(a.model_request.systemPrompt, 'Prompt A');
    assert.equal(b.model_request.systemPrompt, 'Prompt B');
    assert.notEqual(a.input_hash, b.input_hash);
    assert.doesNotMatch(JSON.stringify(a.input.messages), /Full tool evidence|"toolCall"/);
    assert.match(JSON.stringify(full.input.messages), /Full tool evidence|"toolCall"/);
    assert.match(JSON.stringify(a.input.messages), /image\/png/);
    assert.match(
      JSON.stringify(f.capture.samples[0].evidence),
      /Incomplete synthetic evidence|Orphan synthetic result/
    );
    assert.doesNotMatch(
      JSON.stringify(full.model_request),
      /Incomplete synthetic evidence|Orphan synthetic result/
    );
    assert.deepEqual(f.capture, original);
    assert.equal(a.trim, true);
    assert.equal(full.trim, false);
    assert.throws(() => prepare(f.capture, task, '   '), /prompt override/);
    assert.throws(() => prepare(f.capture, { sample: 99, type: 'session' }), /sample index/);
    const legacy = { ...f.capture, context_mode: 'trimmed' };
    assert.throws(() => prepare(legacy, task, undefined, false), /older capture/);
    assert.ok(prepare(legacy, task));
  } finally {
    f.dispose();
  }
});

test('saved requests round-trip, use exact rendered input across reruns, and retain raw invalid responses', async () => {
  const f = fixture();
  try {
    const prepared = prepare(f.capture, { sample: 0, type: 'session' }, 'Saved prompt\nline 2');
    const artifact = savePrepared(f.root, prepared);
    const loaded = loadPrepared(f.root, artifact.id);
    const before = structuredClone(loaded);
    const seen = [];
    for (const state of ['First', 'Second']) {
      const result = await run(
        loaded,
        runtime(async (_m, payload) => {
          seen.push(structuredClone(payload));
          payload.systemPrompt = 'Mutation from provider';
          return response({ state: { text: state, short: null } });
        }),
        model,
        signal()
      );
      assert.equal(result.status, 'completed');
      assert.deepEqual(result.value.changes.state, { text: state, short: null });
    }
    assert.deepEqual(seen, [prepared.model_request, prepared.model_request]);
    assert.deepEqual(loaded, before);

    // Invalid non-object tool output fails cleanly and retains raw response.
    const invalid = {
      ...response({}),
      content: [{ type: 'toolCall', id: 'r', name: 'record_card', arguments: { changes: { status: 'bogus' } } }],
    };
    const failed = await run(loaded, runtime(async () => invalid), model, signal());
    assert.equal(failed.status, 'failed');
    assert.match(failed.error, /invalid structured/);
    assert.deepEqual(failed.raw_response, invalid);
    assert.equal(statSync(join(artifact.directory, 'request.json')).mode & 0o777, 0o600);
  } finally {
    f.dispose();
  }
});

test('capture/prepare/run/inspect adapters need no UI, preserve recordings, and only call models for run', async () => {
  const f = fixture();
  try {
    let calls = 0;
    const environment = {
      root: f.root,
      client: f.client,
      current: f.manager,
      runtime: runtime(async () => {
        calls++;
        return response({ state: { text: 'After', short: null } });
      }),
    };
    const before = readFileSync(join(f.root, 'captures', f.capture.id, 'capture.json'), 'utf8');
    const request = await executeLab(
      { operation: 'prepare', capture_id: f.capture.id, sample: 0, task: 'session', prompt: 'First prompt' },
      environment,
      signal()
    );
    const fork = await executeLab(
      { operation: 'prepare', request_id: request.id, prompt: 'Second prompt' },
      environment,
      signal()
    );
    assert.equal(calls, 0);
    assert.deepEqual(loadPrepared(f.root, request.id).input, loadPrepared(f.root, fork.id).input);
    const result = await executeLab(
      { operation: 'run', request_id: fork.id, model: selection },
      environment,
      signal()
    );
    assert.equal(result.status, 'completed');
    assert.equal(calls, 1);
    assert.ok(result.helper_file);

    const page = await executeLab(
      { operation: 'inspect', kind: 'run', id: result.id, section: 'input', limit: 80 },
      environment,
      signal()
    );
    assert.equal(page.text.length, 80);
    assert.equal(page.next_offset, 80);
    const output = await executeLab(
      { operation: 'inspect', kind: 'run', id: result.id, section: 'output' },
      environment,
      signal()
    );
    assert.match(output.text, /raw_response/);
    assert.match(output.text, /After/);
    const entries = await executeLab({ operation: 'list', kind: 'request' }, environment, signal());
    assert.equal(entries.artifacts.length, 2);
    assert.equal(readFileSync(join(f.root, 'captures', f.capture.id, 'capture.json'), 'utf8'), before);
    assert.doesNotMatch(JSON.stringify(request), /Synthetic evidence|messages|Saved prompt/);
  } finally {
    f.dispose();
  }
});

test('tampered task/schema/input and symlinked manifests fail before inference', async () => {
  const f = fixture();
  try {
    const prepared = prepare(f.capture, { sample: 0, type: 'session' });
    let calls = 0;
    for (const mutate of [
      (v) => {
        v.input.messages = [];
      },
      (v) => {
        v.model_request.tools[0].parameters = { type: 'object' };
      },
      (v) => {
        v.source.native_session_id = 'foreign';
      },
    ]) {
      const artifact = savePrepared(f.root, prepared);
      const path = join(artifact.directory, 'request.json');
      const value = JSON.parse(readFileSync(path, 'utf8'));
      mutate(value);
      writeFileSync(path, JSON.stringify(value));
      await assert.rejects(
        executeLab(
          { operation: 'run', request_id: artifact.id, model: selection },
          { root: f.root, client: f.client, runtime: runtime(() => { calls++; }) },
          signal()
        ),
        /prepared|Prepared|catalog context/
      );
    }
    const artifact = savePrepared(f.root, prepared);
    const path = join(artifact.directory, 'request.json');
    const copy = join(f.directory, 'copy.json');
    writeFileSync(copy, readFileSync(path));
    rmSync(path);
    symlinkSync(copy, path);
    assert.throws(() => loadPrepared(f.root, artifact.id), /regular file|symbolic/);
    assert.equal(calls, 0);
    await assert.rejects(
      executeLab({ operation: 'inspect', kind: 'request', id: '../capture' }, { root: f.root, client: f.client }, signal()),
      /exact artifact/
    );
  } finally {
    f.dispose();
  }
});
