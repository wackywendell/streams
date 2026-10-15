import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtemp, readdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { evaluatedGoal, RecordStore } from '../dist/records.js';
import { agentAuthority } from '../dist/records.js';

const moduleURL = name => JSON.stringify(new URL(`../dist/${name}.js`, import.meta.url).href);
async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'workspace-pane-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const store = new RecordStore(root);
  await store.createWorkspace({ id: 'shared', creationToken: 'creation', title: 'Shared' });
  for (const sessionId of ['one', 'two']) {
    let agent = await store.join({ workspaceId: 'shared', sessionId, cwd: root, title: `Agent ${sessionId}`, membershipToken: `member-${sessionId}`,
      boundary: { compactionDigest: 'compact', transcriptLength: 0, prefixDigest: 'prefix' } });
    agent = await store.updateOwn(agentAuthority(agent.record), { now: `Own work ${sessionId}`, status: sessionId === 'one' ? 'active' : 'blocked' }, agent.revision);
    await store.setDisplayAttempt(agentAuthority(agent.record), { attempt: 'attempt', nonce: 'nonce' }, agent.revision);
  }
  const agent = await store.readAgent('one');
  await store.addReferences(agentAuthority(agent.record), { documents: [{ checkout: agent.record.checkout, path: 'notes.md', description: '' }] });
  const workspace = await store.readWorkspace('shared');
  await store.applyPlan(agentAuthority(agent.record), { goal: evaluatedGoal('Shared goal', [{ checkout: agent.record.checkout, path: 'notes.md' }], 'plan-fingerprint') }, workspace.revision);
  return { root, store };
}
function launch(root, sessionId = 'one', options = {}) {
  const store = new RecordStore(root);
  const env = { WORK_CARDS_AGENT: store.agentPath(sessionId), WORK_CARDS_SESSION_ID: sessionId, WORK_CARDS_STATE_DIR: root,
    WORK_CARDS_OWNER_TERMINAL: 'owner', WORK_CARDS_NONCE: 'nonce', WORK_CARDS_CONFIG: join(root, 'missing.json'), ...options.env };
  const script = `
    import { PassThrough, Writable } from 'node:stream';
    import { runPane } from ${moduleURL('herdr/pane')};
    import { RecordStore } from ${moduleURL('records')};
    import { WorkspaceClient } from ${moduleURL('herdr/workspace-client')};
    import { WorkspaceCardView } from ${moduleURL('herdr/workspace-view')};
    const options = ${JSON.stringify(options)};
    const originalOutput = process.stdout, input = new PassThrough();
    input.isTTY = true; input.isRaw = false;
    const result = { raw: [], frames: [], notices: [], projects: [], opens: [], effects: [], observations: [], aborted: [], forbidden: [] };
    input.setRawMode = value => { result.raw.push(value); input.isRaw = value; };
    let painted = '', iteration = 0, signal;
    const output = new Writable({ write(chunk, encoding, done) { painted += chunk.toString(); done(); } });
    output.isTTY = true; output.columns = 100; output.rows = 14;
    Object.defineProperty(process, 'stdin', { value: input });
    Object.defineProperty(process, 'stdout', { value: output });
    for (const name of ['createWorkspace', 'join', 'updateOwn', 'setDisplayAttempt', 'deleteOwn', 'deleteWorkspace', 'addReferences']) {
      RecordStore.prototype[name] = async () => { result.forbidden.push(name); throw new Error('Forbidden store mutation'); };
    }
    for (const name of ['joinWorkspace', 'openDisplay', 'closeDisplay', 'cleanupSeparatedPair', 'refreshDocumentNames', 'projectSummaries']) {
      WorkspaceClient.prototype[name] = async () => { result.forbidden.push(name); throw new Error('Forbidden native operation'); };
    }
    const initial = { socket: '/fake.sock', terminalId: 'card', workspaceId: 'source', tabId: 'tab-source', bindingNonce: 'binding-source' };
    WorkspaceClient.prototype.bootstrap = async () => {
      if (options.stopAt === 'bootstrap') process.emit('SIGTERM');
      return initial;
    };
    WorkspaceClient.prototype.observe = async context => {
      iteration++;
      result.observations.push(context);
      if (options.stopAt === 'observe') process.emit('SIGTERM');
      if (iteration === options.failObserve) throw new Error('Inventory unavailable');
      const destination = options.moves && iteration >= 2;
      const next = { ...context, workspaceId: destination ? 'destination' : 'source', tabId: destination ? 'tab-destination' : 'tab-source', bindingNonce: destination ? 'binding-destination' : 'binding-source' };
      return { context: next, snapshot: { workspaces: [], tabs: [], panes: [], layouts: [] }, caller: { terminal_id: 'card', workspace_id: next.workspaceId, tab_id: next.tabId, pane_id: 'pane-card', tokens: {} } };
    };
    WorkspaceClient.prototype.verifyDisplay = async (store, context, agent, config, passedSignal) => {
      signal = passedSignal;
      if (options.stale) throw new Error('Stale or foreign ready workspace binding');
      return { kind: options.separated ? 'separated' : 'paired', agent: { terminal_id: options.wrongOwner ? 'foreign' : 'owner' }, card: { terminal_id: 'card' }, nonce: 'nonce', identity: 'identity' };
    };
    WorkspaceClient.prototype.projectCaption = async (store, context, agent, config, passedSignal) => {
      signal = passedSignal;
      result.projects.push({ iteration, context, sessionId: agent.record.sessionId });
      if (options.contention) throw new Error('Work-cards lock exists');
      if (options.stopAt === 'project') {
        await new Promise(resolve => { passedSignal.addEventListener('abort', resolve, { once: true }); setImmediate(() => process.emit('SIGTERM')); });
        result.aborted.push(passedSignal.aborted);
      }
    };
    WorkspaceClient.prototype.openDocument = async (store, context, workspace, agent, identity, config, passedSignal) => {
      signal = passedSignal;
      result.opens.push({ context, workspaceId: workspace.record.id, sessionId: agent.record.sessionId, identity });
      // Deliberately permissive effect sink: unsafe contexts cannot be rescued by adapter validation.
      result.effects.push('launch', 'stamp', 'focus');
      input.emit('data', Buffer.from('aaaaaq'));
      if (options.stopAt === 'document') {
        await new Promise(resolve => { passedSignal.addEventListener('abort', resolve, { once: true }); setImmediate(() => process.emit('SIGTERM')); });
        result.aborted.push(passedSignal.aborted);
      }
      if (options.documentError) throw new Error('Manual editor unavailable (known tab t9)');
    };
    const render = WorkspaceCardView.prototype.render;
    WorkspaceCardView.prototype.render = function(columns, rows) {
      result.notices.push(this.notice ?? null);
      const text = render.call(this, columns, rows); result.frames.push(text);
      setImmediate(() => {
        if (iteration === 1 && options.document) input.emit('data', Buffer.from(options.failObserve ? 'a' : 'aaaaaq'));
        else if (iteration >= (options.rounds ?? 1)) options.quit ? input.emit('data', Buffer.from('q')) : process.emit('SIGTERM');
        else output.emit('resize');
      });
      return text;
    };
    try { await runPane(${JSON.stringify(env)}); } catch (error) { result.error = error.message; }
    result.painted = painted; result.signalAborted = signal?.aborted;
    originalOutput.write(JSON.stringify(result));
  `;
  const child = spawnSync(process.execPath, ['--input-type=module', '-e', script], { encoding: 'utf8', timeout: 5000 });
  assert.equal(child.status, 0, child.stderr || String(child.error));
  return JSON.parse(child.stdout);
}
function restored(result) {
  assert.deepEqual(result.raw, [true, false]);
  assert.ok(result.painted.endsWith('\x1b[?2004l\x1b[?7h\x1b[?25h\x1b[?1049l'));
  assert.deepEqual(result.forbidden, []);
}

