import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, realpath, rm, open, writeFile, readFile, copyFile, symlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { registerWorkCards } from '../dist/pi/index.js';
import { RecordStore, referenceIdentity } from '../dist/records.js';
import { DEFAULT_CONFIG } from '../dist/config.js';
import { WorkspaceClient, agentAuthority } from '../dist/herdr/workspace-client.js';
import { AgentSession, SessionManager, SettingsManager } from '@earendil-works/pi-coding-agent';
import { Agent } from '@earendil-works/pi-agent-core';
const { createExtensionRuntime, loadExtensionFromFactory } = await import(new URL('./core/extensions/loader.js', import.meta.resolve('@earendil-works/pi-coding-agent')));
const tick = () => new Promise(resolve => setImmediate(resolve));
async function until(predicate) {
  const deadline = performance.now() + 4000;
  while (!(await predicate())) { if (performance.now() > deadline) assert.fail('Expected asynchronous host effect'); await tick(); }
}
function deferred() { let resolve; const promise = new Promise(r => { resolve = r; }); return { promise, resolve }; }
const user = text => ({ role: 'user', content: text });
async function host(t, config = {}, initial = {}) {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'wc-pi-')));
  const cwd = join(root, 'checkout'); await mkdir(cwd);
  const directory = join(root, 'state'), store = new RecordStore(directory), socket = join(root, 'herdr.sock');
  const workspaces = ['w', 'dest'].map(workspace_id => ({ workspace_id, tokens: {} }));
  const tabs = ['w', 'dest'].map(workspace_id => ({ workspace_id, tab_id: `${workspace_id}:tab`, number: 1, label: 'Manual tab' }));
  const panes = [{ pane_id: 'w:pi', terminal_id: 'pi', workspace_id: 'w', tab_id: 'w:tab', tokens: {} }];
  const native = [], calls = [], notices = [], statuses = [], handlers = new Map(), commands = new Map(), tools = new Map();
  let branch = initial.branch ?? [], messages = initial.messages ?? [], sessionId = initial.sessionId ?? 'session-1', next = 1;
  let configError, configWait, hook = async () => {}, result = async () => ({ action: 'no_change' });
  const env = { HERDR_ENV: '1', HERDR_PANE_ID: 'old-public-id', HERDR_SOCKET_PATH: socket };
  const settings = { ...DEFAULT_CONFIG, model: 'fake/simple', ...config };
  const run = async args => {
    native.push(args); await hook(args);
    const reply = (type, fields) => JSON.stringify({ result: { type, ...fields } });
    if (args[0] === 'status') return JSON.stringify({ server: { running: true, compatible: true, endpoint_compatible: true, version: '0.9.4', socket } });
    if (args[0] === 'api') return reply('session_snapshot', { snapshot: { workspaces, tabs, panes, layouts: tabs.map(tab => ({ ...tab, panes: panes.filter(p => p.tab_id === tab.tab_id).map(p => ({ pane_id: p.pane_id })) })) } });
    if (args[0] === 'pane' && args[1] === 'current') return reply('pane_current', { pane: panes.find(p => p.terminal_id === 'pi') });
    if (args[0] === 'pane' && args[1] === 'resize') return reply('pane_resize', { resize: { pane_id: args[3] } });
    if (args[1] === 'report-metadata') {
      const target = (args[0] === 'pane' ? panes : workspaces).find(p => (p.pane_id ?? p.workspace_id) === args[2]);
      for (let i = 5; i < args.length; i += 2) { const [key, ...v] = args[i + 1].split('='); if (args[i] === '--clear-token') delete target.tokens[key]; else target.tokens[key] = v.join('='); }
      return '';
    }
    if (args[0] === 'plugin' && args[1] === 'list') return reply('plugin_list', { plugins: [] });
    if (args[0] === 'plugin' && args[1] === 'link') return reply('plugin_linked', { plugin: { plugin_id: 'work-cards', enabled: true, plugin_root: root, manifest_path: join(root, 'herdr-plugin.toml'), source: { kind: 'local' }, version: '0.1.0', min_herdr_version: '0.9.3', platforms: ['macos', 'linux'], panes: [{ id: 'card', title: 'Work card', placement: 'split', command: ['node', 'dist/herdr/pane.js'] }, { id: 'document', title: 'Document', placement: 'tab', command: ['/bin/sh', 'bin/open-document.sh'] }] } });
    if (args[0] === 'plugin' && args[2] === 'open') {
      const owner = panes.find(p => p.pane_id === args[args.indexOf('--target-pane') + 1]);
      const pane = { ...owner, pane_id: `card${next}`, terminal_id: `card${next++}`, tokens: {} }; panes.push(pane);
      return reply('plugin_pane_opened', { plugin_pane: { plugin_id: 'work-cards', entrypoint: 'card', pane } });
    }
    if (args[0] === 'plugin' && args[2] === 'close') { panes.splice(panes.findIndex(p => p.pane_id === args[3]), 1); return reply('plugin_pane_closed', { pane_id: args[3] }); }
    throw new Error(`Unexpected native command ${args.join(' ')}`);
  };
  await writeFile(join(root, `${sessionId}.jsonl`), `${JSON.stringify({ type: 'session', version: 3, id: sessionId, cwd, timestamp: new Date().toISOString() })}\n`);
  const client = new WorkspaceClient({ env, run, pluginRoot: root });
  const ctx = { cwd, mode: 'tui', hasUI: true,
    ui: { notify: (text, level) => notices.push({ text, level }), setStatus: (key, text) => statuses.push({ key, text }) },
    sessionManager: { getSessionId: () => sessionId, getSessionFile: () => join(root, `${sessionId}.jsonl`), getBranch: () => structuredClone(branch), getEntries: () => structuredClone(branch), buildSessionProjection: () => ({ messages: structuredClone(messages), entries: [] }) },
    modelRegistry: { find: () => ({ api: 'fake' }), streamSimple: (_model, context, options) => {
      const data = JSON.parse(context.messages[0].content[0].text); const kind = 'agent' in data ? 'own' : 'ownCheckout' in data ? 'shared' : 'documents' in data ? 'select' : 'goal';
      calls.push({ kind, data, options }); return { result: async () => ({ stopReason: 'stop', content: [{ type: 'text', text: JSON.stringify(await result(kind, options)) }] }) };
    } },
  };
  const pi = { on: (name, fn) => handlers.set(name, fn), registerCommand: (name, def) => commands.set(name, def), registerTool: def => tools.set(def.name, def), appendEntry: (customType, data) => branch.push({ type: 'custom', customType, data: structuredClone(data) }) };
  const overrides = { recordDirectory: () => directory, loadConfig: async () => { if (configWait) await configWait; if (configError) throw new Error(configError); return { ...settings }; }, client, env };
  const install = target => registerWorkCards(target, overrides); install(pi);
  const emit = async (name, event = {}) => { await handlers.get(name)?.({ type: name, ...event }, ctx); await tick(); };
  const command = async args => { await commands.get('work-card').handler(args, ctx); await tick(); };
  t.after(async () => { await emit('session_shutdown', { reason: 'quit' }); await rm(root, { recursive: true, force: true, maxRetries: 3, retryDelay: 10 }); });
  const h = { root, cwd, store, directory, env, client, native, calls, notices, statuses, settings, handlers, tools, ctx, install, command, emit,
    setMessages: value => { messages = value; }, setSession: value => { sessionId = value; }, setBranch: value => { branch = value; }, branch: () => structuredClone(branch),
    setResult: value => { result = value; }, setHook: value => { hook = value; }, setConfigError: value => { configError = value; }, setConfigWait: value => { configWait = value; },
    launches: () => native.filter(a => a[0] === 'plugin' && a[2] === 'open'),
    closes: () => native.filter(a => a[0] === 'plugin' && a[2] === 'close'),
    read: () => store.readAgent(sessionId),
    current: () => tools.get('work_card').execute('call', {}, new AbortController().signal, undefined, ctx),
    q: () => { const i = panes.findIndex(p => p.tokens.wca_role === 'card'); if (i >= 0) panes.splice(i, 1); },
    move: (both = true) => { for (const p of panes.filter(p => p.terminal_id === 'pi' || both)) { p.workspace_id = 'dest'; p.tab_id = 'dest:tab'; p.pane_id = `dest:${p.terminal_id}`; } },
    panes, workspaces,
  };
  if (!initial.skipStart) await emit('session_start', { reason: 'startup' });
  return h;
}
async function attach(t, config = {}) { const h = await host(t, config); await h.command('new Agent task'); return h; }

