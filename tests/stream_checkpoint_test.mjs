import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { createJiti } from 'jiti';
import { SessionManager } from '@earendil-works/pi-coding-agent';
import { testCli } from './helpers/stream-cli.mjs';

const jiti = createJiti(import.meta.url);
const load = (name) =>
  jiti.import(new URL(`../extensions/stream-checkpoint/${name}.ts`, import.meta.url).pathname);

const {
  buildInferencePayload,
  completedMessages,
  parseInferenceResponse,
  projectMessages,
} = await load('agent');
const {
  applyEditLocally,
  buildSessionReportEdit,
  buildStreamReportEdit,
  summarizeWrittenReferences,
} = await load('core');
const { runReport } = await load('report-workflow');
const { Reporter } = await load('reporter');
const { chooseInitial } = await load('chooser');
const { openCard } = await load('panel');
const { createStreamExtension } = await load('index');

const makeStream = (overrides = {}) => ({
  host_id: 'host',
  id: 'effort',
  title: { text: 'Streams', manual: false },
  goal: { text: 'Keep work legible', short: 'Legible work', manual: false },
  status: 'active',
  revision: 1,
  updated_at_ms: 0,
  ...overrides,
});

const makeSession = (overrides = {}) => ({
  host_id: 'host',
  harness: 'pi',
  id: 'sess-1',
  stream_id: 'effort',
  title: { text: 'CLI', manual: false },
  status: 'active',
  recap: null,
  state: null,
  remaining: null,
  directory: {
    path: '/repo/work',
    repository: { url: 'git@example.test:streams.git', within: '.', branch: 'main' },
  },
  references: {},
  artifacts: {},
  revision: 1,
  updated_at_ms: 0,
  ...overrides,
});

const makeCatalog = (overrides = {}) => ({
  stream: makeStream(),
  session: makeSession(),
  naming: { stream: 'automatic', session: 'automatic' },
  ...overrides,
});

const toolReply = (args) => ({
  role: 'assistant',
  api: 'openai-responses',
  provider: 'test',
  model: 'm',
  timestamp: 1,
  stopReason: 'toolUse',
  usage: {
    input: 1,
    output: 1,
    cacheRead: 0,
    cacheWrite: 0,
    totalTokens: 2,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
  },
  content: [{ type: 'toolCall', id: 'c1', name: 'record_card', arguments: args }],
});

test('completedMessages and projectMessages drop incomplete tool batches and summarize file activity', () => {
  const messages = [
    { role: 'user', content: 'Inspect files', timestamp: 1 },
    {
      role: 'assistant',
      stopReason: 'toolUse',
      content: [{ type: 'toolCall', id: 'ok', name: 'read', arguments: { path: 'src/lib.rs' } }],
      timestamp: 2,
    },
    { role: 'toolResult', toolCallId: 'ok', toolName: 'read', content: [{ type: 'text', text: 'raw file body' }], timestamp: 3 },
    {
      role: 'assistant',
      stopReason: 'toolUse',
      content: [{ type: 'toolCall', id: 'dangling', name: 'read', arguments: { path: 'src/bad.rs' } }],
      timestamp: 4,
    },
    { role: 'toolResult', toolCallId: 'orphan', toolName: 'read', content: [{ type: 'text', text: 'orphan' }], timestamp: 5 },
    { role: 'custom', customType: 'stream-helper-session', content: 'hidden', timestamp: 6 },
  ];

  const completed = completedMessages(messages);
  assert.equal(completed.length, 3);
  const trimmed = projectMessages(messages, 'trimmed');
  assert.equal(trimmed.length, 2);
  assert.match(JSON.stringify(trimmed[1]), /src\/lib\.rs/);
  assert.doesNotMatch(JSON.stringify(trimmed), /raw file body|src\/bad\.rs|orphan/);

  const full = projectMessages(messages, 'full');
  assert.equal(full.length, 3);
  assert.match(JSON.stringify(full), /raw file body/);
});

test('buildInferencePayload respects manual fields, naming policy, and Session-owned references', () => {
  const manualSession = makeCatalog({
    session: makeSession({ title: { text: 'Pinned', manual: true }, directory: null }),
  });
  const payload = buildInferencePayload({ type: 'session', catalog: manualSession, messages: [] });
  const props = Object.keys(payload.tools[0].parameters.properties.changes.properties);
  assert.ok(!props.includes('title'), 'manual session title must not be in automatic schema');
  assert.ok(!props.includes('branch'), 'branch requires directory.repository');
  assert.ok(payload.tools[0].parameters.properties.reference_proposals);
  assert.match(payload.systemPrompt, /belong to this Session/);

  const manualNaming = makeCatalog({ naming: { stream: 'manual', session: 'manual' } });
  const streamPayload = buildInferencePayload({ type: 'stream', catalog: manualNaming, messages: [] });
  const streamProps = Object.keys(streamPayload.tools[0].parameters.properties.changes.properties);
  assert.ok(!streamProps.includes('title'), 'naming.stream=manual omits stream title');
  assert.ok(streamProps.includes('goal'));
});

