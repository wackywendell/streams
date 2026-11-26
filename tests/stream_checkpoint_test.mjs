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
  next: null,
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

test('parseInferenceResponse strips disallowed fields, drops non-clearable nulls, and formats errors cleanly', () => {
  const req = { type: 'session', catalog: makeCatalog(), messages: [] };
  const parsed = parseInferenceResponse(
    req,
    toolReply({
      changes: {
        title: null,
        recap: { text: 'Working', short: '   ' },
        branch: null,
        unknown_field: 'ignored',
        next: { text: '   ', short: null },
      },
    })
  );
  assert.deepEqual(parsed.changes, {
    recap: { text: 'Working', short: null },
    next: null,
  });

  // Explicit null on next is preserved (clearable); null on recap and stream goal is dropped.
  const clearNext = parseInferenceResponse(req, toolReply({ changes: { recap: null, next: null } }));
  assert.deepEqual(clearNext.changes, { next: null });

  const streamReq = { type: 'stream', catalog: makeCatalog(), messages: [] };
  const streamParsed = parseInferenceResponse(
    streamReq,
    toolReply({ changes: { title: null, goal: null, status: 'done' } })
  );
  assert.deepEqual(streamParsed.changes, {});

  // Invalid enum produces a single deduplicated summary without anyOf spam.
  assert.throws(
    () => parseInferenceResponse(req, toolReply({ changes: { status: 'bogus' } })),
    (err) => {
      assert.match(err.message, /invalid structured card fields/);
      assert.match(err.message, /changes\.status: expected active \| waiting \| blocked \| parked \| done/);
      assert.doesNotMatch(err.message, /anyOf/);
      return true;
    }
  );

  // Plain-string recap (old flat format) is rejected cleanly.
  assert.throws(
    () => parseInferenceResponse(req, toolReply({ changes: { recap: 'Old flat string' } })),
    (err) => {
      assert.match(err.message, /changes\.recap: expected object/);
      assert.doesNotMatch(err.message, /anyOf/);
      return true;
    }
  );

  // Nested leaf error suppresses parent anyOf/null noise.
  assert.throws(
    () => parseInferenceResponse(req, toolReply({ changes: { next: { text: 'ok', short: 42 } } })),
    (err) => {
      assert.match(err.message, /changes\.next\.short: expected string or null/);
      assert.doesNotMatch(err.message, /anyOf/);
      return true;
    }
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
      recap: { text: 'Implement format-5 CLI', short: 'Format-5 CLI' },
      next: { text: 'Waiting on review', short: 'In review' },
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
    { title: 'Streams CLI', goal: { text: 'Attempted overwrite', short: 'Overwrite' } }
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

test('buildSessionReportEdit never wipes saved recap or branch on null/blank input, while next clears', () => {
  const session = makeSession({
    recap: { text: 'Existing recap', short: 'Recap' },
    next: { text: 'Next step', short: 'Next' },
  });
  const body = buildSessionReportEdit(
    session,
    { stream: 'automatic', session: 'automatic' },
    {
      recap: { text: '   ', short: null },
      next: null,
      branch: '   ',
    },
    []
  );
  assert.ok(body);
  assert.deepEqual(Object.keys(body.changes), ['next']);
  assert.equal(body.changes.next, null);
  assert.deepEqual(body.before.next, { text: 'Next step', short: 'Next' });
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
          return { changes: { next: { text: 'Working on Phase 3', short: null } }, reference_proposals: [] };
        }
        return { changes: { goal: { text: 'Simplify Streams CLI', short: null } } };
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
        // Simulate concurrent conflict on `next` -> returned in `not_written`, no retry!
        return { key, not_written: { next: { text: 'Concurrent', short: null } }, partial: true };
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

test('runReport enroll attaches to an existing candidate Stream via stream_key without creating a new Stream', async () => {
  const existingStream = makeStream({
    id: 'existing-effort',
    title: { text: 'Streams', manual: false },
    goal: { text: 'Build Streams UI', short: 'Streams UI', manual: false },
  });
  const state = {
    session: null,
    stream: null,
    sessions: [],
    references: [],
    artifacts: [],
    naming: { stream: 'automatic', session: 'automatic' },
  };
  const calls = [];
  let createdStreamCount = 0;
  let capturedEnrollRequest = null;

  const saved = await runReport(
    { kind: 'report', force: false },
    {
      signal: new AbortController().signal,
      automaticEnrollment: true,
      capture: async () => ({
        native: 'sess-attach',
        cwd: '/work',
        evidence: { messages: [{ role: 'user', content: 'Update dotfiles tab marks' }], leafId: 'm1' },
        catalog: structuredClone(state),
        valid: () => true,
        attached: async () => true,
      }),
      enrollmentCandidates: async () => ({
        attachedStream: null,
        candidates: [
          {
            key: 'host/existing-effort',
            title: 'Streams',
            goal: 'Build Streams UI',
            status: 'active',
            in_current_space: true,
          },
        ],
      }),
      generate: async (req) => {
        calls.push(req.type);
        if (req.type === 'enroll') {
          capturedEnrollRequest = req;
          const payload = buildInferencePayload(req);
          assert.ok(payload.tools[0].parameters.properties.stream_key);
          const parsed = parseInferenceResponse(
            req,
            {
              role: 'assistant',
              stopReason: 'toolUse',
              content: [
                {
                  type: 'toolCall',
                  id: 'tc1',
                  name: 'record_card',
                  arguments: {
                    meaningful: true,
                    stream_key: 'host/existing-effort',
                    session: { title: 'Marks Migration' },
                  },
                },
              ],
            },
            payload
          );
          return parsed;
        }
        if (req.type === 'session') {
          return { changes: { next: { text: 'Migrating tab marks', short: null } }, reference_proposals: [] };
        }
        return { changes: {} };
      },
      createStream: async () => {
        createdStreamCount += 1;
        return { key: 'host/should-not-happen' };
      },
      createSession: async (id, input) => {
        assert.equal(input.stream, 'host/existing-effort');
        state.stream = existingStream;
        state.session = makeSession({
          id,
          stream_id: 'existing-effort',
          title: { text: input.title, manual: false },
        });
        return { key: `host/pi/${id}` };
      },
      editSession: async (key, body) => {
        calls.push(['editSession', key]);
        const applied = applyEditLocally(state.session, body);
        state.session = applied.after;
        return applied.result;
      },
      editStream: async () => ({ key: 'host/existing-effort', not_written: {}, partial: false }),
      notice: () => {},
    }
  );

  assert.equal(saved, true);
  assert.equal(createdStreamCount, 0);
  assert.equal(capturedEnrollRequest?.candidate_streams?.length, 1);
  assert.equal(state.session.stream_id, 'existing-effort');
  assert.deepEqual(calls, ['enroll', 'session', ['editSession', 'host/pi/sess-attach'], 'stream']);
});

test('runReport enroll rejects an unknown stream_key when no fallback stream is proposed', async () => {
  const state = { session: null, stream: null, sessions: [], references: [], artifacts: [] };
  await assert.rejects(
    runReport(
      { kind: 'report', force: false },
      {
        signal: new AbortController().signal,
        automaticEnrollment: true,
        capture: async () => ({
          native: 'sess-bad-key',
          cwd: '/work',
          evidence: { messages: [{ role: 'user', content: 'Work on something' }], leafId: 'm1' },
          catalog: structuredClone(state),
          valid: () => true,
          attached: async () => true,
        }),
        enrollmentCandidates: async () => ({
          attachedStream: null,
          candidates: [{ key: 'host/real-stream', title: 'Real', status: 'active' }],
        }),
        generate: async () => ({
          meaningful: true,
          stream_key: 'host/hallucinated-stream',
          session: { title: 'Lane' },
        }),
        createStream: async () => {
          throw new Error('should not create');
        },
        createSession: async () => {
          throw new Error('should not create session');
        },
        editSession: async () => ({ key: '', not_written: {}, partial: false }),
        editStream: async () => ({ key: '', not_written: {}, partial: false }),
        notice: () => {},
      }
    ),
    /unknown Stream key/i
  );
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

test('Reporter skips Stream edit if Session moves during Stream inference while keeping Session edits', async () => {
  const manager = SessionManager.inMemory('/repo/work', { id: 'sess-move' });
  manager.appendMessage({ role: 'user', content: 'hi', timestamp: 1 });
  const ctx = { cwd: '/repo/work', sessionManager: manager };
  let currentStreamId = 'effort-a';
  const sessionEdits = [];
  const streamEdits = [];
  const reporter = new Reporter({
    cli: () => ({
      context: async () =>
        makeCatalog({
          stream: makeStream({ id: currentStreamId, title: { text: `Stream ${currentStreamId}`, manual: false } }),
          session: makeSession({ id: 'sess-move', stream_id: currentStreamId }),
        }),
      editSession: async (key, body) => {
        sessionEdits.push({ key, body });
        return { key, not_written: {} };
      },
      editStream: async (key, body) => {
        streamEdits.push({ key, body });
        return { key, not_written: {} };
      },
    }),
    settings: () => ({
      reporting: 'automatic',
      enrollment: 'automatic',
      frequency: 1,
      context: 'trimmed',
      session_logging: false,
      debug_commands: false,
      pi_name_edits: true,
    }),
    infer: async (_ctx, req) => {
      if (req.type === 'session') {
        return { changes: { next: { text: 'Updated next', short: null } }, reference_proposals: [] };
      }
      if (req.type === 'stream') {
        // Session moves to another Stream while Stream inference is in flight.
        currentStreamId = 'effort-b';
        return { changes: { title: 'Stale title for effort-a' } };
      }
      return { changes: {} };
    },
    notice: () => {},
  });

  reporter.response(ctx);
  await reporter.idle();
  assert.equal(sessionEdits.length, 1);
  assert.deepEqual(streamEdits, [], 'must not edit old Stream after Session moved during inference');
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

test('Reporter surfaces automatic failures as info and forced failures as warning', async () => {
  const manager = SessionManager.inMemory('/repo/work', { id: 'sess-err' });
  manager.appendMessage({ role: 'user', content: 'hi', timestamp: 1 });
  const ctx = { cwd: '/repo/work', sessionManager: manager };
  const notices = [];
  const reporter = new Reporter({
    cli: () => ({
      context: async () => makeCatalog({ session: makeSession({ id: 'sess-err' }) }),
      editSession: async (k) => ({ key: k, not_written: {} }),
      editStream: async (k) => ({ key: k, not_written: {} }),
    }),
    settings: () => ({
      reporting: 'automatic',
      enrollment: 'automatic',
      frequency: 1,
      context: 'trimmed',
      session_logging: false,
      debug_commands: false,
      pi_name_edits: true,
    }),
    infer: async () => {
      throw new Error('Reporting model returned invalid structured card fields; nothing was saved (changes.recap: expected object)');
    },
    notice: (text, level) => notices.push({ text, level }),
  });

  reporter.response(ctx);
  await reporter.idle();
  assert.equal(notices.length, 1);
  assert.equal(notices[0].level, 'info');
  assert.match(notices[0].text, /^Automatic report skipped: /);

  reporter.request(ctx, true);
  await reporter.idle();
  assert.equal(notices.length, 2);
  assert.equal(notices[1].level, 'warning');
  assert.doesNotMatch(notices[1].text, /^Automatic report skipped: /);
});

test('derived wire and tool schemas round-trip cleanly against real Rust format-5 CLI', async (t) => {
  const { Check } = await import('typebox/value');
  const { SessionWireSchema, StreamWireSchema } = await load('wire');

  const root = mkdtempSync(join(tmpdir(), 'streams-schema-parity-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const workDir = join(root, 'work');
  mkdirSync(workDir, { recursive: true });
  const cli = testCli(workDir);

  const createdStream = await cli.createStream({
    title: 'Parity Effort',
    titleMode: 'automatic',
    goal: 'Keep schemas aligned',
    goalMode: 'automatic',
  });
  const createdSession = await cli.createSession('parity-sess', {
    stream: createdStream.key,
    title: 'Parity Lane',
    titleMode: 'automatic',
    directory: workDir,
  });

  const beforeCtx = await cli.context('parity-sess');
  assert.equal(Check(StreamWireSchema, beforeCtx.stream), true);
  assert.equal(Check(SessionWireSchema, beforeCtx.session), true);

  // Run a session report payload through parseInferenceResponse -> buildSessionReportEdit -> Rust CLI.
  const sessReq = { type: 'session', catalog: beforeCtx, messages: [] };
  const sessPayload = buildInferencePayload(sessReq);
  const sessParsed = parseInferenceResponse(
    sessReq,
    toolReply({
      changes: {
        title: null,
        status: 'waiting',
        recap: { text: 'Verified schema parity end to end.', short: 'Schema parity' },
        next: { text: 'Merge once approved.', short: 'Merge' },
      },
      reference_proposals: [
        {
          key: 'https://example.test/pr/1',
          kind: 'pr',
          name: 'Parity PR',
          purpose: 'Tracks schema unification',
          role: 'deliverable',
          status: 'active',
        },
      ],
    }),
    sessPayload
  );
  const sessEdit = buildSessionReportEdit(
    beforeCtx.session,
    beforeCtx.naming,
    sessParsed.changes,
    sessParsed.reference_proposals
  );
  const sessRes = await cli.editSession(createdSession.key, sessEdit);
  assert.deepEqual(sessRes.not_written, {});

  // Run a stream report payload through parseInferenceResponse -> buildStreamReportEdit -> Rust CLI.
  const midCtx = await cli.context('parity-sess');
  const streamReq = { type: 'stream', catalog: midCtx, messages: [] };
  const streamPayload = buildInferencePayload(streamReq);
  const streamParsed = parseInferenceResponse(
    streamReq,
    toolReply({
      changes: {
        goal: { text: 'Keep wire and model schemas derived from one source.', short: 'Single schema' },
      },
    }),
    streamPayload
  );
  const streamEdit = buildStreamReportEdit(midCtx.stream, midCtx.naming, streamParsed.changes);
  const streamRes = await cli.editStream(createdStream.key, streamEdit);
  assert.deepEqual(streamRes.not_written, {});

  // Verify discussion reference edit schema matches Rust's ReferenceEntry (no bogus `key` field).
  const discussReq = { type: 'discussion', catalog: midCtx, messages: [], discussion: [] };
  const discussPayload = buildInferencePayload(discussReq);
  const discussParsed = parseInferenceResponse(
    discussReq,
    toolReply({
      reply: 'Added link.',
      edit: {
        kind: 'session',
        changes: {
          references: {
            'https://example.test/doc': {
              kind: 'link',
              name: 'Spec',
              purpose: 'Reference spec',
              role: 'brief',
              status: 'active',
              manual: true,
            },
          },
        },
      },
    }),
    discussPayload
  );
  const { buildBeforeForChanges } = await load('core');
  const discussRes = await cli.editSession(createdSession.key, {
    changes: discussParsed.edit.changes,
    before: buildBeforeForChanges(midCtx.session, discussParsed.edit.changes),
  });
  assert.deepEqual(discussRes.not_written, {});

  // Live records read back from Rust still validate against canonical wire schemas.
  const afterCtx = await cli.context('parity-sess');
  assert.equal(Check(StreamWireSchema, afterCtx.stream), true);
  assert.equal(Check(SessionWireSchema, afterCtx.session), true);
  assert.deepEqual(afterCtx.session.recap, {
    text: 'Verified schema parity end to end.',
    short: 'Schema parity',
  });
  assert.deepEqual(afterCtx.stream.goal, {
    text: 'Keep wire and model schemas derived from one source.',
    short: 'Single schema',
    manual: false,
  });
  assert.equal(afterCtx.session.references['https://example.test/doc'].name, 'Spec');
});

test('/streams sync formats outcome and lifecycle hooks run async start sync and shutdown sync after park', async () => {
  const calls = [];
  const notices = [];
  const entries = [];
  let syncMode = 'fail-start';

  const runner = async (program, args, options) => {
    calls.push({ program, args, stdin: options?.stdin ?? '' });
    if (args[0] === 'config' && args[1] === 'get') {
      return {
        code: 0,
        stdout: JSON.stringify({
          naming: { stream: 'automatic', session: 'automatic' },
          sync: { auto_interval_minutes: 10 },
          herdr: {
            viewer: { mode: 'editor', placement: 'pane-right' },
            naming: { tab: true, workspace: true },
            cards: { auto_open: 'automatic' },
            timeout_ms: 3000,
            pane_height_fraction: 0.2,
          },
          pi: {
            naming: { session: true, import_edits: true },
            reporting: {
              mode: 'automatic',
              enrollment: 'automatic',
              frequency: 1,
              context: 'trimmed',
              model: null,
              session_logging: false,
              debug_commands: false,
            },
          },
        }),
        stderr: '',
      };
    }
    if (args[0] === 'session' && args[1] === 'show') {
      return {
        code: 0,
        stdout: JSON.stringify(makeSession({ id: 's1', status: 'active' })),
        stderr: '',
      };
    }
    if (args[0] === 'stream' && args[1] === 'show') {
      return {
        code: 0,
        stdout: JSON.stringify(makeStream()),
        stderr: '',
      };
    }
    if (args[0] === 'session' && args[1] === 'edit') {
      return { code: 0, stdout: JSON.stringify({ key: 'host-a/pi/s1', not_written: {}, partial: false }), stderr: '' };
    }
    if (args[0] === 'catalog' && args[1] === 'sync') {
      if (syncMode === 'fail-start') {
        return {
          code: 3,
          stdout: JSON.stringify({
            local: 'unchanged',
            sharing: 'failed',
            detail: 'remote unreachable',
          }),
          stderr: '',
        };
      }
      return {
        code: 0,
        stdout: JSON.stringify({
          local: 'committed',
          sharing: 'confirmed',
          detail: '',
        }),
        stderr: '',
      };
    }
    return { code: 0, stdout: '{}', stderr: '' };
  };

  const commands = new Map();
  const handlers = new Map();
  const pi = {
    on: (ev, fn) => handlers.set(ev, fn),
    registerTool: () => {},
    registerCommand: (name, cmd) => commands.set(name, cmd),
    appendEntry: (type, value) => entries.push([type, value]),
    getSessionName: () => '',
    setSessionName: () => {},
  };

  const ext = createStreamExtension({ runner, native: false });
  ext(pi);

  const ctx = {
    cwd: '/tmp/work',
    hasUI: true,
    sessionManager: {
      getSessionId: () => 's1',
      buildSessionProjection: () => ({ messages: [] }),
    },
    ui: {
      notify: (text, level) => notices.push({ text, level }),
      setStatus: () => {},
    },
  };

  // session_start triggers async sync with --if-configured --background --timeout 10.
  await handlers.get('session_start')({}, ctx);
  await new Promise((r) => setTimeout(r, 20));
  const startSyncCall = calls.find((c) => c.args[0] === 'catalog' && c.args[1] === 'sync');
  assert.ok(startSyncCall, `expected start sync call in ${JSON.stringify(calls)}`);
  assert.ok(startSyncCall.args.includes('--if-configured'), JSON.stringify(startSyncCall.args));
  assert.ok(startSyncCall.args.includes('--background'), JSON.stringify(startSyncCall.args));
  assert.ok(startSyncCall.args.includes('--timeout'), JSON.stringify(startSyncCall.args));
  assert.equal(entries.length, 1);
  assert.equal(entries[0][0], 'stream-catalog-sync');
  assert.equal(entries[0][1].event, 'start');
  assert.equal(entries[0][1].sharing, 'failed');
  assert.ok(
    notices.some(
      (n) => n.level === 'warning' && n.text.includes('failed (local: unchanged)') && n.text.includes('remote unreachable')
    ),
    JSON.stringify(notices)
  );

  // Manual `/streams sync` shows warning on needs_attention, then info on confirmed.
  notices.length = 0;
  await commands.get('streams').handler('sync', ctx);
  assert.equal(notices.length, 1);
  assert.equal(notices[0].level, 'warning');
  assert.equal(notices[0].text, 'Catalog sync: failed (local: unchanged) — remote unreachable');

  syncMode = 'ok';
  notices.length = 0;
  await commands.get('streams').handler('sync', ctx);
  assert.equal(notices.length, 1);
  assert.equal(notices[0].level, 'info');
  assert.equal(notices[0].text, 'Catalog sync: confirmed (local: committed)');

  // session_shutdown awaits catalog sync with --if-configured (blocking, no --background) AFTER marking park.
  calls.length = 0;
  await handlers.get('session_shutdown')({}, ctx);
  const parkIdx = calls.findIndex(
    (c) => c.args[0] === 'session' && c.args[1] === 'edit' && c.stdin.includes('"parked"')
  );
  const syncIdx = calls.findIndex((c) => c.args[0] === 'catalog' && c.args[1] === 'sync');
  assert.ok(parkIdx !== -1, `expected park call in ${JSON.stringify(calls)}`);
  assert.ok(syncIdx > parkIdx, `expected shutdown sync after park: ${JSON.stringify(calls)}`);
  assert.ok(calls[syncIdx].args.includes('--if-configured'), JSON.stringify(calls[syncIdx].args));
  assert.ok(!calls[syncIdx].args.includes('--background'), JSON.stringify(calls[syncIdx].args));
  assert.ok(calls[syncIdx].args.includes('--timeout'), JSON.stringify(calls[syncIdx].args));
});