test('factory does no I/O; empty/inherited-only startup ignores all old binding entries', async t => {
  const h = await host(t, {}, { skipStart: true, messages: [user('Inherited task')], branch: [{ type: 'custom', customType: 'work-cards.binding', data: { sessionId: 'session-1', slug: 'old' } }] });
  assert.equal(h.native.length, 0); assert.equal(h.calls.length, 0);
  await h.emit('session_start'); assert.equal((await h.current()).details, null);
  await h.emit('context'); await tick(); assert.deepEqual(await h.store.listAgents(), []);
  h.setMessages([user('Inherited task'), user('Fresh task')]); await h.emit('context');
  await until(async () => (await h.store.listAgents()).length === 1 && h.launches().length === 1);
  const a = await h.read(); assert.equal(a.record.sessionId, 'session-1'); assert.equal(a.record.boundary.transcriptLength, 1);
});

test('startup baseline precedes awaited configuration; queued fresh dialogue is not inherited evidence', async t => {
  const h = await host(t, {}, { skipStart: true, messages: [user('Inherited')] });
  const wait = deferred(); h.setConfigWait(wait.promise); const starting = h.emit('session_start');
  await tick(); h.setMessages([user('Inherited'), user('Fresh task')]); await h.emit('context'); wait.resolve(); await starting;
  await until(() => h.launches().length === 1); assert.equal((await h.read()).record.boundary.transcriptLength, 1);
});

test('cancellation at the final file sync prevents the host summary rename', async t => {
  const h = await attach(t);
  const handle = await open(join(h.root, 'probe'), 'w'); const prototype = Object.getPrototypeOf(handle), original = prototype.sync; await handle.close();
  let cancelled = false;
  t.after(() => { prototype.sync = original; });
  h.setResult(async () => {
    prototype.sync = async function() { await original.call(this); if (!cancelled) { cancelled = true; await h.emit('session_before_fork'); } };
    return { action: 'update', patch: { now: 'Not committed' } };
  });
  await h.command('refresh'); await until(() => cancelled); await tick();
  assert.notEqual((await h.read()).record.now, 'Not committed');
  prototype.sync = original;
});

test('new attaches own agent, preserves shared title and returns both fresh records and references read-only', async t => {
  const h = await attach(t); const a = await h.read(); const workspace = await h.store.readWorkspace(a.record.workspaceId);
  assert.equal(a.record.titleMode, 'manual'); assert.equal(workspace.record.title, 'Agent task');
  await h.store.addReferences(agentAuthority(a.record), { documents: [{ checkout: a.record.checkout, path: 'missing.md', description: 'Plan' }] });
  await h.emit('session_before_tree');
  const before = h.native.length, result = await h.current(); assert.equal(h.native.length, before);
  assert.equal(result.details.agent.path, a.path); assert.equal(result.details.workspace.path, workspace.path);
  assert.equal(result.details.references.documents[0].target, join(h.cwd, 'missing.md'));
  assert.match(result.content[0].text, /ADRs.*project checkout/); assert.match(h.tools.get('work_card').description, /read.only/i);
  await h.command('new Renamed own task'); assert.equal((await h.read()).record.title, 'Renamed own task');
  assert.equal((await h.store.readWorkspace(workspace.record.id)).record.title, 'Agent task'); assert.equal(h.launches().length, 1);
  await h.command('show'); assert.match(h.notices.at(-1).text, /Renamed own task/);
});

test('manual startup waits for explicit attachment; attached summaries still run', async t => {
  const h = await host(t, { workspace_startup: 'manual' }); h.setMessages([user('Implement task')]); await h.emit('context'); await tick();
  assert.deepEqual(await h.store.listAgents(), []); await h.command('open'); await until(() => h.calls.some(c => c.kind === 'own'));
  assert.equal(h.launches().length, 1); h.move(); await h.emit('context'); await tick();
  const before = (await h.read()).record.workspaceId; await h.command('refresh'); assert.equal((await h.read()).record.workspaceId, before);
  await h.command('open'); assert.notEqual((await h.read()).record.workspaceId, before);
});

test('UUID-preserving resume/tree reset boundaries; fresh fork inherits no assignment or saved pause', async t => {
  const h = await attach(t); h.setMessages([user('Old evidence')]); const first = await h.read();
  await h.emit('session_tree'); const tree = await h.read(); assert.notEqual(tree.record.evidenceGeneration, first.record.evidenceGeneration); assert.equal(tree.record.boundary.transcriptLength, 1);
  await h.emit('session_start', { reason: 'resume' }); assert.notEqual((await h.read()).record.evidenceGeneration, tree.record.evidenceGeneration); assert.equal(h.launches().length, 1);
  await h.command('detach'); h.setSession('fork-uuid'); await h.emit('session_start', { reason: 'fork' });
  assert.equal((await h.current()).details, null); await h.emit('context'); await tick(); await assert.rejects(h.store.readAgent('fork-uuid'), e => e.code === 'ENOENT');
});