test('parseInferenceResponse strips disallowed fields and rejects invalid enums', () => {
  const req = { type: 'session', catalog: makeCatalog(), messages: [] };
  const parsed = parseInferenceResponse(
    req,
    toolReply({ changes: { state: 'Working', unknown_field: 'ignored', remaining: '   ' } })
  );
  assert.deepEqual(parsed.changes, { state: 'Working', remaining: null });

  assert.throws(
    () => parseInferenceResponse(req, toolReply({ changes: { status: 'bogus' } })),
    /invalid structured card fields/
  );
});

test('buildSessionReportEdit enforces manual/dropped protection, document containment, and branch updates', () => {
  const session = makeSession({
    title: { text: 'Pinned', manual: true },
    references: {
      'https://example.test/dropped': {
        kind: 'link',
        name: 'Dropped',
        purpose: 'Old',
        role: 'background',
        status: 'dropped',
        manual: false,
      },
      'https://example.test/manual': {
        kind: 'pr',
        name: 'Manual PR',
        purpose: 'Human note',
        role: 'deliverable',
        status: 'active',
        manual: true,
      },
    },
  });
  const notices = [];
  const body = buildSessionReportEdit(
    session,
    { stream: 'automatic', session: 'automatic' },
    {
      title: 'Attempted overwrite',
      status: 'waiting',
      recap: 'Implement format-5 CLI',
      recap_short: 'Format-5 CLI',
      state: 'Waiting on review',
      state_short: 'In review',
      branch: 'cli-by-entity',
    },
    [
      {
        key: 'https://example.test/dropped',
        kind: 'link',
        name: 'Dropped',
        purpose: 'Try restore',
        role: 'background',
        status: 'active',
      },
      {
        key: 'https://example.test/manual',
        kind: 'pr',
        name: 'Manual PR',
        purpose: 'Overwrite',
        role: 'deliverable',
        status: 'fulfilled',
      },
      {
        key: '../outside.md',
        kind: 'document',
        name: 'Outside',
        purpose: 'Escapes dir',
        role: 'background',
        status: 'active',
      },
      {
        key: '/repo/work/docs/plan.md',
        kind: 'document',
        name: 'Plan',
        purpose: 'Design doc',
        role: 'brief',
        status: 'active',
      },
    ],
    notices
  );

  assert.ok(body);
  assert.equal(body.changes.title, undefined, 'manual title skipped');
  assert.equal(body.changes.status, 'waiting');
  assert.deepEqual(body.changes.recap, { text: 'Implement format-5 CLI', short: 'Format-5 CLI' });
  assert.deepEqual(body.changes.directory, {
    path: '/repo/work',
    repository: { url: 'git@example.test:streams.git', within: '.', branch: 'cli-by-entity' },
  });
  // Dropped and escaping proposals were filtered out; manual entry preserves name/purpose/role/manual while allowing status change; valid document added.
  assert.deepEqual(Object.keys(body.changes.references).sort(), [
    'docs/plan.md',
    'https://example.test/manual',
  ]);
  assert.equal(body.changes.references['docs/plan.md'].manual, false);
  assert.equal(body.before.references['docs/plan.md'], null);
  assert.deepEqual(body.changes.references['https://example.test/manual'], {
    kind: 'pr',
    name: 'Manual PR',
    purpose: 'Human note',
    role: 'deliverable',
    status: 'fulfilled',
    manual: true,
  });
  assert.ok(notices.length >= 1);

  const applied = applyEditLocally(session, body);
  assert.deepEqual(applied.result.not_written, {});
  assert.equal(applied.after.references['docs/plan.md'].name, 'Plan');
  assert.equal(applied.after.references['https://example.test/dropped'].status, 'dropped');
  assert.equal(applied.after.references['https://example.test/manual'].purpose, 'Human note');
  assert.equal(applied.after.references['https://example.test/manual'].status, 'fulfilled');
  const refNotices = summarizeWrittenReferences(body, applied.result);
  assert.equal(refNotices.length, 2);
  assert.ok(refNotices.some((n) => n.includes('Plan')));
  assert.ok(refNotices.some((n) => n.includes('fulfilled')));
});