test('explicit full workspace environment renders shared Goal and only each own agent; q is display-only', async t => {
  const { root, store } = await fixture(t);
  const before = await Promise.all([store.readAgent('one'), store.readAgent('two'), store.readWorkspace('shared')]);
  for (const sessionId of ['one', 'two']) {
    const result = launch(root, sessionId, { quit: true });
    assert.equal(result.error, undefined);
    assert.match(result.frames[0], /Shared goal/); assert.match(result.frames[0], new RegExp(`Own work ${sessionId}`));
    assert.doesNotMatch(result.frames[0], new RegExp(`Own work ${sessionId === 'one' ? 'two' : 'one'}`));
    assert.match(result.frames[0], sessionId === 'one' ? /active/ : /blocked/);
    assert.ok(result.projects.every(project => project.sessionId === sessionId));
    assert.equal(result.signalAborted, true); restored(result);
  }
  assert.deepEqual(await Promise.all([store.readAgent('one'), store.readAgent('two'), store.readWorkspace('shared')]), before);
});

test('legacy-only launch input is unsupported before terminal or native effects', async t => {
  const { root } = await fixture(t);
  const result = launch(root, 'one', { env: {
    WORK_CARDS_CARD: join(root, 'legacy.yaml'), WORK_CARDS_AGENT: undefined, WORK_CARDS_SESSION_ID: undefined,
    WORK_CARDS_OWNER_TERMINAL: undefined, WORK_CARDS_NONCE: undefined,
  } });
  assert.match(result.error, /WORK_CARDS_CARD is unsupported/);
  assert.deepEqual(result.raw, []); assert.deepEqual(result.projects, []); assert.deepEqual(result.effects, []);
});