test('normalized SDK projections drive helpers without awaiting model completion or duplicate settled checks', async t => {
  const h = await attach(t, { check_every_n_responses: 0 }); await until(() => h.calls.length > 0); await tick();
  const wait = deferred(); h.setResult(async () => { await wait.promise; return { action: 'no_change' }; });
  h.setMessages([user('USER'), { role: 'assistant', content: [{ type: 'text', text: 'ANSWER' }, { type: 'thinking', thinking: 'SECRET' }, { type: 'toolCall', arguments: { secret: 'ARGS' } }] }, { role: 'toolResult', content: [{ type: 'text', text: 'TOOL' }] }, { role: 'custom', content: 'CUSTOM' }]);
  const before = h.calls.length; await h.command('refresh'); await until(() => h.calls.length > before);
  const own = h.calls.at(-1); assert.deepEqual(own.data.transcript, [{ role: 'user', text: 'USER' }, { role: 'assistant', text: 'ANSWER' }]);
  await h.emit('turn_end', { outcome: 'completed' }); await h.emit('agent_settled', { aborted: false }); assert.equal(h.calls.length, before + 1);
  await h.emit('session_before_tree'); assert.equal(own.options.signal.aborted, true); wait.resolve();
});

test('real SDK historical projection, compaction and context edits determine helper evidence', async t => {
  const h = await host(t, {}, { skipStart: true });
  const manager = SessionManager.inMemory(h.cwd);
  manager.appendMessage(user('Summarized private history'));
  const kept = manager.appendMessage(user('Unredacted user secret'));
  manager.appendCompaction('Verified checkpoint', kept, 100);
  manager.appendContextEdit(kept, { content: 'Projected user text' });
  manager.appendMessage({ role: 'assistant', content: [{ type: 'text', text: 'Abandoned branch secret' }] });
  manager.branch(kept);
  manager.appendCompaction('Verified checkpoint', kept, 100);
  manager.appendContextEdit(kept, { content: 'Projected user text' });
  manager.appendMessage({ role: 'assistant', content: [{ type: 'text', text: 'Projected answer' }, { type: 'thinking', thinking: 'Private thinking' }, { type: 'toolCall', id: 'call', name: 'bash', arguments: { secret: 'Private arguments' } }] });
  manager.appendMessage({ role: 'toolResult', toolCallId: 'call', toolName: 'bash', content: [{ type: 'text', text: 'Private output' }] });
  h.ctx.sessionManager = manager;
  await h.emit('session_start'); await h.command('new Projected task');
  await until(() => h.calls.some(c => c.kind === 'shared'));
  for (const kind of ['own', 'shared']) {
    const data = h.calls.find(c => c.kind === kind).data;
    assert.equal(data.compactionSummary, 'Verified checkpoint');
    assert.deepEqual(data.transcript, [{ role: 'user', text: 'Projected user text' }, { role: 'assistant', text: 'Projected answer' }]);
    assert.doesNotMatch(JSON.stringify(data), /Summarized private history|Unredacted user secret|Abandoned branch secret|Private thinking|Private arguments|Private output/);
  }
  assert.match(JSON.stringify(manager.getEntries()), /Unredacted user secret|Abandoned branch secret/);
});

test('helper requests are single-flight across cancellation and configuration replacement, with spinner cleanup', async t => {
  const h = await attach(t, { show_refresh_spinner: true }); await until(() => h.calls.length > 0); await tick();
  const wait = deferred(); let active = 0, max = 0, requests = 0;
  h.setResult(async () => { const request = ++requests; active++; max = Math.max(max, active); await wait.promise; active--; return request === 1 ? { action: 'update', patch: { now: 'MUST NOT COMMIT' } } : { action: 'no_change' }; });
  await h.command('refresh'); await until(() => active === 1); const call = h.calls.at(-1);
  assert.ok(h.statuses.some(s => s.text)); await h.emit('session_before_switch'); assert.equal(call.options.signal.aborted, true); assert.equal(h.statuses.at(-1).text, undefined);
  h.settings.check_every_n_responses = 0; await h.command('refresh');
  try { await new Promise(resolve => setTimeout(resolve, 120)); assert.equal(max, 1); }
  finally { wait.resolve(); }
  await until(() => requests > 1 && active === 0);
  assert.equal(max, 1); assert.notEqual((await h.read()).record.now, 'MUST NOT COMMIT');
});

test('timed-out noncooperative provider holds queued force until actual rejection settlement', async t => {
  const h = await attach(t, { request_timeout_ms: 30, show_refresh_spinner: true });
  await until(() => h.calls.length > 0); await new Promise(resolve => setTimeout(resolve, 50));
  const wait = deferred(); let starts = 0;
  h.setResult(async () => { starts++; if (starts === 1) { await wait.promise; throw new Error('late provider failure'); } return { action: 'no_change' }; });
  await h.command('refresh'); await until(() => starts === 1);
  await until(() => h.statuses.at(-1)?.text === undefined);
  await h.command('refresh');
  try { await new Promise(resolve => setTimeout(resolve, 120)); assert.equal(starts, 1); }
  finally { wait.resolve(); }
  await until(() => starts > 1);
});

test('q suppresses polls/reload/refresh respawn but does not pause summaries; explicit open reopens', async t => {
  const h = await attach(t, { pane_poll_interval_ms: 15 }); h.q();
  await h.command('refresh'); await h.emit('session_start', { reason: 'reload' });
  await new Promise(resolve => setTimeout(resolve, 45)); assert.equal(h.launches().length, 1);
  await h.command('open'); assert.equal(h.launches().length, 2);
});

test('idle polls reconcile native movement without scheduling model checks', async t => {
  const h = await attach(t, { pane_poll_interval_ms: 15 }); await until(() => h.calls.length > 0); await tick();
  const before = h.calls.length, old = (await h.read()).record.workspaceId; h.move();
  await until(async () => (await h.read()).record.workspaceId !== old);
  assert.equal(h.calls.length, before);
  await h.emit('session_shutdown', { reason: 'quit' }); const count = h.native.length;
  await new Promise(resolve => setTimeout(resolve, 40)); assert.equal(h.native.length, count); assert.equal(h.closes().length, 0);
});

test('detach persists pause before fallible own cleanup and survives tree/reload until explicit open', async t => {
  const h = await attach(t); h.setHook(async args => { if (args[0] === 'plugin' && args[2] === 'close') { assert.equal(h.branch().at(-1).data.updates, false); throw new Error('close failed'); } });
  await h.command('detach'); const saved = h.branch().at(-1); assert.equal(saved.customType, 'work-cards.workspace'); assert.equal(saved.data.sessionId, 'session-1');
  assert.match(h.notices.at(-1).text, /close failed/); const before = h.calls.length;
  await h.emit('session_tree'); await h.emit('session_start', { reason: 'reload' }); h.setMessages([user('Not resumed')]); await h.emit('context'); await tick(); assert.equal(h.calls.length, before);
  h.setHook(async () => {}); await h.command('detach'); assert.equal(h.panes.some(p => p.tokens.wca_role === 'card'), false);
  await h.command('open'); assert.equal(h.branch().at(-1).data.updates, true); assert.equal(h.launches().length, 2);
});