test('buildStreamReportEdit respects manual flags and naming.stream policy', () => {
  const stream = makeStream({
    title: { text: 'Streams', manual: false },
    goal: { text: 'Keep work legible', short: 'Legible', manual: true },
  });
  const body = buildStreamReportEdit(
    stream,
    { stream: 'automatic', session: 'automatic' },
    { title: 'Streams CLI', goal: 'Attempted overwrite' }
  );
  assert.ok(body);
  assert.deepEqual(body.changes, { title: { text: 'Streams CLI', manual: false } });
  assert.deepEqual(body.before, { title: { text: 'Streams', manual: false } });

  // No-op returns null.
  assert.equal(
    buildStreamReportEdit(stream, { stream: 'manual', session: 'automatic' }, { title: 'Blocked' }),
    null
  );
});

test('runReport executes enroll -> session -> stream and logs partial not_written without retrying', async () => {
  let state = { stream: null, session: null, naming: { stream: 'automatic', session: 'automatic' } };
  const calls = [];
  const notices = [];
  const saved = await runReport(
    { kind: 'report', force: true },
    {
      signal: new AbortController().signal,
      automaticEnrollment: true,
      capture: async () => ({
        native: 'sess-new',
        cwd: '/repo/work',
        evidence: { messages: [{ role: 'user', content: 'Build entity CLI' }], leafId: 'm1' },
        catalog: structuredClone(state),
        valid: () => true,
        attached: async () => true,
      }),
      generate: async (req) => {
        calls.push(req.type);
        if (req.type === 'enroll') {
          return { meaningful: true, stream: { title: 'Streams' }, session: { title: 'CLI' } };
        }
        if (req.type === 'session') {
          return { changes: { state: 'Working on Phase 3' }, reference_proposals: [] };
        }
        return { changes: { goal: 'Simplify Streams CLI' } };
      },
      createStream: async (input) => {
        state.stream = makeStream({ title: { text: input.title, manual: false }, goal: null });
        return { key: 'host/effort' };
      },
      createSession: async (id, input) => {
        state.session = makeSession({ id, title: { text: input.title, manual: false } });
        return { key: `host/pi/${id}` };
      },
      editSession: async (key, body) => {
        calls.push(['editSession', key]);
        // Simulate concurrent conflict on `state` -> returned in `not_written`, no retry!
        return { key, not_written: { state: { text: 'Concurrent', short: null } }, partial: true };
      },
      editStream: async (key, body) => {
        calls.push(['editStream', key]);
        const applied = applyEditLocally(state.stream, body);
        state.stream = applied.after;
        return applied.result;
      },
      notice: (text) => notices.push(text),
    }
  );

  assert.equal(saved, true);
  assert.deepEqual(calls, [
    'enroll',
    'session',
    ['editSession', 'host/pi/sess-new'],
    'stream',
    ['editStream', 'host/effort'],
  ]);
  assert.ok(notices.some((n) => n.includes('Session edit skipped:')));
  assert.equal(state.stream.goal.text, 'Simplify Streams CLI');
});

test('Reporter cadence triggers every N responses and respects exclusive lock', async () => {
  const ran = [];
  const manager = SessionManager.inMemory('/repo/work', { id: 'sess-r' });
  manager.appendMessage({ role: 'user', content: 'hi', timestamp: 1 });
  const ctx = { cwd: '/repo/work', sessionManager: manager };
  const reporter = new Reporter({
    cli: () => ({
      context: async () => makeCatalog({ session: makeSession({ id: 'sess-r' }) }),
      editSession: async (k) => ({ key: k, not_written: {} }),
      editStream: async (k) => ({ key: k, not_written: {} }),
    }),
    settings: () => ({
      reporting: 'automatic',
      enrollment: 'automatic',
      frequency: 2,
      context: 'trimmed',
      session_logging: false,
      debug_commands: false,
      pi_name_edits: true,
    }),
    infer: async (_ctx, req) => {
      ran.push(req.type);
      return { changes: {} };
    },
    notice: () => {},
  });

  reporter.response(ctx);
  await reporter.idle();
  assert.deepEqual(ran, []);

  reporter.response(ctx);
  await reporter.idle();
  assert.deepEqual(ran, ['session', 'stream']);
});

test('chooseInitial enrolls against an existing Stream and refuses already-bound sessions', async () => {
  const calls = [];
  let bound = false;
  const cli = {
    context: async () => (bound ? makeCatalog() : { stream: null, session: null, naming: { stream: 'automatic', session: 'automatic' } }),
    listStreams: async () => ({ items: [makeStream()], unreadable: [], more: false }),
    createSession: async (id, input) => {
      calls.push(['createSession', id, input]);
      bound = true;
      return { key: `host/pi/${id}` };
    },
    writeSessionLabel: async (key) => calls.push(['label', key]),
  };
  const ctx = {
    cwd: '/repo/work',
    hasUI: true,
    sessionManager: { getSessionId: () => 'sess-choose' },
    ui: {
      select: async (_title, rows) => rows[0],
      editor: async () => 'My Lane',
      confirm: async () => true,
    },
  };

  const enrolled = await chooseInitial(ctx, cli, {});
  assert.equal(enrolled.status, 'enrolled');
  assert.equal(calls[0][0], 'createSession');
  assert.deepEqual(calls[1], ['label', 'host/pi/sess-choose']);

  const second = await chooseInitial(ctx, cli, {});
  assert.match(second.status, /already-bound/);
});

