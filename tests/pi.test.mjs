import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, open, readFile, readdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { registerWorkCards } from '../dist/pi/index.js';
import { createCard, readCard, updateCard } from '../dist/card.js';
import { DEFAULT_CONFIG } from '../dist/config.js';
import { buildSessionProjection } from '@earendil-works/pi-coding-agent';

const tick = () => new Promise(resolve => setImmediate(resolve));
async function until(predicate) {
  const deadline = performance.now() + 2000;
  while (!(await predicate())) {
    if (performance.now() > deadline) assert.fail('Expected asynchronous adapter effect did not occur');
    await tick();
  }
}
const card = title => ({ title, status: 'active', state: '', cwd: '/repo', documents: [], issues: [], prs: [] });
const answer = decision => ({ stopReason: 'stop', content: [{ type: 'text', text: JSON.stringify(decision) }] });

async function host(t, config = {}) {
  const directory = await mkdtemp(join(tmpdir(), 'work-cards-pi-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const handlers = new Map(), commands = new Map(), tools = new Map();
  const notices = [], attachments = [], detachments = [], calls = [];
  let branch = [], messages = [], sessionId = 'session-1', leaf = 'root';
  let modelResult = async () => answer({ action: 'no_change' });
  let attachError, detachError, configError, attachWait;
  let configuredDirectory = directory;
  const env = { HERDR_ENV: '1', HERDR_PANE_ID: 'pane-1', HERDR_SOCKET_PATH: '/socket' };
  const settings = { ...DEFAULT_CONFIG, model: 'fake/simple', ...config };
  const ctx = {
    cwd: '/repo', hasUI: true,
    ui: { notify: (text, level) => notices.push({ text, level }) },
    sessionManager: {
      getSessionId: () => sessionId, getBranch: () => structuredClone(branch),
      getLeafId: () => leaf, getSessionFile: () => '/session.jsonl',
      buildSessionProjection: () => ({ messages: structuredClone(messages), entries: [] }),
    },
    modelRegistry: {
      find: (provider, id) => provider === 'fake' && id === 'simple' ? { api: 'fake', id } : undefined,
      streamSimple: (model, context, options) => {
        calls.push({ model, context: structuredClone(context), options });
        return { result: () => modelResult(options) };
      },
    },
  };
  const pi = {
    on: (name, fn) => handlers.set(name, fn),
    registerCommand: (name, def) => commands.set(name, def),
    registerTool: def => tools.set(def.name, def),
    appendEntry: (customType, data) => {
      leaf = `custom-${branch.length}`;
      branch.push({ type: 'custom', customType, data: structuredClone(data), id: leaf });
    },
  };
  registerWorkCards(pi, {
    cardDirectory: () => configuredDirectory, loadConfig: async () => { if (configError) throw new Error(configError); return { ...settings }; }, env,
    attachCard: async snapshot => { attachments.push(snapshot); if (attachWait) await attachWait; if (attachError) throw new Error(attachError); },
    detachCard: async snapshot => { detachments.push(snapshot); if (detachError) throw new Error(detachError); },
  });
  const emit = async (name, event = {}) => { await handlers.get(name)?.({ type: name, ...event }, ctx); await tick(); };
  const command = async args => { await commands.get('work-card').handler(args, ctx); await tick(); };
  const evidence = (id, text = 'Implemented the requested fix') => {
    leaf = id;
    messages = [{ role: 'user', content: 'Fix this' }, { role: 'assistant', content: [{ type: 'text', text }, { type: 'thinking', thinking: 'SECRET' }, { type: 'toolCall', arguments: { secret: 'ARGS' } }] }];
    branch.push({ type: 'message', id, message: messages.at(-1) });
  };
  await emit('session_start');
  return { directory, notices, attachments, detachments, calls, settings, env, ctx, tools, command, emit, evidence,
    branch: () => structuredClone(branch), setBranch: value => { branch = structuredClone(value); leaf = branch.at(-1)?.id ?? null; },
    setSession: value => { sessionId = value; }, setMessages: value => { messages = value; },
    setResult: value => { modelResult = value; }, setAttachError: value => { attachError = value; },
    setDetachError: value => { detachError = value; }, setConfigError: value => { configError = value; },
    setAttachWait: value => { attachWait = value; },
    setDirectory: value => { configuredDirectory = value; },
    current: () => tools.get('work_card').execute('call', {}, new AbortController().signal, undefined, ctx),
  };
}

test('only explicit new/open binds; initial card and read-only tool contain source and references', async t => {
  const h = await host(t);
  assert.equal((await h.current()).details, null);
  assert.equal(h.attachments.length, 0);
  await h.command('new Fix cache');
  const snapshot = await readCard('fix-cache', h.directory);
  assert.deepEqual(snapshot.card, card('Fix cache'));
  assert.equal(h.attachments.length, 1);
  assert.deepEqual(h.branch().at(-1).data, { slug: 'fix-cache', sessionId: 'session-1', path: snapshot.path, updates: true });
  const result = await h.current();
  assert.equal(result.details.path, snapshot.path);
  assert.deepEqual(result.details.card.documents, []);
  assert.match(h.tools.get('work_card').description, /read.only/i);
  assert.match(h.tools.get('work_card').description, /ADRs.*project checkout/);
  assert.match(result.content[0].text, /metadata only references them/);
  assert.deepEqual(h.tools.get('work_card').parameters.properties, {});
  await h.command('show');
  assert.match(h.notices.at(-1).text, /Fix cache/);
  await h.command('open fix-cache');
  assert.equal(h.attachments.length, 2);
});

test('bare open without a binding gives explicit recovery commands without mutation', async t => {
  const h = await host(t);
  delete h.env.HERDR_ENV;
  await h.command('open');
  assert.match(h.notices.at(-1).text, /No work card/i);
  assert.match(h.notices.at(-1).text, /\/work-card open <slug>/);
  assert.match(h.notices.at(-1).text, /\/work-card new <title>/);
  assert.deepEqual(await readdir(h.directory), []);
  assert.deepEqual(h.branch(), []);
  assert.equal(h.attachments.length, 0);
  assert.equal(h.detachments.length, 0);
  await h.command('new');
  assert.match(h.notices.at(-1).text, /Usage:.*open \[slug\]/);
});

test('bare open rereads the saved target rather than cwd or a changed configured directory', async t => {
  const h = await host(t), other = await host(t);
  await h.command('new Fresh');
  const initial = await readCard('fresh', h.directory);
  const fresh = await updateCard('fresh', { state: 'Externally edited' }, initial.revision, h.directory);
  await createCard({ ...card('Fresh'), state: 'Different directory' }, other.directory);
  h.setDirectory(other.directory);
  h.ctx.cwd = '/unrelated-checkout';
  await h.command('open');
  assert.equal(h.attachments.length, 2);
  assert.deepEqual(h.attachments.at(-1), fresh);
  assert.deepEqual(h.branch().at(-1).data, { slug: 'fresh', sessionId: 'session-1', path: initial.path, updates: true });
  await h.command('open fresh');
  assert.equal(h.attachments.at(-1).path, join(other.directory, 'fresh.yaml'));
  assert.equal((await h.current()).details.card.state, 'Different directory');
});

test('binding retains only its target while show, tool and checks read fresh card data', async t => {
  const h = await host(t);
  await h.command('new Fresh');
  const initial = await readCard('fresh', h.directory);
  await updateCard('fresh', { state: 'Externally edited' }, initial.revision, h.directory);
  await h.command('show');
  assert.match(h.notices.at(-1).text, /Externally edited/);
  assert.equal((await h.current()).details.card.state, 'Externally edited');
  h.evidence('fresh-evidence');
  await h.emit('agent_settled', { aborted: false });
  await until(() => h.calls.length === 1);
  assert.equal(JSON.parse(h.calls[0].context.messages[0].content[0].text).card.state, 'Externally edited');
  await h.command('detach');
  assert.deepEqual(h.detachments, [{ path: initial.path }]);
});

test('current-binding deleted YAML cleanup uses its saved target without rereading the card', async t => {
  const h = await host(t);
  await h.command('new Deleted');
  const path = join(h.directory, 'deleted.yaml');
  await rm(path);
  await assert.rejects(h.current(), /ENOENT/);
  const saved = h.branch();
  await h.command('open');
  assert.match(h.notices.at(-1).text, /ENOENT/);
  assert.deepEqual(h.branch(), saved);
  assert.equal(h.attachments.length, 1);
  assert.deepEqual(await readdir(h.directory), []);
  await h.command('detach');
  assert.deepEqual(h.detachments, [{ path }]);
  assert.deepEqual(h.branch().at(-1).data, { slug: null, sessionId: 'session-1' });
  assert.equal((await h.current()).details, null);
});

test('missing YAML before restore remains unbound and does not invent cleanup ownership', async t => {
  const h = await host(t);
  await h.command('new Gone');
  await rm(join(h.directory, 'gone.yaml'));
  await h.emit('session_start', { reason: 'resume' });
  assert.equal((await h.current()).details, null);
  await h.command('detach');
  assert.equal(h.detachments.length, 0);
  assert.equal(h.attachments.length, 1);
});

test('a binding change during a read invalidates the returned tool snapshot', async t => {
  const h = await host(t);
  await h.command('new Read race');
  const handle = await open(join(h.directory, 'read-race.yaml'), 'r');
  const prototype = Object.getPrototypeOf(handle), read = prototype.readFile;
  await handle.close();
  let entered = false, release;
  const suspended = new Promise(resolve => { release = resolve; });
  t.mock.method(prototype, 'readFile', async function (...args) {
    entered = true;
    await suspended;
    return read.apply(this, args);
  });
  const pending = h.current();
  const rejected = assert.rejects(pending, /binding changed during read/);
  try {
    await until(() => entered);
    await h.command('detach');
  } finally { release(); }
  await rejected;
});

test('outside Herdr creation is refused before writing; show remains available', async t => {
  const h = await host(t);
  delete h.env.HERDR_ENV;
  await h.command('new Would orphan');
  await assert.rejects(readCard('would-orphan', h.directory), /ENOENT/);
  assert.match(h.notices.at(-1).text, /Herdr/);
  await h.command('show');
  assert.match(h.notices.at(-1).text, /No work card/i);
});

test('attach failures preserve prior binding and report newly created card as partial', async t => {
  const h = await host(t);
  await h.command('new Existing');
  h.setAttachError('endpoint mismatch; inspect pane');
  await h.command('new Partial');
  assert.equal((await h.current()).details.slug, 'existing');
  assert.equal((await readCard('partial', h.directory)).card.title, 'Partial');
  assert.match(h.notices.at(-1).text, /partial.*created|created.*partial/i);
  assert.match(h.notices.at(-1).text, /partial.yaml/);
  assert.equal(h.branch().length, 1);
});

test('resume restores exact path without pane opening; fork and changed path never inherit', async t => {
  const h = await host(t);
  await h.command('new Durable');
  const saved = h.branch();
  await h.emit('session_start', { reason: 'resume' });
  assert.equal((await h.current()).details.slug, 'durable');
  assert.equal(h.attachments.length, 1);
  await h.command('open');
  assert.equal(h.attachments.length, 2);
  assert.equal((await h.current()).details.slug, 'durable');
  const resumed = h.branch();
  h.setSession('fork');
  await h.emit('session_start', { reason: 'fork' });
  assert.equal((await h.current()).details, null);
  await h.command('open');
  assert.match(h.notices.at(-1).text, /No work card/i);
  assert.equal(h.attachments.length, 2);
  assert.deepEqual(h.branch(), resumed);
  await h.command('open durable');
  assert.equal((await h.current()).details.slug, 'durable');
  h.setSession('session-1');
  saved[0].data.path = '/different-state/durable.yaml';
  h.setBranch(saved);
  await h.emit('session_tree');
  assert.equal((await h.current()).details, null);
  await h.command('open');
  assert.match(h.notices.at(-1).text, /No work card/i);
  assert.equal(h.attachments.length, 3);
});

test('detach failure stops automatic work but preserves explicit cleanup; tombstone wins on tree restore', async t => {
  const h = await host(t);
  await h.command('new Cleanup');
  h.setDetachError('close failed');
  await h.command('detach');
  assert.equal((await h.current()).details.slug, 'cleanup');
  h.evidence('e1');
  await h.emit('agent_settled', { aborted: false });
  assert.equal(h.calls.length, 0);
  h.setDetachError(undefined);
  await h.command('detach');
  assert.equal(h.detachments.length, 2);
  assert.equal(h.branch().at(-1).data.slug, null);
  await h.emit('session_tree');
  assert.equal((await h.current()).details, null);
  const saved = h.branch();
  await h.command('open');
  assert.match(h.notices.at(-1).text, /No work card/i);
  assert.deepEqual(h.branch(), saved);
  assert.equal(h.attachments.length, 1);
});

test('failed detach persists pause across reload and tree restore until explicit open', async t => {
  const h = await host(t);
  await h.command('new Paused');
  h.setDetachError('close failed');
  await h.command('detach');
  const snapshot = await readCard('paused', h.directory);
  assert.deepEqual(h.branch().at(-1).data, { slug: 'paused', sessionId: 'session-1', path: snapshot.path, updates: false });
  for (const event of ['session_start', 'session_tree']) {
    await h.emit(event, { reason: 'reload' });
    assert.equal((await h.current()).details.slug, 'paused');
    h.evidence(event);
    await h.emit('agent_settled', { aborted: false });
    for (let i = 0; i < 5; i++) await readCard('paused', h.directory);
    assert.equal(h.calls.length, 0);
    assert.equal(h.attachments.length, 1);
  }
  const saved = h.branch();
  h.setAttachError('attachment failed');
  await h.command('open');
  assert.match(h.notices.at(-1).text, /attachment failed/);
  assert.deepEqual(h.branch(), saved);
  assert.equal((await h.current()).details.slug, 'paused');
  h.evidence('failed-open');
  await h.emit('agent_settled', { aborted: false });
  for (let i = 0; i < 5; i++) await readCard('paused', h.directory);
  assert.equal(h.calls.length, 0);
  h.setAttachError(undefined);
  await h.command('open');
  assert.equal(h.branch().at(-1).data.updates, true);
  h.evidence('explicit-open');
  await h.emit('agent_settled', { aborted: false });
  await until(() => h.calls.length === 1);
  h.setDetachError(undefined);
  await h.command('detach');
  assert.equal(h.branch().at(-1).data.slug, null);
});

test('restore enables legacy bindings but fails closed on malformed update flags', async t => {
  const h = await host(t);
  await h.command('new Flags');
  const saved = h.branch();
  delete saved[0].data.updates;
  h.setBranch(saved);
  await h.emit('session_start', { reason: 'resume' });
  h.evidence('legacy');
  await h.emit('agent_settled', { aborted: false });
  await until(() => h.calls.length === 1);
  for (const updates of ['true', 1, null, undefined]) {
    saved[0].data.updates = updates;
    h.setBranch(saved);
    await h.emit('session_start', { reason: 'reload' });
    assert.equal((await h.current()).details.slug, 'flags');
    h.evidence(`malformed-${updates}`);
    await h.emit('agent_settled', { aborted: false });
    for (let i = 0; i < 5; i++) await readCard('flags', h.directory);
    assert.equal(h.calls.length, 1);
    assert.equal(h.attachments.length, 1);
  }
});

test('detach cannot persist a pause for a different originating session', async t => {
  const h = await host(t);
  await h.command('new Origin');
  const saved = h.branch();
  h.setSession('replacement');
  await h.command('detach');
  assert.deepEqual(h.branch(), saved);
  assert.equal(h.detachments.length, 0);
  await h.command('open');
  assert.match(h.notices.at(-1).text, /No work card/i);
  assert.deepEqual(h.branch(), saved);
  assert.equal(h.attachments.length, 1);
});

test('completed cycles and response boundaries deduplicate evidence and apply helper patch without transcript entries', async t => {
  const h = await host(t, { check_every_n_cycles: 1, check_every_n_responses: 1 });
  await h.command('new Summarize');
  h.setResult(async () => answer({ action: 'update', patch: { state: 'Fixed cache invalidation' } }));
  h.evidence('e1');
  await h.emit('turn_end', { outcome: 'completed' });
  await h.emit('agent_settled', { aborted: false });
  await until(async () => (await readCard('summarize', h.directory)).card.state === 'Fixed cache invalidation');
  assert.equal(h.calls.length, 1);
  assert.equal((await readCard('summarize', h.directory)).card.state, 'Fixed cache invalidation');
  assert.equal(h.branch().filter(e => e.customType === 'work-cards.binding').length, 1);
  const { context, options } = h.calls[0];
  assert.deepEqual(context.tools, []);
  assert.equal(options.toolChoice, 'none');
  assert.equal(options.maxTokens, DEFAULT_CONFIG.max_output_tokens);
  assert.doesNotMatch(JSON.stringify(context), /SECRET|ARGS/);
});

test('no_change preserves revision; zero triggers never call even an unavailable helper', async t => {
  const h = await host(t, { check_every_n_cycles: 0, check_every_n_responses: 0, model: 'missing/helper' });
  await h.command('new Disabled');
  const before = await readCard('disabled', h.directory);
  h.evidence('e1');
  await h.emit('turn_end', { outcome: 'completed' });
  await h.emit('agent_settled', { aborted: false });
  assert.equal(h.calls.length, 0);
  assert.equal((await readCard('disabled', h.directory)).revision, before.revision);
  h.settings.model = 'fake/simple';
  h.settings.check_every_n_responses = 1;
  await h.emit('session_start', { reason: 'reload' });
  h.evidence('e2');
  await h.emit('agent_settled', { aborted: false });
  await until(() => h.calls.length === 1);
  assert.equal(h.calls.length, 1);
  assert.equal((await readCard('disabled', h.directory)).revision, before.revision);
});

test('ordinary newer evidence does not cancel running helper; latest eligible evidence coalesces', async t => {
  const h = await host(t);
  await h.command('new Coalesce');
  let release;
  h.setResult(() => new Promise(resolve => { release = resolve; }));
  h.evidence('e1');
  await h.emit('agent_settled', { aborted: false });
  await until(() => h.calls.length === 1);
  const first = h.calls[0];
  h.evidence('e2');
  await h.emit('agent_settled', { aborted: false });
  h.evidence('e3', 'Latest evidence');
  await h.emit('agent_settled', { aborted: false });
  assert.equal(first.options.signal.aborted, false);
  assert.equal(h.calls.length, 1);
  h.setResult(async () => answer({ action: 'no_change' }));
  release(answer({ action: 'update', patch: { state: 'Accepted first check' } }));
  await until(() => h.calls.length === 2);
  assert.equal((await readCard('coalesce', h.directory)).card.state, 'Accepted first check');
  assert.equal(h.calls.length, 2);
  assert.match(JSON.stringify(h.calls[1].context), /Latest evidence/);
});

test('session changes cancel stale helpers before update, without shutdown detach', async t => {
  const h = await host(t);
  await h.command('new Stale');
  let release;
  h.setResult(() => new Promise(resolve => { release = resolve; }));
  h.evidence('e1');
  await h.emit('agent_settled', { aborted: false });
  await until(() => h.calls.length === 1);
  await h.emit('session_before_switch');
  assert.equal(h.calls[0].options.signal.aborted, true);
  release(answer({ action: 'update', patch: { state: 'Must not land' } }));
  for (let n = 0; n < 5; n++) await tick();
  assert.equal((await readCard('stale', h.directory)).card.state, '');
  await h.emit('session_shutdown');
  assert.equal(h.detachments.length, 0);
});

test('session switch during temporary file sync cancels the core commit and cleans up', async t => {
  const h = await host(t);
  await h.command('new Commit boundary');
  const before = await readCard('commit-boundary', h.directory);
  const bytes = await readFile(before.path);
  const handle = await open(before.path, 'r');
  const prototype = Object.getPrototypeOf(handle);
  const sync = prototype.sync;
  await handle.close();
  let entered = false, release;
  const suspended = new Promise(resolve => { release = resolve; });
  t.mock.method(prototype, 'sync', async function () {
    entered = true;
    await suspended;
    return sync.call(this);
  });
  h.setResult(async () => answer({ action: 'update', patch: { state: 'Must not commit' } }));
  h.evidence('e1');
  try {
    await h.emit('agent_settled', { aborted: false });
    await until(() => entered);
    await h.emit('session_before_switch');
    assert.equal(h.calls[0].options.signal.aborted, true);
  } finally { release(); }
  await until(async () => (await readdir(h.directory)).length === 1);
  assert.deepEqual(await readFile(before.path), bytes);
  assert.deepEqual(await readCard('commit-boundary', h.directory), before);
  assert.deepEqual(await readdir(h.directory), ['commit-boundary.yaml']);
  assert.equal(h.notices.filter(n => n.level === 'warning').length, 0);
});

test('optimistic conflicts preserve external edits and errors warn once without retry', async t => {
  const h = await host(t);
  await h.command('new Conflict');
  let release;
  h.setResult(() => new Promise(resolve => { release = resolve; }));
  h.evidence('e1');
  await h.emit('agent_settled', { aborted: false });
  await until(() => h.calls.length === 1);
  const before = await readCard('conflict', h.directory);
  await updateCard('conflict', { state: 'Manual edit' }, before.revision, h.directory);
  release(answer({ action: 'update', patch: { state: 'Stale overwrite' } }));
  await until(() => h.notices.some(n => /revision|reread/i.test(n.text)));
  assert.equal((await readCard('conflict', h.directory)).card.state, 'Manual edit');
  assert.match(h.notices.at(-1).text, /revision|reread/i);
  h.setResult(async () => answer({ action: 'update', patch: { title: 'Forbidden' } }));
  h.evidence('e2');
  await h.emit('agent_settled', { aborted: false });
  await until(() => h.notices.some(n => /unknown|title/i.test(n.text) && n.level === 'warning'));
  const warnings = h.notices.filter(n => n.level === 'warning').length;
  h.evidence('e3');
  await h.emit('agent_settled', { aborted: false });
  await until(() => h.calls.length === 3);
  assert.equal(h.notices.filter(n => n.level === 'warning').length, warnings);
});

test('aborted settlement resets cycle counts without counting a successful response', async t => {
  const h = await host(t, { check_every_n_cycles: 2, check_every_n_responses: 2 });
  await h.command('new Boundaries');
  h.evidence('e1');
  await h.emit('turn_end', { outcome: 'completed' });
  await h.emit('agent_settled', { aborted: true });
  h.evidence('e2');
  await h.emit('turn_end', { outcome: 'error' });
  await h.emit('turn_end', { outcome: 'aborted' });
  await h.emit('turn_end', { outcome: 'completed' });
  await h.emit('agent_settled', { aborted: false });
  assert.equal(h.calls.length, 0);
  h.evidence('e3');
  await h.emit('agent_settled', { aborted: false });
  await until(() => h.calls.length === 1);
});

test('show does not cancel accepted work; config changes cancel and reapply without overlap', async t => {
  const h = await host(t);
  await h.command('new Reconfigure');
  let release;
  h.setResult(() => new Promise(resolve => { release = resolve; }));
  h.evidence('e1');
  await h.emit('agent_settled', { aborted: false });
  await until(() => h.calls.length === 1);
  await h.command('show');
  assert.equal(h.calls[0].options.signal.aborted, false);
  h.settings.max_output_tokens = 99;
  h.settings.request_timeout_ms = 1500;
  h.settings.thinking_level = 'high';
  await h.command('show');
  assert.equal(h.calls[0].options.signal.aborted, true);
  h.setResult(async () => answer({ action: 'no_change' }));
  h.evidence('e2');
  await h.emit('agent_settled', { aborted: false });
  assert.equal(h.calls.length, 1);
  release(answer({ action: 'update', patch: { state: 'Canceled' } }));
  await until(() => h.calls.length === 2);
  assert.equal(h.calls[1].options.maxTokens, 99);
  assert.equal(h.calls[1].options.reasoning, 'high');
  assert.equal((await readCard('reconfigure', h.directory)).card.state, '');
});

test('stale attachments never bind to a replacement session and report the durable partial file', async t => {
  const h = await host(t);
  let release;
  h.setAttachWait(new Promise(resolve => { release = resolve; }));
  const pending = h.command('new During switch');
  await until(() => h.attachments.length === 1);
  h.setSession('replacement');
  await h.emit('session_start', { reason: 'new' });
  release();
  await pending;
  assert.equal((await h.current()).details, null);
  assert.equal(h.branch().length, 0);
  assert.equal((await readCard('during-switch', h.directory)).card.title, 'During switch');
  assert.match(h.notices.at(-1).text, /partial.*session|session.*partial/i);
});

test('bare open cannot save a binding after lifecycle changes during read or attachment', async t => {
  for (const stage of ['read', 'attach']) {
    const h = await host(t);
    await h.command('new Reopen race');
    const saved = h.branch();
    let entered = false, release, mocked;
    const suspended = new Promise(resolve => { release = resolve; });
    if (stage === 'read') {
      const handle = await open(join(h.directory, 'reopen-race.yaml'), 'r');
      const prototype = Object.getPrototypeOf(handle), read = prototype.readFile;
      await handle.close();
      mocked = t.mock.method(prototype, 'readFile', async function (...args) {
        entered = true;
        await suspended;
        return read.apply(this, args);
      });
    } else {
      h.setAttachWait(suspended);
    }
    const pending = h.command('open');
    try {
      await until(() => stage === 'read' ? entered : h.attachments.length === 2);
      h.setSession('replacement');
      await h.emit('session_start', { reason: 'new' });
    } finally {
      release();
      mocked?.mock.restore();
    }
    await pending;
    assert.equal((await h.current()).details, null);
    assert.deepEqual(h.branch(), saved);
    assert.equal(h.attachments.length, stage === 'read' ? 1 : 2);
  }
});

test('tree navigation and explicit binding replacement cancel a stale helper', async t => {
  const h = await host(t);
  await h.command('new First');
  let release;
  h.setResult(() => new Promise(resolve => { release = resolve; }));
  h.evidence('e1');
  await h.emit('agent_settled', { aborted: false });
  await until(() => h.calls.length === 1);
  await h.emit('session_before_tree');
  assert.equal(h.calls[0].options.signal.aborted, true);
  h.setBranch([]);
  await h.emit('session_tree');
  await h.command('new Second');
  release(answer({ action: 'update', patch: { state: 'Wrong target' } }));
  await until(async () => (await h.current()).details.slug === 'second');
  assert.equal((await readCard('first', h.directory)).card.state, '');
  assert.equal((await readCard('second', h.directory)).card.state, '');
});

test('projected summary and uncapped dialogue exclude tools and respect host redactions', async t => {
  const h = await host(t, { max_context_chars: 1 });
  await h.command('new Dialogue');
  h.evidence('e1', 'RAW_SECRET');
  const userText = 'EARLIEST_REQUEST'.repeat(4000);
  const projected = [
    { role: 'compactionSummary', summary: 'Earlier milestone' },
    { role: 'user', content: userText },
    { role: 'assistant', content: [
      { type: 'text', text: 'Confirmed new ADR docs/cache.md' },
      { type: 'text', text: 'Implementation remains unfinished' },
      { type: 'thinking', thinking: 'PRIVATE THINKING' },
      { type: 'toolCall', arguments: { password: 'PRIVATE ARGS' } },
    ] },
    { role: 'toolResult', content: [
      { type: 'text', text: 'PRIVATE TOOL'.repeat(24000) },
      { type: 'image', data: 'PRIVATE IMAGE' },
    ] },
  ];
  const before = structuredClone(projected);
  h.setMessages(projected);
  h.ctx.sessionManager.getEntries = () => { throw new Error('Raw history is forbidden'); };
  await h.emit('agent_settled', { aborted: false });
  await until(() => h.calls.length === 1);
  const context = h.calls[0].context;
  const data = JSON.parse(context.messages[0].content[0].text);
  assert.deepEqual(data, {
    compactionSummary: 'Earlier milestone',
    transcript: [
      { role: 'user', text: userText },
      { role: 'assistant', text: 'Confirmed new ADR docs/cache.md\nImplementation remains unfinished' },
    ],
    card: card('Dialogue'),
  });
  assert.doesNotMatch(JSON.stringify(context), /RAW_SECRET|PRIVATE/);
  assert.deepEqual(projected, before);
});

test('summary-only changes participate in revision hashing at the same dialogue leaf', async t => {
  const h = await host(t);
  await h.command('new Summary revision');
  h.evidence('same-leaf');
  const dialogue = [{ role: 'user', content: 'Same dialogue' }];
  h.setMessages([{ role: 'compactionSummary', summary: 'First milestone' }, ...dialogue]);
  await h.emit('agent_settled', { aborted: false });
  await until(() => h.calls.length === 1);
  h.setMessages([{ role: 'compactionSummary', summary: 'Changed milestone' }, ...dialogue]);
  await h.emit('agent_settled', { aborted: false });
  await until(() => h.calls.length === 2);
  assert.equal(JSON.parse(h.calls[1].context.messages[0].content[0].text).compactionSummary, 'Changed milestone');
  await h.emit('agent_settled', { aborted: false });
  for (let i = 0; i < 10; i++) await readCard('summary-revision', h.directory);
  assert.equal(h.calls.length, 2);
});

test('actual SDK historical projection excludes precompaction, future and other branches and applies edits', async t => {
  const h = await host(t);
  await h.command('new Historical');
  const timestamp = '2026-01-01T00:00:00.000Z';
  const message = (id, parentId, role, content) => ({ type: 'message', id, parentId, timestamp, message: { role, content, timestamp: 0 } });
  const entries = [
    message('pre', null, 'user', 'PRECOMPACTION_SECRET'),
    { type: 'compaction', id: 'old-compaction', parentId: 'pre', timestamp, firstKeptEntryId: 'pre', summary: 'OLD_SUMMARY_SECRET', tokensBefore: 100 },
    message('kept', 'old-compaction', 'user', 'REDACTED_SECRET'),
    message('deleted', 'kept', 'assistant', [{ type: 'text', text: 'DELETED_SECRET' }]),
    { type: 'compaction', id: 'compaction', parentId: 'deleted', timestamp, firstKeptEntryId: 'kept', summary: 'Actual projected checkpoint', tokensBefore: 200 },
    message('outcome', 'compaction', 'assistant', [{ type: 'text', text: 'Retained outcome' }]),
    { type: 'context_edit', id: 'edit', parentId: 'outcome', timestamp, targetId: 'kept', replacement: { content: 'Redacted request' } },
    { type: 'context_edit', id: 'historical-leaf', parentId: 'edit', timestamp, targetId: 'deleted', replacement: null },
    message('future', 'historical-leaf', 'user', 'FUTURE_SECRET'),
    message('other-branch', 'compaction', 'user', 'OTHER_BRANCH_SECRET'),
  ];
  const before = structuredClone(entries);
  h.ctx.sessionManager.buildSessionProjection = () => buildSessionProjection(entries, 'historical-leaf');
  h.ctx.sessionManager.getBranch = () => { throw new Error('Do not reconstruct raw history'); };
  h.ctx.sessionManager.getEntries = () => { throw new Error('Do not read raw history'); };
  await h.emit('agent_settled', { aborted: false });
  await until(() => h.calls.length === 1);
  assert.deepEqual(JSON.parse(h.calls[0].context.messages[0].content[0].text), {
    compactionSummary: 'Actual projected checkpoint',
    transcript: [
      { role: 'user', text: 'Redacted request' },
      { role: 'assistant', text: 'Retained outcome' },
    ],
    card: card('Historical'),
  });
  assert.doesNotMatch(JSON.stringify(h.calls[0].context), /SECRET/);
  assert.deepEqual(entries, before);
});

test('provider context-window errors warn without silently trimming or retrying', async t => {
  const h = await host(t);
  await h.command('new Provider limit');
  const before = await readCard('provider-limit', h.directory);
  h.evidence('e1', 'Outcome '.repeat(6000));
  h.setResult(async () => { throw new Error('Provider context window exceeded'); });
  await h.emit('agent_settled', { aborted: false });
  await until(() => h.notices.some(n => n.level === 'warning' && /context window exceeded/.test(n.text)));
  assert.ok(h.calls[0].context.messages[0].content[0].text.length > 24000);
  await h.emit('agent_settled', { aborted: false });
  for (let i = 0; i < 10; i++) await readCard('provider-limit', h.directory);
  assert.equal(h.calls.length, 1);
  assert.deepEqual(await readCard('provider-limit', h.directory), before);
});

test('configuration and missing-model failures are visible without fallback; tool read failures throw', async t => {
  const h = await host(t);
  await h.command('new Errors');
  h.setConfigError('Invalid work-cards config /config/work-cards.json: unknown field');
  await h.command('show');
  assert.match(h.notices.at(-1).text, /work-cards.json/);
  h.evidence('e1');
  await h.emit('agent_settled', { aborted: false });
  assert.equal(h.calls.length, 0);
  h.setConfigError(undefined);
  h.settings.model = 'missing/simple';
  await h.command('show');
  h.evidence('e2');
  await h.emit('agent_settled', { aborted: false });
  await until(() => h.notices.some(n => /not registered/.test(n.text)));
  assert.equal(h.calls.length, 0);
  await rm(join(h.directory, 'errors.yaml'));
  await assert.rejects(h.current(), /ENOENT/);
  const aborted = new AbortController();
  aborted.abort();
  await assert.rejects(h.tools.get('work_card').execute('id', {}, aborted.signal, undefined, h.ctx), /abort/i);
});

test('read/show restored binding works outside Herdr and never opens or selects a pane', async t => {
  const h = await host(t);
  await h.command('new Read only');
  delete h.env.HERDR_ENV;
  await h.emit('session_start', { reason: 'resume' });
  await h.command('show');
  assert.match(h.notices.at(-1).text, /read-only.yaml/);
  assert.equal((await h.current()).details.slug, 'read-only');
  await h.command('open');
  assert.match(h.notices.at(-1).text, /Herdr/);
  await h.command('open read-only');
  assert.match(h.notices.at(-1).text, /Herdr/);
  assert.equal(h.attachments.length, 1);
  assert.equal((await h.current()).details.slug, 'read-only');
});

test('new preserves supplied title spacing and rejects multiline titles instead of silently normalizing', async t => {
  const h = await host(t);
  await h.command('new Keep  spacing');
  assert.equal((await readCard('keep-spacing', h.directory)).card.title, 'Keep  spacing');
  await h.command('new Invalid\nmultiline');
  await assert.rejects(readCard('invalid-multiline', h.directory), /ENOENT/);
  assert.equal((await h.current()).details.slug, 'keep-spacing');
});

test('non-evidence session entries cannot duplicate a cycle-final helper check', async t => {
  const h = await host(t, { check_every_n_cycles: 1 });
  await h.command('new Dedupe');
  h.evidence('e1');
  await h.emit('turn_end', { outcome: 'completed' });
  await until(() => h.calls.length === 1);
  h.setBranch([...h.branch(), { type: 'custom', customType: 'another-extension.state', data: {}, id: 'unrelated-leaf' }]);
  await h.emit('agent_settled', { aborted: false });
  // The core read is the first asynchronous step in any wrongly duplicated request.
  for (let i = 0; i < 10; i++) await readCard('dedupe', h.directory);
  assert.equal(h.calls.length, 1);
});

test('detach stops updates even when configuration loading fails and keeps cleanup binding', async t => {
  const h = await host(t);
  await h.command('new Stop');
  h.setConfigError('Invalid config');
  await h.command('detach');
  assert.equal((await h.current()).details.slug, 'stop');
  assert.equal(h.branch().at(-1).data.updates, false);
  assert.equal(h.detachments.length, 0);
  h.setConfigError(undefined);
  await h.emit('session_start', { reason: 'reload' });
  await h.command('show');
  assert.equal((await h.current()).details.slug, 'stop');
  assert.equal(h.attachments.length, 1);
  h.evidence('e1');
  await h.emit('agent_settled', { aborted: false });
  for (let i = 0; i < 5; i++) await readCard('stop', h.directory);
  assert.equal(h.calls.length, 0);
  await h.command('detach');
  assert.equal((await h.current()).details, null);
});