test('unnamed open after reset recovers documents and workspace naming from retained dialogue', async t => {
  for (const workspace_startup of ['automatic', 'manual']) await t.test(workspace_startup, async t => {
    const messages = [user('Implement query parsing using docs/design.md')];
    const h = await host(t, { workspace_startup, check_every_n_responses: 0 }, { messages });
    await mkdir(join(h.cwd, 'docs'));
    await writeFile(join(h.cwd, 'docs/design.md'), 'Provide reusable query parsing');
    h.setResult(async kind => kind === 'select' ? { sources: [] } : { action: 'no_change' });
    await h.command('new Old manual title');
    await until(async () => (await h.current()).details?.refresh?.state === 'complete');
    await h.command('detach');
    await rm(h.directory, { recursive: true, force: true });
    for (const workspace of h.workspaces) workspace.tokens = {};
    const before = h.calls.length;
    await h.emit('session_start', { reason: 'reload' });
    assert.equal(h.branch().at(-1).data.updates, false);
    assert.deepEqual(await h.store.listAgents(), []); assert.equal(h.calls.length, before);
    h.setResult(async kind => {
      const identity = { checkout: (await h.read()).record.checkout, path: 'docs/design.md' };
      if (kind === 'own') return { action: 'update', patch: { title: 'Query parsing' } };
      if (kind === 'shared') return { action: 'update', additions: { documents: [{ ...identity, description: 'Query parsing design' }] } };
      if (kind === 'select') return { sources: [identity] };
      return { goal: 'Provide reusable query parsing', goalShort: 'Query parsing', title: 'Query parsing workspace' };
    });
    await h.command('open');
    assert.equal((await h.current()).details?.updates, true, JSON.stringify(h.notices));
    await until(async () => (await h.current()).details?.refresh?.state === 'complete');
    const agent = await h.read(), workspace = await h.store.readWorkspace(agent.record.workspaceId);
    assert.equal(agent.record.title, 'Query parsing'); assert.equal(agent.record.titleMode, 'auto');
    assert.equal(agent.record.boundary.transcriptLength, 1);
    assert.equal(workspace.record.titleMode, 'auto'); assert.equal(workspace.record.title, 'Query parsing workspace');
    assert.equal(workspace.record.goal, 'Provide reusable query parsing');
    assert.deepEqual(workspace.record.documents.map(d => d.path), ['docs/design.md']);
    const calls = h.calls.slice(before);
    assert.deepEqual(calls.map(c => c.kind), ['own', 'shared', 'select', 'goal']);
    assert.deepEqual(calls[0].data.transcript, [{ role: 'user', text: messages[0].content }]);
    assert.deepEqual(calls[1].data.transcript, calls[0].data.transcript);
    assert.equal(h.launches().length, 2); assert.equal(h.notices.some(n => n.level === 'warning'), false);
  });
});

test('implicit creation and existing-workspace joining both consider retained session references', async t => {
  for (const existing of [false, true]) await t.test(existing ? 'join existing' : 'implicit creation', async t => {
    const messages = [user('The query parsing design is in plan.md')];
    const h = await host(t, {}, { messages });
    await writeFile(join(h.cwd, 'plan.md'), 'Provide reusable query parsing');
    if (existing) await h.store.createWorkspace({ id: 'known', creationToken: 'known-token', title: 'Parser project' });
    h.setResult(async kind => {
      const identity = { checkout: (await h.read()).record.checkout, path: 'plan.md' };
      if (kind === 'own') return { action: 'no_change' };
      if (kind === 'shared') return { action: 'update', additions: { documents: [{ ...identity, description: 'Query parsing design' }] } };
      if (kind === 'select') return { sources: [identity] };
      return { goal: 'Provide reusable query parsing', goalShort: 'Query parsing', title: 'Parser project' };
    });
    if (existing) await h.command('open known');
    else {
      h.setMessages([...messages, user('Continue query parsing')]);
      await h.emit('context'); await h.emit('agent_settled', { aborted: false });
    }
    await until(async () => (await h.current()).details?.refresh?.state === 'complete');
    const shared = h.calls.find(c => c.kind === 'shared').data;
    assert.deepEqual(shared.transcript[0], { role: 'user', text: messages[0].content });
    assert.equal(shared.workspace.title, existing ? 'Parser project' : 'Continue query parsing');
    const agent = await h.read(), workspace = await h.store.readWorkspace(agent.record.workspaceId);
    if (existing) assert.equal(workspace.record.id, 'known');
    assert.deepEqual(workspace.record.documents.map(d => d.path), ['plan.md']);
    assert.equal(workspace.record.goal, 'Provide reusable query parsing');
    assert.equal(h.launches().length, 1);
  });
});

test('detach persists pause without reading even an unattached or missing agent record', async t => {
  for (const attached of [false, true]) await t.test(String(attached), async t => {
    const h = attached ? await attach(t) : await host(t);
    const original = RecordStore.prototype.readAgent;
    RecordStore.prototype.readAgent = async () => { throw new Error('record read unavailable'); };
    try {
      await h.command('detach');
      const saved = h.branch().at(-1);
      assert.equal(saved?.data.updates, false);
      assert.equal(saved.data.sessionId, 'session-1');
      assert.equal(saved.data.store, h.directory);
      if (attached) assert.ok(saved.data.cleanup);
    } finally { RecordStore.prototype.readAgent = original; }
    const count = h.calls.length;
    await h.emit('session_start', { reason: 'reload' });
    h.setMessages([user('Do not resume')]); await h.emit('context'); await tick();
    assert.equal(h.calls.length, count);
    if (!attached) assert.deepEqual(await h.store.listAgents(), []);
  });
});