test('openCard manages Session references (add + drop with manual: true)', async () => {
  const session = makeSession();
  const edits = [];
  const cli = {
    context: async () => makeCatalog({ session: structuredClone(session) }),
    editSession: async (key, body) => {
      edits.push({ key, body });
      const applied = applyEditLocally(session, body);
      Object.assign(session, applied.after);
      return applied.result;
    },
    editStream: async () => assert.fail('stream edit not expected'),
  };

  const selectQueue = [
    'Session · References…',
    'Add reference… · Add a PR, issue, link, or document',
    'URL…',
    'Name…',
    'Purpose…',
    'Save reference',
    '1. Design · Link · Background · Active · https://example.test/design · manual',
    'Drop',
    'Close',
    'Close',
  ];
  const editorQueue = ['https://example.test/design', 'Design', 'Architecture overview'];
  const ctx = {
    hasUI: true,
    sessionManager: { getSessionId: () => 'sess-1' },
    ui: {
      notify: (msg, level) => {
        if (level === 'error') assert.fail(`Unexpected error notify: ${msg}`);
      },
      select: async (_title, rows) => {
        const next = selectQueue.shift();
        assert.ok(rows.includes(next), `Expected ${JSON.stringify(next)} in ${JSON.stringify(rows)}`);
        return next;
      },
      editor: async () => editorQueue.shift(),
      confirm: async () => true,
    },
  };

  await openCard(ctx, cli, { discussion: { input: '', messages: [] }, edits: new Map() }, {
    onResult: () => {},
    discuss: async () => {},
    settings: async () => {},
    report: () => {},
  });

  assert.equal(edits.length, 2);
  assert.equal(edits[0].body.changes.references['https://example.test/design'].manual, true);
  assert.equal(edits[0].body.changes.references['https://example.test/design'].status, 'active');
  assert.equal(edits[1].body.changes.references['https://example.test/design'].status, 'dropped');
  assert.equal(edits[1].body.changes.references['https://example.test/design'].manual, true);
});

test('createStreamExtension registers tools and /streams command and reads real format-5 catalog', async (t) => {
  const root = mkdtempSync(join(tmpdir(), 'streams-ext-e2e-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const workDir = join(root, 'work');
  mkdirSync(workDir, { recursive: true });
  const cli = testCli(workDir);
  const createdStream = await cli.createStream({ title: 'Ext Effort', titleMode: 'manual' });
  await cli.createSession('ext-sess', {
    stream: createdStream.key,
    title: 'Ext Lane',
    titleMode: 'manual',
    directory: workDir,
  });

  const tools = new Map();
  const commands = new Map();
  const handlers = new Map();
  const pi = {
    on: (ev, fn) => handlers.set(ev, fn),
    registerTool: (tool) => tools.set(tool.name, tool),
    registerCommand: (name, cmd) => commands.set(name, cmd),
    appendEntry: () => {},
    getSessionName: () => '',
    setSessionName: () => {},
  };

  // Inject runner that uses our isolated env.
  const { createProcessRunner } = await load('streams');
  const ext = createStreamExtension({
    runner: createProcessRunner(cli.env),
    native: false,
  });
  ext(pi);

  assert.ok(tools.has('stream_read'));
  assert.ok(tools.has('checkpoint'));
  assert.ok(tools.has('select_stream'));
  assert.ok(tools.has('create_stream'));
  assert.ok(tools.has('artifact'));
  assert.ok(commands.has('streams'));

  const ctx = {
    cwd: workDir,
    hasUI: false,
    sessionManager: {
      getSessionId: () => 'ext-sess',
      buildSessionProjection: () => ({ messages: [] }),
    },
    ui: { notify: () => {}, setStatus: () => {} },
  };
  await handlers.get('session_start')({}, ctx);
  const readRes = await tools.get('stream_read').execute('id', {}, new AbortController().signal, undefined, ctx);
  assert.equal(readRes.details.stream.title.text, 'Ext Effort');
  assert.equal(readRes.details.session.title.text, 'Ext Lane');
  assert.equal(readRes.details.session.directory.path, realpathSync(workDir));

  await handlers.get('session_shutdown')({}, ctx);
});