test('rejects mixed, missing, malformed and foreign launch identities before native effects', async t => {
  const { root, store } = await fixture(t);
  const cases = [ { WORK_CARDS_CARD: join(root, 'legacy.yaml') }, { WORK_CARDS_AGENT: '' },
    ...['WORK_CARDS_SESSION_ID', 'WORK_CARDS_STATE_DIR', 'WORK_CARDS_OWNER_TERMINAL', 'WORK_CARDS_NONCE'].map(key => ({ [key]: '' })),
    { WORK_CARDS_AGENT: store.agentPath('two') }, { WORK_CARDS_AGENT: `${root}/agents/../agents/${store.agentPath('one').split('/').at(-1)}` },
    { WORK_CARDS_STATE_DIR: 'relative' }, { WORK_CARDS_STATE_DIR: `${root}/.` },
    { WORK_CARDS_AGENT: '/foreign/agent.yaml' }, { WORK_CARDS_SESSION_ID: 'bad\nidentity' },
    { WORK_CARDS_OWNER_TERMINAL: 'bad\nowner' }, { WORK_CARDS_NONCE: 'bad nonce' } ];
  for (const env of cases) {
    const result = launch(root, 'one', { env });
    assert.ok(result.error, JSON.stringify(env)); assert.deepEqual(result.projects, []); assert.deepEqual(result.observations, []);
  }
});

test('missing records remain unavailable and are never created', async t => {
  const root = await mkdtemp(join(tmpdir(), 'workspace-pane-absent-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const result = launch(root);
  assert.equal(result.error, undefined); assert.match(result.notices[0].text, /Card unavailable/);
  assert.deepEqual(result.projects, []); assert.deepEqual(await readdir(root), []); restored(result);
});

test('each serialized observation follows the card terminal, and failed observation suspends all effects until recovery', async t => {
  const { root } = await fixture(t);
  const result = launch(root, 'one', { moves: true, failObserve: 2, document: true, rounds: 3 });
  assert.equal(result.error, undefined); assert.deepEqual(result.projects.map(p => p.iteration), [1, 3]);
  assert.equal(result.projects.at(-1).context.workspaceId, 'destination');
  assert.equal(result.projects.at(-1).context.bindingNonce, 'binding-destination');
  assert.ok(result.observations.every(context => context.terminalId === 'card'));
  assert.deepEqual(result.effects, []); assert.match(result.notices[1].text, /Inventory unavailable/); restored(result);
  const afterMove = launch(root, 'one', { moves: true, failObserve: 3, rounds: 4 });
  assert.deepEqual(afterMove.projects.map(p => p.iteration), [1, 2, 4]);
  assert.equal(afterMove.observations[2].workspaceId, 'destination');
  assert.match(afterMove.notices[2].text, /Inventory unavailable/); restored(afterMove);
});

for (const options of [{ wrongOwner: true }, { separated: true }, { stale: true }]) {
  test(`unsafe native context stays visible without document effects: ${JSON.stringify(options)}`, async t => {
    const { root } = await fixture(t);
    const result = launch(root, 'one', { ...options, document: true, rounds: 3 });
    assert.equal(result.error, undefined); assert.equal(result.notices[0].source, 'projection');
    assert.deepEqual(result.effects, []);
    assert.deepEqual(result.projects, []);
    restored(result);
  });
}

test('projection lock contention preserves a fresh verified document key', async t => {
  const { root } = await fixture(t);
  const result = launch(root, 'one', { contention: true, document: true, rounds: 4 });
  assert.equal(result.opens.length, 1);
  assert.deepEqual(result.effects, ['launch', 'stamp', 'focus']);
  assert.match(result.notices[0].text, /lock exists/);
  restored(result);
});

for (const documentError of [false, true]) {
  test(`document intent uses exact checkout identity, discards queued q/autorepeat, and preserves document notice (${documentError})`, async t => {
    const { root, store } = await fixture(t);
    const agent = await store.readAgent('one');
    const result = launch(root, 'one', { document: true, documentError, rounds: 4 });
    assert.equal(result.error, undefined); assert.equal(result.opens.length, 1);
    assert.deepEqual(result.opens[0].identity, { checkout: agent.record.checkout, path: 'notes.md' });
    assert.equal(result.opens[0].workspaceId, 'shared'); assert.equal(result.opens[0].sessionId, 'one');
    assert.ok(result.projects.length >= 4);
    if (documentError) { assert.equal(result.notices.at(-1).source, 'document'); assert.match(result.notices.at(-1).text, /Cannot open notes.md.*known tab t9/); }
    else assert.ok(result.notices.every(notice => notice === null));
    restored(result);
  });
}

for (const stopAt of ['bootstrap', 'observe', 'project', 'document']) {
  test(`SIGTERM during ${stopAt} cancels work and restores terminal`, async t => {
    const { root } = await fixture(t);
    const result = launch(root, 'one', { stopAt, document: stopAt === 'document', rounds: 4 });
    assert.equal(result.error, undefined); restored(result);
    if (stopAt === 'bootstrap' || stopAt === 'observe') { assert.deepEqual(result.projects, []); assert.deepEqual(result.opens, []); }
    else { assert.equal(result.signalAborted, true); assert.deepEqual(result.aborted, [true]); }
  });
}