test('detach supersedes older queued open/new intents across cleanup failure and reload', async t => {
  for (const intent of ['open', 'new Superseded title']) for (const cleanupFails of [false, true]) for (const realReload of [false, true]) {
    await t.test(`${intent}, cleanupFails=${cleanupFails}, realReload=${realReload}`, async t => {
      const h = await host(t, { check_every_n_responses: 0 }, { skipStart: true });
      let manager, handler;
      if (realReload) {
        manager = SessionManager.create(h.cwd, join(h.root, 'sessions'));
        manager.appendMessage({ ...user('Persist this session'), timestamp: Date.now() });
        h.ctx.sessionManager = manager;
      }
      const install = () => h.install({
        on: (name, fn) => h.handlers.set(name, fn),
        registerCommand: (_name, def) => { handler = def.handler; },
        registerTool: def => h.tools.set(def.name, def),
        appendEntry: (customType, data) => manager ? manager.appendCustomEntry(customType, data) : h.setBranch([...h.branch(), { type: 'custom', customType, data: structuredClone(data) }]),
      });
      const command = args => handler(args, h.ctx);
      const saved = () => h.ctx.sessionManager.getEntries().filter(e => e.type === 'custom' && e.customType === 'work-cards.workspace').at(-1);
      install(); await h.emit('session_start');
      if (manager) manager.appendMessage({ ...user('Fresh task'), timestamp: Date.now() });
      else h.setMessages([user('Fresh task')]);
      await h.emit('context'); await command('show');
      const originalAgent = (await h.current()).details.agent.record;
      const launches = h.launches().length, calls = h.calls.length;
      const entered = deferred(), release = deferred(), original = RecordStore.prototype.readAgent;
      let once = true;
      RecordStore.prototype.readAgent = async function(id) {
        const result = await original.call(this, id);
        if (this.directory === h.directory && once) { once = false; entered.resolve(); await release.promise; }
        return result;
      };
      t.after(() => { RecordStore.prototype.readAgent = original; release.resolve(); });
      h.setHook(async args => {
        if (args[0] === 'plugin' && args[2] === 'close') {
          assert.equal(saved().data.updates, false);
          if (cleanupFails) throw new Error('queued cleanup failed');
        }
      });
      const show = command('show'); await entered.promise;
      const queued = command(intent), detach = command('detach');
      assert.equal(saved().data.updates, false);
      release.resolve(); await Promise.all([show, queued, detach]);
      RecordStore.prototype.readAgent = original;
      assert.equal(saved().data.updates, false);
      assert.equal((await h.current()).details.updates, false);
      assert.equal((await h.current()).details.agent.record.title, originalAgent.title);
      assert.equal(h.launches().length, launches); assert.equal(h.calls.length, calls);
      assert.equal(h.closes().length, 1);
      assert.equal(h.panes.filter(p => p.tokens.wca_role === 'card').length, cleanupFails ? 1 : 0);
      assert.match(h.notices.at(-1).text, cleanupFails ? /queued cleanup failed/ : /updates paused/);
      await h.emit('session_shutdown', { reason: 'reload' });
      if (manager) { manager = SessionManager.open(manager.getSessionFile()); h.ctx.sessionManager = manager; }
      install(); await h.emit('session_start', { reason: 'reload' });
      await h.emit('context'); await command('show');
      assert.equal(saved().data.updates, false); assert.equal((await h.current()).details.updates, false);
      assert.equal(h.launches().length, launches); assert.equal(h.calls.length, calls);
      h.setHook(async () => {}); await command('open');
      assert.equal(saved().data.updates, true); assert.equal((await h.current()).details.updates, true);
      await until(() => h.calls.length > calls);
    });
  }
});

test('a stale detach continuation cannot persist the outgoing workspace under a replacement UUID', async t => {
  const h = await attach(t); const wait = deferred(), entered = deferred();
  const original = RecordStore.prototype.readAgent; let once = true;
  RecordStore.prototype.readAgent = async function(id) { const result = await original.call(this, id); if (once) { once = false; entered.resolve(); await wait.promise; } return result; };
  t.after(() => { RecordStore.prototype.readAgent = original; });
  const detach = h.command('detach');
  assert.equal(h.branch().at(-1).data.updates, false);
  await entered.promise;
  h.setSession('replacement'); const start = h.emit('session_start', { reason: 'new' }); wait.resolve(); await Promise.all([detach, start]);
  assert.equal(h.branch().some(e => e.customType === 'work-cards.workspace' && e.data.sessionId === 'replacement'), false);
  RecordStore.prototype.readAgent = original;
});

test('committed departure retires old display while reload/quit and same-session resume retain it', async t => {
  for (const reason of ['new', 'resume', 'fork', 'reload', 'quit', 'same-resume']) await t.test(reason, async t => {
    const h = await attach(t); await h.emit('session_before_switch'); assert.equal(h.closes().length, 0);
    const distinct = join(h.root, 'distinct.jsonl');
    await writeFile(distinct, JSON.stringify({ type: 'session', id: 'distinct', cwd: h.cwd, timestamp: new Date().toISOString() }) + '\n');
    await h.emit('session_shutdown', { reason: reason === 'same-resume' ? 'resume' : reason, ...(reason === 'same-resume' ? { targetSessionFile: h.ctx.sessionManager.getSessionFile() } : reason === 'resume' ? { targetSessionFile: distinct } : {}) });
    assert.equal(h.closes().length, ['new', 'resume', 'fork'].includes(reason) ? 1 : 0);
    const count = h.native.length; await h.emit('context'); await h.command('open'); assert.equal(h.native.length, count);
  });
});

test('shutdown cleanup failure reports explicit recovery and cannot revive the disposed extension', async t => {
  const h = await attach(t); h.setHook(async args => { if (args[0] === 'plugin' && args[2] === 'close') throw new Error('close unavailable'); });
  await h.emit('session_shutdown', { reason: 'fork' }); assert.match(h.notices.at(-1).text, /recovery|inspect/i);
  h.setHook(async () => {}); const count = h.native.length; await h.emit('context'); await h.command('open'); assert.equal(h.native.length, count);
});

test('outside Herdr reads and explicitly refreshes durable own records without native fallback or creation', async t => {
  const h = await attach(t); await h.emit('session_shutdown', { reason: 'quit' }); delete h.env.HERDR_ENV;
  const handlers = new Map(); h.install({ on: (name, fn) => handlers.set(name, fn), registerCommand: (_name, def) => { h.offlineCommand = def.handler; }, registerTool: def => { h.offlineTool = def; }, appendEntry: () => {} });
  await handlers.get('session_start')({}, h.ctx); const count = h.native.length, before = h.calls.length;
  assert.ok((await h.offlineTool.execute('read', {}, undefined, undefined, h.ctx)).details.agent);
  await h.offlineCommand('refresh', h.ctx); await until(() => h.calls.length > before); assert.equal(h.native.length, count);
  await h.offlineCommand('open', h.ctx); assert.match(h.notices.at(-1).text, /Herdr/); assert.equal(h.native.length, count);
  await handlers.get('session_shutdown')({ reason: 'quit' }, h.ctx);
});

test('native failure never becomes offline helper authority and config failures fail closed with deduplicated errors', async t => {
  const h = await attach(t); await tick(); const before = h.calls.length;
  h.setHook(async args => { if (args[0] === 'api') throw new Error('native unavailable'); });
  await h.command('refresh'); await h.command('refresh'); assert.equal(h.calls.length, before);
  assert.equal(h.notices.filter(n => n.text.includes('native unavailable')).length, 1);
  h.setConfigError('invalid policy'); await h.command('refresh'); await h.emit('context'); assert.equal(h.calls.length, before);
  assert.match(h.notices.at(-1).text, /invalid policy/);
  h.setHook(async () => {}); h.setConfigError(undefined); h.q();
  await h.command('open'); assert.equal(h.launches().length, 2);
  await until(() => h.calls.length > before);
});

test('literal remove is unique, checked, metadata-only and broken references remain removable', async t => {
  const h = await attach(t), a = await h.read(), authority = agentAuthority(a.record);
  await h.store.addReferences(authority, { documents: [{ checkout: a.record.checkout, path: 'missing-plan.md', description: 'description-only' }], issues: [{ url: 'https://example.com/issue/plan', description: '' }], prs: [{ url: 'https://example.com/pr/other', description: '' }] });
  for (const arg of ['', 'plan', 'description-only', 'PLAN']) { const w = await h.store.readWorkspace(a.record.workspaceId); await h.command(`remove ${arg}`); assert.equal((await h.store.readWorkspace(a.record.workspaceId)).revision, w.revision); }
  await h.command('remove missing-plan.md'); const w = await h.store.readWorkspace(a.record.workspaceId); assert.equal(w.record.documents.length, 0); assert.equal(w.record.exclusions.length, 1);
  assert.equal(w.record.issues.length, 1); assert.equal(w.record.prs.length, 1);
});

test('explicit workspace selection refuses native replacement and duplicate live binding', async t => {
  const h = await host(t); await h.store.createWorkspace({ id: 'selected', creationToken: 'token', title: 'Selected' });
  await h.command('open selected'); assert.equal((await h.read()).record.workspaceId, 'selected');
  await h.store.createWorkspace({ id: 'other', creationToken: 'other-token', title: 'Other' }); await h.command('open other');
  assert.equal((await h.read()).record.workspaceId, 'selected'); assert.match(h.notices.at(-1).text, /bound|replace/i);
  h.move(); await h.command('open selected'); assert.match(h.notices.at(-1).text, /already.*live|duplicate/i);
});

test('real SDK failed initial configuration requires reload before open/new and retains detach pause', async t => {
  for (const intent of ['open', 'new Recovered task']) await t.test(intent, async t => {
    const h = await host(t, { check_every_n_responses: 0 }, { skipStart: true });
    const { createEventBus } = await import(new URL('./core/event-bus.js', import.meta.resolve('@earendil-works/pi-coding-agent')));
    let runtime, extension;
    const resources = {
      getExtensions: () => ({ extensions: [extension], errors: [], runtime }),
      getSystemPrompt: () => 'Test only', getAppendSystemPrompt: () => [],
      getSkills: () => ({ skills: [] }), getAgentsFiles: () => ({ agentsFiles: [] }), getPrompts: () => ({ prompts: [] }),
      reload: async () => {
        runtime = createExtensionRuntime();
        extension = await loadExtensionFromFactory(pi => h.install(pi), h.cwd, createEventBus(), runtime);
      },
    };
    await resources.reload();
    const manager = SessionManager.inMemory(h.cwd), errors = [];
    const session = new AgentSession({ agent: new Agent(), sessionManager: manager, cwd: h.cwd,
      settingsManager: SettingsManager.inMemory({ compaction: { enabled: false } }), resourceLoader: resources,
      baseToolsOverride: {}, initialActiveToolNames: [],
      modelRuntime: { getModel: (...args) => h.ctx.modelRegistry.find(...args), streamSimple: (...args) => h.ctx.modelRegistry.streamSimple(...args) } });
    t.after(async () => { await session.extensionRunner.emit({ type: 'session_shutdown', reason: 'quit' }); session.dispose(); });
    h.setConfigError('invalid initial configuration');
    await session.bindExtensions({ mode: 'tui', uiContext: h.ctx.ui, onError: error => errors.push(error) });
    assert.match(h.notices.at(-1).text, /invalid initial configuration/);
    h.setConfigError(undefined);
    await session.prompt(`/work-card ${intent}`);
    assert.match(h.notices.at(-1).text, /startup.*(?:did not|didn't) complete.*configuration.*\/reload.*retry/i);
    assert.doesNotMatch(h.notices.at(-1).text, /undefined|TypeError/);
    const saved = () => manager.getEntries().filter(e => e.type === 'custom' && e.customType === 'work-cards.workspace').at(-1);
    const detaching = session.prompt('/work-card detach');
    assert.equal(saved().data.updates, false);
    await detaching;
    assert.match(h.notices.at(-1).text, /\/reload/);
    manager.appendMessage({ ...user('Fresh task while paused'), timestamp: Date.now() });
    await session.extensionRunner.emit({ type: 'context', messages: [] });
    await session.prompt('/work-card show');
    assert.equal(h.notices.at(-1).text, 'null');
    const read = await extension.tools.get('work_card').definition.execute('read', {}, undefined, undefined, session.extensionRunner.createContext());
    assert.equal(read.details, null);
    assert.equal(h.native.length, 0); assert.equal(h.calls.length, 0);
    assert.deepEqual(await h.store.listAgents(), []); assert.deepEqual(await h.store.listWorkspaces(), []);
    await session.reload();
    manager.appendMessage({ ...user('Fresh task after reload, still paused'), timestamp: Date.now() });
    await session.extensionRunner.emit({ type: 'context', messages: [] });
    await session.prompt('/work-card show');
    assert.equal(saved().data.updates, false);
    assert.equal(h.launches().length, 0); assert.equal(h.calls.length, 0);
    assert.deepEqual(await h.store.listAgents(), []);
    await h.store.createWorkspace({ id: 'recovered', creationToken: 'recovered-token', title: 'Recovered workspace' });
    await session.prompt('/work-card open recovered');
    assert.equal(saved().data.updates, true, JSON.stringify(h.notices));
    assert.equal(h.launches().length, 1);
    assert.equal((await h.store.readAgent(manager.getSessionId())).record.sessionId, manager.getSessionId());
    assert.deepEqual(errors, []);
  });
});

test('real Pi loader and AgentSessionRuntime cancel before-events, retire before disposal and replace factories', async t => {
  const h = await host(t, { check_every_n_responses: 0 }, { skipStart: true });
  const { createEventBus } = await import(new URL('./core/event-bus.js', import.meta.resolve('@earendil-works/pi-coding-agent')));
  const { AgentSessionRuntime } = await import(new URL('./core/agent-session-runtime.js', import.meta.resolve('@earendil-works/pi-coding-agent')));
  const errors = []; let cancelSwitch = false; const instances = [];
  const create = async ({ sessionManager, sessionStartEvent }) => {
    const runtime = createExtensionRuntime();
    const extension = await loadExtensionFromFactory(pi => { h.install(pi); pi.on('session_before_switch', () => cancelSwitch ? { cancel: true } : undefined); }, h.cwd, createEventBus(), runtime);
    const resources = { getExtensions: () => ({ extensions: [extension], errors: [], runtime }), getSystemPrompt: () => 'Test only', getAppendSystemPrompt: () => [], getSkills: () => ({ skills: [] }), getAgentsFiles: () => ({ agentsFiles: [] }), getPrompts: () => ({ prompts: [] }) };
    const agent = new Agent();
    const session = new AgentSession({ agent, sessionManager, cwd: h.cwd, settingsManager: SettingsManager.inMemory({ compaction: { enabled: false } }),
      resourceLoader: resources, baseToolsOverride: {}, initialActiveToolNames: [], sessionStartEvent,
      modelRuntime: { getModel: (...args) => h.ctx.modelRegistry.find(...args), streamSimple: (...args) => h.ctx.modelRegistry.streamSimple(...args) } });
    const instance = { session, extension, disposed: false }; const dispose = session.dispose.bind(session);
    session.dispose = () => { instance.disposed = true; dispose(); };
    await session.bindExtensions({ mode: 'tui', uiContext: h.ctx.ui, onError: error => errors.push(error) });
    instances.push(instance);
    return { session, services: { cwd: h.cwd, agentDir: join(h.root, 'agent') } };
  };
  const initial = await create({ sessionManager: SessionManager.inMemory(h.cwd), sessionStartEvent: { type: 'session_start', reason: 'startup' } });
  const runtime = new AgentSessionRuntime(initial.session, initial.services, create);
  t.after(() => runtime.dispose());
  const runner = runtime.session.extensionRunner, context = runner.createCommandContext();
  await runner.getCommand('work-card').handler('new SDK assignment', context);
  assert.equal(h.launches().length, 1); const oldId = runtime.session.sessionManager.getSessionId();
  assert.equal((await h.store.readAgent(oldId)).record.sessionId, oldId);
  cancelSwitch = true; assert.deepEqual(await runtime.newSession(), { cancelled: true });
  assert.equal(h.closes().length, 0); assert.equal(instances[0].disposed, false);
  h.setHook(async args => { if (args[0] === 'plugin' && args[2] === 'close') assert.equal(instances[0].disposed, false); });
  cancelSwitch = false; assert.deepEqual(await runtime.newSession(), { cancelled: false });
  assert.equal(h.closes().length, 1); assert.equal(instances[0].disposed, true); assert.equal(instances.length, 2);
  assert.notEqual(runtime.session.sessionManager.getSessionId(), oldId);
  const tool = instances[1].extension.tools.get('work_card').definition;
  const read = await tool.execute('read', {}, undefined, undefined, runtime.session.extensionRunner.createContext());
  assert.equal(read.details, null); assert.deepEqual(errors, []);
});

test('real SDK copied and symlinked session locators retain same-UUID display without inspection writes', async t => {
  const h = await host(t, {}, { skipStart: true });
  const { createEventBus } = await import(new URL('./core/event-bus.js', import.meta.resolve('@earendil-works/pi-coding-agent')));
  const { AgentSessionRuntime } = await import(new URL('./core/agent-session-runtime.js', import.meta.resolve('@earendil-works/pi-coding-agent')));
  const errors = [];
  const create = async ({ sessionManager, sessionStartEvent }) => {
    const runtime = createExtensionRuntime();
    const extension = await loadExtensionFromFactory(pi => h.install(pi), h.cwd, createEventBus(), runtime);
    const resources = { getExtensions: () => ({ extensions: [extension], errors: [], runtime }), getSystemPrompt: () => 'Test only', getAppendSystemPrompt: () => [], getSkills: () => ({ skills: [] }), getAgentsFiles: () => ({ agentsFiles: [] }), getPrompts: () => ({ prompts: [] }) };
    const session = new AgentSession({ agent: new Agent(), sessionManager, cwd: h.cwd, settingsManager: SettingsManager.inMemory({ compaction: { enabled: false } }), resourceLoader: resources, baseToolsOverride: {}, initialActiveToolNames: [], sessionStartEvent,
      modelRuntime: { getModel: (...args) => h.ctx.modelRegistry.find(...args), streamSimple: (...args) => h.ctx.modelRegistry.streamSimple(...args) } });
    await session.bindExtensions({ mode: 'tui', uiContext: h.ctx.ui, onError: error => errors.push(error) });
    return { session, services: { cwd: h.cwd, agentDir: join(h.root, 'agent') } };
  };
  const manager = SessionManager.create(h.cwd, join(h.root, 'sessions'));
  manager.appendMessage({ ...user('Real persisted dialogue'), timestamp: Date.now() });
  const initial = await create({ sessionManager: manager, sessionStartEvent: { type: 'session_start', reason: 'startup' } });
  const runtime = new AgentSessionRuntime(initial.session, initial.services, create);
  t.after(() => runtime.dispose());
  const runner = runtime.session.extensionRunner;
  await runner.getCommand('work-card').handler('new SDK task', runner.createCommandContext());
  const id = manager.getSessionId(), copy = join(h.root, 'copy.jsonl'), alias = join(h.root, 'alias.jsonl');
  await copyFile(manager.getSessionFile(), copy); await symlink(copy, alias);
  for (const path of [copy, alias]) {
    const before = await readFile(path, 'utf8');
    assert.deepEqual(await runtime.switchSession(path), { cancelled: false });
    assert.equal(runtime.session.sessionManager.getSessionId(), id);
    assert.equal(h.closes().length, 0); assert.equal(h.launches().length, 1);
    assert.equal(await readFile(path, 'utf8'), before);
  }
  const distinct = SessionManager.create(h.cwd, join(h.root, 'other-sessions'));
  distinct.appendMessage({ ...user('Other UUID'), timestamp: Date.now() });
  await runtime.switchSession(distinct.getSessionFile());
  assert.equal(h.closes().length, 1); assert.deepEqual(errors, []);
});

test('unreadable or malformed resume identity does not guess departure from locator', async t => {
  for (const malformed of [false, true]) await t.test(String(malformed), async t => {
    const h = await attach(t), path = join(h.root, 'unknown.jsonl');
    if (malformed) await writeFile(path, 'not a header\n');
    await h.emit('session_shutdown', { reason: 'resume', targetSessionFile: path });
    assert.equal(h.closes().length, 0);
    if (malformed) assert.equal(await readFile(path, 'utf8'), 'not a header\n');
  });
});

test('refresh diagnostics distinguish no purpose sources and read-only inspection makes no requests', async t => {
  const h = await attach(t, { check_every_n_responses: 0, pane_poll_interval_ms: 60_000 });
  await until(async () => (await h.current()).details.refresh?.state === 'complete');
  const report = (await h.current()).details.refresh;
  assert.equal(report.forced, true);
  assert.deepEqual(report.stages.map(({ stage, outcome }) => ({ stage, outcome })), [
    { stage: 'own', outcome: 'unchanged' },
    { stage: 'selection', outcome: 'no_sources' }, { stage: 'goal', outcome: 'no_sources' },
  ]);
  await until(() => h.notices.some(n => /Goal unknown.*purpose documents/.test(n.text)));
  const requests = h.calls.length, native = h.native.length, revision = (await h.read()).revision;
  await h.current(); await h.command('show');
  assert.equal(h.calls.length, requests); assert.equal(h.native.length, native);
  assert.equal((await h.read()).revision, revision);
});

test('refresh diagnostics distinguish pre-goal failure and do not claim source selection ran', async t => {
  const h = await attach(t, { check_every_n_responses: 0, pane_poll_interval_ms: 60_000 });
  await until(async () => (await h.current()).details.refresh?.state === 'complete');
  h.setResult(async () => { throw new Error('provider unavailable'); });
  await h.command('refresh');
  await until(async () => (await h.current()).details.refresh?.state === 'failed');
  const report = (await h.current()).details.refresh;
  assert.deepEqual(report.stages, [
    { stage: 'own', outcome: 'failed', cancelled: false }, { stage: 'goal', outcome: 'skipped' },
  ]);
  await until(() => h.notices.some(n => /own stage.*provider unavailable/.test(n.text)));
  assert.equal((await h.current()).details.workspace.record.goal, '');
});

test('Pi publishes separate purpose and own progress after q, including invalidation without helper calls', async t => {
  const h = await attach(t, { check_every_n_responses: 0, pane_poll_interval_ms: 15 });
  await until(async () => (await h.current()).details.refresh?.state === 'complete');
  const a = await h.read(), authority = agentAuthority(a.record), source = { checkout: a.record.checkout, path: 'purpose.md' };
  await writeFile(join(h.cwd, source.path), '# Go port\nPreserve tracing behavior across Rust and Go.');
  await h.store.addReferences(authority, { documents: [{ ...source, description: 'Go port design and purpose' }] });
  h.setMessages([user('Use purpose.md for the shared Go port purpose.')]);
  h.setResult(async kind => kind === 'own' ? { action: 'update', patch: { now: 'Waiting for Go port review.', status: 'waiting', statusShort: 'Review pending' } }
    : kind === 'shared' ? { action: 'no_change' }
    : kind === 'select' ? { sources: [source] }
    : { goal: 'Preserve tracing behavior across Rust and Go.', goalShort: 'Port tracing to Go', title: 'Tracing Go port' });
  await h.command('refresh');
  await until(() => h.workspaces[0].tokens.wcw_goal_short === 'Port tracing to Go' && h.panes[0].tokens.wca_status_short === 'Review pending');
  assert.equal(h.workspaces[0].tokens.wcw_goal, 'Preserve tracing behavior across Rust and Go.');
  assert.equal(h.panes[0].tokens.wca_status, 'waiting');
  assert.equal(h.workspaces[0].tokens.wca_status_short, undefined);
  h.q(); const launches = h.launches().length, requests = h.calls.length;
  let current = await h.read();
  await h.store.updateOwn(agentAuthority(current.record), { status: 'blocked', now: 'Missing review access.', statusShort: 'Review access blocked' }, current.revision);
  await until(() => h.panes[0].tokens.wca_status_short === 'Review access blocked');
  current = await h.read();
  await h.store.updateOwn(agentAuthority(current.record), { now: 'Access restored.' }, current.revision);
  const workspace = await h.store.readWorkspace(current.record.workspaceId);
  await h.store.removeReference(agentAuthority(current.record), { workspaceId: workspace.record.id, revision: workspace.revision, identity: referenceIdentity(source) });
  await until(() => !h.panes[0].tokens.wca_status_short && !h.workspaces[0].tokens.wcw_goal_short);
  assert.equal(h.calls.length, requests); assert.equal(h.launches().length, launches);
  assert.equal(h.native.some(args => args.includes('focus')), false);
});

test('post-refresh publication and reconciliation share the host queue, including after q', async t => {
  for (const closed of [false, true]) await t.test(closed ? 'card closed' : 'card open', async t => {
    const h = await attach(t, { check_every_n_responses: 0, pane_poll_interval_ms: 60_000 });
    const completions = () => h.notices.filter(n => n.text.startsWith('Work-card refresh:')).length;
    await until(() => completions() === 1);
    if (closed) h.q();
    const before = await h.read(), launched = h.launches().length;
    const entered = deferred(), release = deferred(); let held = false;
    h.setHook(async args => {
      if (!held && args.includes('wca_status_short=Review ready')) {
        held = true; entered.resolve(); await release.promise;
      }
    });
    h.setResult(async kind => kind === 'own' ? { action: 'update', patch: { now: 'Review package prepared.', statusShort: 'Review ready' } } : { action: 'no_change' });
    await h.command('refresh'); await entered.promise;
    const nativeAtGate = h.native.length; let nativeWhileHeld;
    try {
      await h.emit('context');
      await new Promise(resolve => setTimeout(resolve, 100));
      nativeWhileHeld = h.native.length;
    } finally { release.resolve(); }
    await h.command('show');
    assert.equal(nativeWhileHeld, nativeAtGate, 'Reconciliation must not compete for the projection lock');
    await until(() => completions() === 2);
    const after = await h.read(), report = (await h.current()).details.refresh;
    assert.equal(after.record.membershipToken, before.record.membershipToken);
    assert.equal(after.record.evidenceGeneration, before.record.evidenceGeneration);
    assert.equal(report.state, 'complete');
    assert.equal(h.panes[0].tokens.wca_status_short, 'Review ready');
    assert.equal(h.launches().length, launched);
    assert.equal(h.notices.some(n => n.level === 'warning'), false);
  });
});

test('sidebar publication failure does not undo successful record refresh or hide its diagnostics', async t => {
  const h = await attach(t, { check_every_n_responses: 0, pane_poll_interval_ms: 60_000 });
  await until(async () => (await h.current()).details.refresh?.state === 'complete');
  h.setHook(async args => { if (args.includes('wca_status_short=Tests passed')) throw new Error('native metadata unavailable'); });
  h.setResult(async kind => kind === 'own' ? { action: 'update', patch: { now: 'Local tests passed.', statusShort: 'Tests passed' } } : { action: 'no_change' });
  await h.command('refresh');
  await until(() => h.notices.some(n => /Sidebar projection.*native metadata unavailable/.test(n.text)));
  assert.equal((await h.read()).record.statusShort, 'Tests passed');
  assert.equal((await h.current()).details.refresh.state, 'complete');
  assert.ok((await h.current()).details.refresh.stages.some(s => s.stage === 'own' && s.outcome === 'applied'));
});

test('actual WorkspaceClient launch environment is accepted and rendered by runPane', async t => {
  const h = await attach(t); const args = h.launches()[0]; assert.ok(args);
  const env = {}; for (let i = 0; i < args.length; i++) if (args[i] === '--env') { const [key, ...v] = args[++i].split('='); env[key] = v.join('='); }
  assert.equal(env.WORK_CARDS_CARD, undefined); assert.equal(env.WORK_CARDS_AGENT, (await h.read()).path);
  const moduleURL = name => JSON.stringify(new URL(`../dist/${name}.js`, import.meta.url).href);
  const script = `
    import { PassThrough, Writable } from 'node:stream';
    import { runPane } from ${moduleURL('herdr/pane')};
    import { WorkspaceClient } from ${moduleURL('herdr/workspace-client')};
    const original = process.stdout, input = new PassThrough(); input.isTTY = true; input.isRaw = false; input.setRawMode = value => { input.isRaw = value; };
    let frames = ''; const output = new Writable({write(chunk, _encoding, done) { frames += chunk.toString(); if (frames.includes('Agent task')) setImmediate(() => input.emit('data', Buffer.from('q'))); done(); }}); output.isTTY = true; output.columns = 100; output.rows = 14;
    Object.defineProperty(process, 'stdin', {value: input}); Object.defineProperty(process, 'stdout', {value: output});
    WorkspaceClient.prototype.bootstrap = async () => { throw new Error('No real native service in contract test'); };
    await runPane(${JSON.stringify(env)}); original.write(JSON.stringify({frames, raw: input.isRaw}));
  `;
  const child = spawnSync(process.execPath, ['--input-type=module', '-e', script], { encoding: 'utf8', timeout: 5000 });
  assert.equal(child.status, 0, child.stderr || String(child.error)); const output = JSON.parse(child.stdout); assert.match(output.frames, /Agent task/); assert.equal(output.raw, false);
});
