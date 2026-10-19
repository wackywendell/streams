import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, realpath, rm, readdir, readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { StreamsWorkspaceClient, classifyStreamsPair } from '../dist/herdr/streams-workspace-client.js';
import { StreamsCardSource } from '../dist/herdr/streams-source.js';
import { renderStreamsCard } from '../dist/herdr/streams-view.js';
import { NameStore } from '../dist/herdr/name-provenance.js';
import { StreamsNativeState, streamsDigest } from '../dist/herdr/streams-native-state.js';
import { DEFAULT_CONFIG } from '../dist/config.js';
import { withWorkspaceLock } from '../dist/herdr/workspace-lock.js';
const config = { ...DEFAULT_CONFIG, herdr_timeout_ms: 500 };
const native = n => `12345678-1234-4234-8234-123456789ab${n}`;
async function fixture(t) {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'streams-native-'))); t.after(() => rm(root, { recursive: true, force: true }));
  const socket = join(root, 'herdr.sock'), stateDirectory = join(root, 'state');
  const panes = [1, 2].map(n => ({ pane_id: `w:p${n}`, terminal_id: `term${n}`, workspace_id: 'w', tab_id: `w:t${n}`, tokens: {} }));
  const tabs = [1, 2].map(n => ({ tab_id: `w:t${n}`, workspace_id: 'w', number: n, label: String(n) }));
  const workspaces = [{ workspace_id: 'w', number: 1, label: 'Custom workspace', tokens: {} }];
  const contexts = [];
  for (let n = 1; n <= 2; n++) {
    const checkout = join(root, `checkout-${n}`); await mkdir(join(checkout, 'docs'), { recursive: true }); await writeFile(join(checkout, 'docs/design.md'), `Session ${n} edits`);
    const session = { id: `session-${n}`, host_id: 'host', stream_id: 'shared', harness: 'pi', harness_session_id: native(n), title: `Task ${n}`, title_mode: 'manual', status: 'active', state: `Own progress ${n}`, state_short: `Own ${n}`, remaining: `Todo ${n}` };
    contexts.push({ catalog_source: 'catalog', host_id: 'host', authority: null, session, sessions: [session],
      native: { frontend: 'herdr', pane_id: `w:p${n}`, tab_id: `w:t${n}`, workspace_id: 'w' },
      stream: { id: 'shared', title: 'Shared stream', title_mode: 'manual', goal: 'Shared purpose', goal_short: 'Shared',
        repositories: { main: { url: 'https://example.com/repo' } }, documents: [{ repository: 'main', path: 'docs/design.md', ref: 'recovery', added_at: 'abcdef012345', description: 'Design' }], issues: [], prs: [] },
      checkouts: [{ repository: 'main', checkout_id: `checkout-${n}`, root: checkout }],
      document_locations: [{ repository: 'main', path: 'docs/design.md', checkout_id: `checkout-${n}`, root: checkout, file: join(checkout, 'docs/design.md'), error: null }],
    });
  }
  contexts[0].sessions.push(contexts[1].session); contexts[1].sessions.push(contexts[0].session);
  const calls = []; let next = 3, hook, registration;
  const snapshot = () => ({ workspaces, tabs, panes, layouts: tabs.map(tab => ({ tab_id: tab.tab_id, workspace_id: 'w', panes: panes.filter(p => p.tab_id === tab.tab_id).map(p => ({ pane_id: p.pane_id })) })) });
  const result = (type, fields = {}) => JSON.stringify({ result: { type, ...fields } });
  const plugin = () => ({ plugin_id: 'streams-work-cards', enabled: true, plugin_root: root, manifest_path: join(root, 'streams-herdr-plugin.toml'), source: { kind: 'local' }, version: '0.1.0', min_herdr_version: '0.9.3', platforms: ['macos', 'linux'], panes: [
    { id: 'card', title: 'Streams work card', placement: 'split', command: ['node', 'dist/herdr/streams-pane.js'] },
    { id: 'document', title: 'Document', placement: 'tab', command: ['/bin/sh', 'bin/open-document.sh'] },
  ] });
  const run = terminal => async args => {
    calls.push(args); let output;
    if (args[0] === 'status') output = JSON.stringify({ server: { running: true, compatible: true, endpoint_compatible: true, version: '0.9.5', socket } });
    else if (args[0] === 'api') output = result('session_snapshot', { snapshot: snapshot() });
    else if (args[0] === 'pane' && args[1] === 'current') output = result('pane_current', { pane: panes.find(p => p.terminal_id === terminal) });
    else if (args[1] === 'report-metadata') {
      const target = (args[0] === 'pane' ? panes : workspaces).find(p => (p.pane_id ?? p.workspace_id) === args[2]);
      for (let i = 5; i < args.length; i += 2) { const [key, ...v] = args[i + 1].split('='); if (args[i] === '--clear-token' || !v.join('=')) delete target.tokens[key]; else target.tokens[key] = v.join('='); }
      output = '';
    } else if (args[0] === 'plugin' && args[1] === 'list') output = result('plugin_list', { plugins: registration ? [registration] : [] });
    else if (args[0] === 'plugin' && args[1] === 'link') { registration = plugin(); output = result('plugin_linked', { plugin: registration }); }
    else if (args[0] === 'plugin' && args[1] === 'pane' && args[2] === 'open') {
      const arg = name => args[args.indexOf(name) + 1], entrypoint = arg('--entrypoint'), owner = panes.find(p => p.pane_id === arg('--target-pane')), n = next++;
      const tab_id = entrypoint === 'card' ? owner.tab_id : `w:t${n}`;
      if (entrypoint === 'document') tabs.push({ tab_id, workspace_id: 'w', number: n, label: String(tabs.length + 1) });
      const p = { pane_id: `w:p${n}`, terminal_id: `term${n}`, workspace_id: 'w', tab_id, tokens: {}, label: entrypoint === 'card' ? 'Streams work card' : 'Document' };
      panes.push(p); output = result('plugin_pane_opened', { plugin_pane: { plugin_id: 'streams-work-cards', entrypoint, pane: p } });
    } else if (args[0] === 'plugin' && args[2] === 'close') {
      const p = panes.find(p => p.pane_id === args[3]); panes.splice(panes.indexOf(p), 1); output = result('plugin_pane_closed', { pane_id: p.pane_id });
    } else if (args[1] === 'rename') {
      const kind = args[0], surface = (kind === 'pane' ? panes : kind === 'tab' ? tabs : workspaces).find(p => p[`${kind}_id`] === args[2]);
      surface.label = args[3]; output = result(`${kind}_info`, { [kind]: surface });
    } else if (args[0] === 'pane' && args[1] === 'resize') output = result('pane_resize', { resize: { pane_id: args[3] } });
    else if (args[0] === 'tab' && args[1] === 'focus') output = result('tab_focused');
    else throw new Error(`Unexpected native call ${args.join(' ')}`);
    return hook ? await hook(args, output) : output;
  };
  const source = n => new StreamsCardSource({ nativeSessionId: native(n), readContext: async () => contexts[n - 1] });
  const client = (terminal, extra = {}) => {
    const p = panes.find(p => p.terminal_id === terminal);
    return new StreamsWorkspaceClient({ env: { HERDR_ENV: '1', HERDR_SOCKET_PATH: socket, HERDR_PANE_ID: p.pane_id, HERDR_TAB_ID: p.tab_id, HERDR_WORKSPACE_ID: 'w', ...extra }, pluginRoot: root, stateDirectory, run: run(terminal) });
  };
  const owner = async n => { const c = client(`term${n}`), s = source(n), context = await c.bootstrap(config); return { c, s, context, open: (mode = 'automatic', signal) => c.openDisplay(s, context, config, signal, mode) }; };
  return { root, socket, stateDirectory, panes, tabs, workspaces, contexts, calls, snapshot, client, source, owner, hook: fn => { hook = fn; }, registration: value => { registration = value; }, plugin };
}
const launches = f => f.calls.filter(a => a[0] === 'plugin' && a[2] === 'open');
const effects = f => f.calls.filter(a => a[1] === 'report-metadata' || a[1] === 'rename' || a[1] === 'focus' || a[2] === 'open' || a[2] === 'close');
test('automatic enrolled owner opens an isolated pair; reused display has no duplicate, catalog write or workspace launch', async t => {
  const f = await fixture(t), a = await f.owner(1); a.c.env.STREAMS_CARDS_CONFIG = '/native/presentation.json';
  const pair = await a.open(); assert.equal(pair.kind, 'paired');
  assert.equal(pair.agent.tokens.strm_native, native(1)); assert.ok(!Object.keys(pair.agent.tokens).some(k => k.startsWith('wca_')));
  assert.equal((await a.open()).nonce, pair.nonce); assert.equal(launches(f).length, 1);
  const launch = launches(f)[0]; assert.ok(launch.includes('streams-work-cards')); assert.ok(launch.includes('--no-focus'));
  const vars = launch.filter((_, i) => launch[i - 1] === '--env');
  assert.ok(vars.includes(`STREAMS_CARDS_OWNER_TERMINAL=term1`)); assert.ok(vars.some(v => v.startsWith('STREAMS_CARDS_SCOPE=')));
  assert.ok(vars.includes('STREAMS_CARDS_CONFIG=/native/presentation.json'));
  assert.ok(vars.includes(`HERDR_SOCKET_PATH=${f.socket}`));
  assert.ok(vars.includes('STREAMS_CARDS_READ_ENV={}'));
  assert.ok(!f.calls.some(a => a[0] === 'workspace' && ['open', 'launch', 'create'].includes(a[1])));
  assert.deepEqual((await readdir(f.root)).filter(name => !/^\.work-cards-[a-f0-9]{64}\.lock$/.test(name)), ['checkout-1', 'checkout-2', 'state']);
});
for (const [workspace_naming, tab_naming] of [[true, false], [false, true], [false, false], [true, true]]) test(`manual startup projects independent names ${workspace_naming}/${tab_naming} and summaries without a card`, async t => {
  const f = await fixture(t), a = await f.owner(1); f.workspaces[0].label = 'Shared stream';
  const manual = { ...config, workspace_startup: 'manual', workspace_naming, tab_naming };
  const result = await a.c.projectNames(a.s, a.context, manual);
  assert.deepEqual(result, { workspace: workspace_naming ? 'unchanged' : 'disabled', tab: tab_naming ? 'renamed' : 'disabled' });
  await a.c.projectSummaries(a.s, a.context, manual);
  assert.equal(f.workspaces[0].tokens.strm_goal_short, 'Shared'); assert.equal(f.panes[0].tokens.strm_state_short, 'Own 1');
  assert.ok(f.workspaces[0].tokens.strm_binding); assert.equal(launches(f).length, 0);
  assert.ok(!f.calls.some(a => a[0] === 'plugin')); assert.ok(!Object.hasOwn(f.panes[0].tokens, 'strm_nonce'));
  assert.ok(!(await readdir(f.stateDirectory)).some(n => n.endsWith('.pending') || /^[a-f0-9]{64}\.json$/.test(n)));
  if (workspace_naming) { f.contexts[0].stream.title = 'Next shared'; assert.equal((await a.c.projectNames(a.s, a.context, manual)).workspace, 'renamed'); }
  if (tab_naming) { f.contexts[0].session.title = 'Next own'; assert.equal((await a.c.projectNames(a.s, a.context, manual)).tab, 'renamed'); }
});
test('unchanged verified display, names, summaries and caption bypass an occupied effect lock', async t => {
  const f = await fixture(t), a = await f.owner(1);
  await a.open(); await a.c.projectNames(a.s, a.context, config); await a.c.projectSummaries(a.s, a.context, config); await a.c.projectCaption(a.s, a.context, config);
  const before = effects(f).length;
  await withWorkspaceLock(f.socket, 'w', async () => {
    assert.equal((await a.open()).kind, 'paired');
    assert.deepEqual(await a.c.projectNames(a.s, a.context, config), { workspace: 'manual', tab: 'unchanged' });
    await a.c.projectSummaries(a.s, a.context, config);
    await a.c.projectCaption(a.s, a.context, config);
    assert.equal(effects(f).length, before);
    f.contexts[0].session.state_short = 'Changed';
    await assert.rejects(a.c.projectSummaries(a.s, a.context, { ...config, herdr_timeout_ms: 20 }), /lock.*timed out/);
    assert.equal(f.panes[0].tokens.strm_state_short, 'Own 1');
  });
  await a.c.projectSummaries(a.s, a.context, config);
  assert.equal(f.panes[0].tokens.strm_state_short, 'Changed');
});
test('absent compact summaries leave full narratives in the card and do not block naming', async t => {
  const f = await fixture(t), a = await f.owner(1);
  f.contexts[0].stream.goal_short = f.contexts[0].session.state_short = null;
  await a.c.projectSummaries(a.s, a.context, config);
  for (const key of ['strm_goal', 'strm_goal_short']) assert.ok(!Object.hasOwn(f.workspaces[0].tokens, key));
  for (const key of ['strm_now', 'strm_state_short']) assert.ok(!Object.hasOwn(f.panes[0].tokens, key));
  assert.equal((await a.s.read()).stream.goal, 'Shared purpose');
  assert.equal((await a.s.read()).session.state, 'Own progress 1');
  assert.equal((await a.c.projectNames(a.s, a.context, config)).tab, 'renamed');
});
test('long full narratives clear obsolete tokens without altering catalog text or replacing compact summaries', async t => {
  const f = await fixture(t), a = await f.owner(1);
  await a.c.projectSummaries(a.s, a.context, config);
  f.contexts[0].stream.goal = 'Full shared goal\n'.repeat(20);
  f.contexts[0].session.state = 'Full own progress\n'.repeat(20);
  f.workspaces[0].tokens.strm_goal = 'Old truncated goal';
  f.panes[0].tokens.strm_now = 'Old truncated progress';
  const before = structuredClone(f.contexts);
  await a.c.projectSummaries(a.s, a.context, config);
  assert.ok(!Object.hasOwn(f.workspaces[0].tokens, 'strm_goal'));
  assert.ok(!Object.hasOwn(f.panes[0].tokens, 'strm_now'));
  assert.equal(f.workspaces[0].tokens.strm_goal_short, 'Shared');
  assert.equal(f.panes[0].tokens.strm_state_short, 'Own 1');
  assert.deepEqual(f.contexts, before);
  assert.ok(!f.calls.some(args => args.includes('--token') && args.some(arg => arg.startsWith('strm_goal=') || arg.startsWith('strm_now='))));
});
for (const [label, value, expected] of [
  ['80 ASCII scalars', 'x'.repeat(80), 'x'.repeat(80)],
  ['80 supplementary scalars', '😀'.repeat(80), '😀'.repeat(80)],
  ['internal spaces', 'Keep  both spaces', 'Keep  both spaces'],
  ['Unicode format characters', 'Keep\u200dformat', 'Keep\u200dformat'],
  ['81 ASCII scalars', 'x'.repeat(81), null],
  ['81 supplementary scalars', '😀'.repeat(81), null],
  ['controls', 'Do not\njoin words', null],
  ['leading whitespace', ' leading', null],
  ['trailing Unicode whitespace', 'trailing\u00a0', null],
  ['unpaired surrogate', '\ud800', null],
]) test(`compact native hints preserve exact representable text or clear the hint (${label})`, async t => {
  const f = await fixture(t), a = await f.owner(1);
  await a.c.projectSummaries(a.s, a.context, config);
  f.contexts[0].stream.goal_short = f.contexts[0].session.state_short = value;
  f.workspaces[0].tokens.strm_goal_short = f.panes[0].tokens.strm_state_short = 'Stale hint';
  const before = structuredClone(f.contexts);
  await a.c.projectSummaries(a.s, a.context, config);
  assert.equal(f.workspaces[0].tokens.strm_goal_short ?? null, expected);
  assert.equal(f.panes[0].tokens.strm_state_short ?? null, expected);
  assert.equal(f.panes[0].tokens.strm_status, 'active');
  assert.deepEqual(f.contexts, before);
  const count = effects(f).length;
  await a.c.projectSummaries(a.s, a.context, config);
  assert.equal(effects(f).length, count);
});
for (const missing of [null, '']) test(`empty summaries clear native tokens and remain idempotent (${String(missing)})`, async t => {
  const f = await fixture(t), a = await f.owner(1);
  await a.c.projectSummaries(a.s, a.context, config);
  f.contexts[0].stream.goal = f.contexts[0].session.state = '';
  f.contexts[0].stream.goal_short = f.contexts[0].session.state_short = missing;
  Object.assign(f.workspaces[0].tokens, { strm_goal: 'Old goal', strm_goal_short: 'Old compact goal' });
  Object.assign(f.panes[0].tokens, { strm_now: 'Old progress', strm_state_short: 'Old compact progress' });
  await a.c.projectSummaries(a.s, a.context, config);
  for (const key of ['strm_goal', 'strm_goal_short']) assert.ok(!Object.hasOwn(f.workspaces[0].tokens, key));
  for (const key of ['strm_now', 'strm_state_short']) assert.ok(!Object.hasOwn(f.panes[0].tokens, key));
  assert.equal(f.panes[0].tokens.strm_status, 'active');
  const before = effects(f).length;
  await a.c.projectSummaries(a.s, a.context, config);
  assert.equal(effects(f).length, before);
});
for (const change of ['partial', 'legacy', 'pending', 'live-unowned', 'conflicting-binding', 'binding-save']) test(`empty-pair projections fail closed on ${change}`, async t => {
  const f = await fixture(t), a = await f.owner(1);
  if (change === 'partial') f.panes[0].tokens.strm_nonce = 'partial';
  if (change === 'legacy') f.panes[0].tokens.wca_agent = 'legacy';
  if (change === 'conflicting-binding') f.workspaces[0].tokens = { strm_binding: 'wrong', strm_stream: 'foreign' };
  if (change === 'pending' || change === 'live-unowned') {
    const saved = await a.s.read(), p = f.panes[0], key = streamsDigest([f.socket, saved.scope, [p.pane_id, p.terminal_id, p.workspace_id, p.tab_id]]);
    const state = new StreamsNativeState(f.stateDirectory);
    await state.write({ version: 1, key, phase: 'pending', nonce: 'uncertain' });
    if (change === 'live-unowned') await state.write({ version: 1, key, phase: 'ready', nonce: 'uncertain', paneId: 'w:p2', terminalId: 'term2', tabId: 'w:t2' });
  }
  if (change === 'binding-save') f.hook((args, output) => { if (args[1] === 'report-metadata') { delete f.workspaces[0].tokens.strm_binding; } return output; });
  for (const method of ['projectSummaries', 'projectNames']) await assert.rejects(a.c[method](a.s, a.context, config));
  assert.equal(launches(f).length, 0); assert.ok(!f.calls.some(a => a[1] === 'rename' || a.includes('strm_goal_short=Shared')));
});
test('card callers cannot initialize empty-pair workspace projections', async t => {
  const f = await fixture(t), c = f.client('term2', { STREAMS_CARDS_OWNER_TERMINAL: 'term1', STREAMS_CARDS_NONCE: 'nonce' }), context = await c.bootstrap(config);
  for (const method of ['projectSummaries', 'projectNames']) await assert.rejects(c[method](f.source(1), context, config));
  assert.equal(effects(f).length, 0); assert.deepEqual(f.workspaces[0].tokens, {});
});
test('launcher forwards only bounded owner selectors and the pinned endpoint', async t => {
  const f = await fixture(t), a = await f.owner(1);
  Object.assign(a.c.env, { HOME: '/owner/home', XDG_CONFIG_HOME: '/owner/config', PI_CODING_AGENT_DIR: '/owner/pi', HERDR_BIN_PATH: '/owner/herdr', PATH: '/owner/bin', SECRET_TOKEN: 'not-forwarded' });
  await a.open(); const args = launches(f)[0], vars = args.filter((_, i) => args[i - 1] === '--env');
  const captured = JSON.parse(vars.find(v => v.startsWith('STREAMS_CARDS_READ_ENV=')).slice('STREAMS_CARDS_READ_ENV='.length));
  assert.deepEqual(captured, { HOME: '/owner/home', XDG_CONFIG_HOME: '/owner/config', PI_CODING_AGENT_DIR: '/owner/pi', HERDR_BIN_PATH: '/owner/herdr', PATH: '/owner/bin' });
  assert.ok(vars.includes(`HERDR_SOCKET_PATH=${f.socket}`)); assert.doesNotMatch(JSON.stringify(vars), /SECRET_TOKEN|not-forwarded/);
});
test('Sessions A and B coexist in one shared Stream workspace with independent progress and checkouts', async t => {
  const f = await fixture(t), a = await f.owner(1), b = await f.owner(2); await a.open(); await b.open();
  await a.c.projectSummaries(a.s, a.context, config); await b.c.projectSummaries(b.s, b.context, config);
  assert.equal(f.workspaces[0].tokens.strm_stream, 'shared'); assert.equal(f.workspaces[0].tokens.strm_goal_short, 'Shared');
  assert.equal(f.panes[0].tokens.strm_state_short, 'Own 1'); assert.equal(f.panes[1].tokens.strm_state_short, 'Own 2');
  for (const agent of [a, b]) {
    const snapshot = await agent.s.read(), target = renderStreamsCard(snapshot, 100, 10).documents.get('a');
    await agent.c.openDocument(agent.s, agent.context, snapshot, target, config);
  }
  const docs = launches(f).filter(a => a.includes('document'));
  assert.equal(docs.length, 2); assert.ok(docs[0].some(v => v.includes('checkout-1/docs/design.md'))); assert.ok(docs[1].some(v => v.includes('checkout-2/docs/design.md')));
});
test('concurrent independent clients serialize discovery through readiness', async t => {
  const f = await fixture(t), a = await f.owner(1), reloaded = await f.owner(1);
  const [one, two] = await Promise.all([a.open(), reloaded.open()]); assert.equal(one.nonce, two.nonce); assert.equal(launches(f).length, 1);
});
test('closed card keeps owner alive, persists suppression across reload, and explicit open recreates exactly once', async t => {
  const f = await fixture(t), a = await f.owner(1), pair = await a.open();
  await a.c.closeDisplay(a.s, a.context, config); assert.ok(f.panes.some(p => p.terminal_id === 'term1')); assert.ok(!f.panes.some(p => p.terminal_id === pair.card.terminal_id));
  const reloaded = await f.owner(1); assert.deepEqual(await reloaded.open(), { kind: 'suppressed' }); assert.equal(launches(f).length, 1);
  assert.equal((await reloaded.open('explicit')).kind, 'paired'); assert.equal(launches(f).length, 2);
});
test('external card close also suppresses automatic reopening; no requirement to have received q', async t => {
  const f = await fixture(t), a = await f.owner(1), pair = await a.open(); f.panes.splice(f.panes.findIndex(p => p.terminal_id === pair.card.terminal_id), 1);
  assert.equal((await (await f.owner(1)).open()).kind, 'suppressed');
});
for (const failure of ['timeout', 'malformed', 'replayed', 'persist']) test(`uncertain ${failure} launch retains evidence and blocks duplicates even explicit after reload`, async t => {
  const f = await fixture(t), a = await f.owner(1), original = StreamsNativeState.prototype.write;
  if (failure === 'persist') StreamsNativeState.prototype.write = async function(value) { if (value.phase === 'ready') throw Error('post-launch persistence unavailable'); return original.call(this, value); };
  else f.hook((args, output) => {
    if (args[0] !== 'plugin' || args[2] !== 'open') return output;
    if (failure === 'timeout') throw Error('timeout after launch'); if (failure === 'malformed') return '{';
    const response = JSON.parse(output); response.result.plugin_pane.pane = f.panes[0]; return JSON.stringify(response);
  });
  try { await assert.rejects(a.open()); } finally { StreamsNativeState.prototype.write = original; }
  f.hook(undefined); const reloaded = await f.owner(1);
  await assert.rejects(reloaded.open('explicit'), /Uncertain/); assert.equal(launches(f).length, 1); assert.ok(!f.calls.some(a => a[2] === 'close'));
});
for (const change of ['owner pane', 'owner terminal', 'owner tab', 'stream', 'UUID', 'workspace binding', 'nonce', 'duplicate', 'foreign marker']) test(`native effect refuses ${change} mismatch/movement/replay`, async t => {
  const f = await fixture(t), a = await f.owner(1), pair = await a.open(); const before = effects(f).length;
  if (change === 'owner pane') f.panes[0].pane_id = 'w:p99';
  if (change === 'owner terminal') f.panes[0].terminal_id = 'replacement';
  if (change === 'owner tab') f.panes[0].tab_id = 'w:t2';
  if (change === 'stream') f.contexts[0].session.stream_id = f.contexts[0].stream.id = 'other';
  if (change === 'UUID') f.panes[0].tokens.strm_native = native(2);
  if (change === 'workspace binding') f.workspaces[0].tokens.strm_host = 'foreign';
  if (change === 'nonce') for (const p of f.panes.filter(p => p.tokens.strm_nonce)) p.tokens.strm_nonce = 'replayed';
  if (change === 'duplicate') f.panes[1].tokens = { ...pair.card.tokens };
  if (change === 'foreign marker') f.panes[0].tokens.strm_agent = 'foreign';
  await assert.rejects(a.c.projectSummaries(a.s, a.context, config)); assert.equal(effects(f).length, before);
});
test('cancelled caller has no native effects and does not reserve a launch', async t => {
  const f = await fixture(t), a = await f.owner(1), controller = new AbortController(); controller.abort();
  await assert.rejects(a.open('automatic', controller.signal)); assert.equal(effects(f).length, 0); assert.equal(launches(f).length, 0);
});
for (const scenario of ['empty-new', 'empty-closed', 'absent-closed', 'absent-ready', 'editor-new', 'editor-ready']) test(`cancelled completed reservation restores ${scenario} evidence before any dispatch`, async t => {
  const f = await fixture(t), a = await f.owner(1), controller = new AbortController(), state = new StreamsNativeState(f.stateDirectory);
  const editor = scenario.startsWith('editor');
  let painted, target;
  if (scenario !== 'empty-new') await a.open();
  if (editor) {
    painted = await a.s.read(); target = renderStreamsCard(painted, 100, 10).documents.get('a');
    if (scenario === 'editor-ready') {
      await a.c.openDocument(a.s, a.context, painted, target, config);
      const pane = f.panes.find(p => p.tokens.strm_doc);
      f.panes.splice(f.panes.indexOf(pane), 1); f.tabs.splice(f.tabs.findIndex(t => t.tab_id === pane.tab_id), 1);
    }
  } else if (scenario !== 'empty-new') {
    if (scenario.endsWith('closed')) await a.c.closeDisplay(a.s, a.context, config);
    else f.panes.splice(f.panes.findIndex(p => p.tokens.strm_role === 'card'), 1);
    if (scenario.startsWith('empty')) f.panes[0].tokens = {};
  }
  const launch = signal => editor ? a.c.openDocument(a.s, a.context, painted, target, config, signal) : a.open('explicit', signal);
  const before = launches(f).length, write = StreamsNativeState.prototype.write;
  let pending, previous, release, entered;
  const reserved = new Promise(resolve => { entered = resolve; });
  StreamsNativeState.prototype.write = async function(value) {
    if (value.phase !== 'pending' || value.paneId) return write.call(this, value);
    previous = await this.read(value.key); await write.call(this, value); pending = value;
    entered(); await new Promise(resolve => { release = resolve; });
  };
  try {
    const operation = launch(controller.signal), rejected = assert.rejects(operation, /abort/i);
    await reserved; controller.abort(); release(); await rejected;
  } finally { release?.(); StreamsNativeState.prototype.write = write; }
  assert.equal(launches(f).length, before);
  assert.deepEqual(await state.read(pending.key), previous);
  assert.ok(!(await readdir(f.stateDirectory)).includes(`${pending.key}.json.pending`));
  await launch(); await launch(); assert.equal(launches(f).length, before + 1);
});
test('cancellation inside absent marker guard restores prior closed proof without clearing markers', async t => {
  const f = await fixture(t), a = await f.owner(1), controller = new AbortController(); await a.open(); await a.c.closeDisplay(a.s, a.context, config);
  const write = StreamsNativeState.prototype.write, read = a.s.read.bind(a.s), before = effects(f).length;
  let reserved = false, pending, previous;
  StreamsNativeState.prototype.write = async function(value) {
    previous = await this.read(value.key); await write.call(this, value); pending = value; reserved = true;
  };
  a.s.read = async signal => { if (reserved) { controller.abort(); signal.throwIfAborted(); } return read(signal); };
  try { await assert.rejects(a.open('explicit', controller.signal), /abort/i); }
  finally { StreamsNativeState.prototype.write = write; a.s.read = read; }
  assert.equal(effects(f).length, before);
  assert.deepEqual(await new StreamsNativeState(f.stateDirectory).read(pending.key), previous);
  await a.open('explicit'); assert.equal(launches(f).length, 2);
});
test('cancellation after absent marker dispatch retains uncertainty and blocks duplicate attempts', async t => {
  const f = await fixture(t), a = await f.owner(1), controller = new AbortController(); await a.open(); await a.c.closeDisplay(a.s, a.context, config);
  f.hook((args, output) => { if (args.includes('--clear-token')) controller.abort(); return output; });
  await assert.rejects(a.open('explicit', controller.signal), /abort/i); f.hook(undefined);
  await assert.rejects(a.open('explicit'), /Uncertain/); assert.equal(launches(f).length, 1);
});
test('partial reservation write failure is never rolled back even before native dispatch', async t => {
  const f = await fixture(t), a = await f.owner(1), write = StreamsNativeState.prototype.write;
  StreamsNativeState.prototype.write = async function(value) { await write.call(this, value); throw Error('reservation fsync failed'); };
  try { await assert.rejects(a.open(), /reservation fsync failed/); } finally { StreamsNativeState.prototype.write = write; }
  await assert.rejects(a.open('explicit'), /Uncertain/); assert.equal(launches(f).length, 0);
});
test('failed unlaunched rollback stays pending and never dispatches or permits retry', async t => {
  const f = await fixture(t), a = await f.owner(1), controller = new AbortController();
  const write = StreamsNativeState.prototype.write, discard = StreamsNativeState.prototype.discardUnlaunched;
  StreamsNativeState.prototype.write = async function(value) { await write.call(this, value); controller.abort(); };
  StreamsNativeState.prototype.discardUnlaunched = async () => { throw Error('rollback unavailable'); };
  try { await assert.rejects(a.open('automatic', controller.signal), /rollback unavailable/); }
  finally { StreamsNativeState.prototype.write = write; StreamsNativeState.prototype.discardUnlaunched = discard; }
  await assert.rejects(a.open('explicit'), /Uncertain/); assert.equal(launches(f).length, 0);
});
test('unlaunched rollback refuses another nonce and retains the uncertainty gate', async t => {
  const f = await fixture(t), state = new StreamsNativeState(f.stateDirectory), key = streamsDigest('rollback');
  const pending = { version:1, key, phase:'pending', nonce:'current' }; await state.write(pending);
  await assert.rejects(state.discardUnlaunched({ ...pending, nonce:'old' }, null), /reservation changed/);
  assert.deepEqual(await state.read(key), pending); assert.ok((await readdir(f.stateDirectory)).includes(`${key}.json.pending`));
});
test('wrong/disabled registration requires explicit link; never repoints legacy registration', async t => {
  const f = await fixture(t), a = await f.owner(1); f.registration({ ...f.plugin(), plugin_id: 'work-cards' });
  await assert.rejects(a.open(), /explicitly link/); assert.ok(!f.calls.some(a => a[1] === 'link')); assert.equal(launches(f).length, 0);
});
test('manual labels are preserved independently; only actual first agent pane names its tab', async t => {
  const f = await fixture(t), a = await f.owner(1), b = await f.owner(2); await a.open(); await b.open();
  f.tabs[1].label = 'Manual tab';
  assert.deepEqual(await a.c.projectNames(a.s, a.context, config), { workspace: 'manual', tab: 'renamed' });
  assert.equal((await b.c.projectNames(b.s, b.context, config)).tab, 'manual'); assert.equal(f.workspaces[0].label, 'Custom workspace');
  f.tabs[0].label = 'User override'; f.contexts[0].session.title = 'Changed title';
  assert.equal((await a.c.projectNames(a.s, a.context, config)).tab, 'manual'); assert.equal(f.tabs[0].label, 'User override');
  f.panes.unshift({ pane_id: 'w:p0', terminal_id: 'shell', workspace_id: 'w', tab_id: 'w:t1', tokens: {} });
  assert.equal((await a.c.projectNames(a.s, a.context, config)).tab, 'not-first');
});
test('workspace already matching Stream title can adopt provenance; later shared titles track', async t => {
  const f = await fixture(t); f.workspaces[0].label = 'Shared stream'; const a = await f.owner(1); await a.open();
  assert.equal((await a.c.projectNames(a.s, a.context, config)).workspace, 'unchanged');
  f.contexts[0].stream.title = 'Changed shared'; assert.equal((await a.c.projectNames(a.s, a.context, config)).workspace, 'renamed');
});
test('card verifies launcher nonce, exact owner, and cannot publish another Session progress', async t => {
  const f = await fixture(t), a = await f.owner(1), pair = await a.open();
  const card = f.client(pair.card.terminal_id, { STREAMS_CARDS_NONCE: pair.nonce, STREAMS_CARDS_OWNER_TERMINAL: 'term1' }), context = await card.bootstrap(config);
  assert.equal((await card.verifyDisplay(a.s, context, config)).nonce, pair.nonce);
  await assert.rejects(card.projectSummaries(a.s, context, config), /Only/);
  await assert.rejects(card.verifyDisplay(f.source(2), context, config));
  const wrong = f.client(pair.card.terminal_id, { STREAMS_CARDS_NONCE: 'old', STREAMS_CARDS_OWNER_TERMINAL: 'term1' }); await assert.rejects(wrong.verifyDisplay(a.s, await wrong.bootstrap(config), config));
});
for (const terminal of [undefined, 'foreign']) test(`card rejects missing/wrong launcher owner terminal ${terminal}`, async t => {
  const f = await fixture(t), a = await f.owner(1), pair = await a.open();
  const c = f.client(pair.card.terminal_id, { STREAMS_CARDS_NONCE: pair.nonce, STREAMS_CARDS_OWNER_TERMINAL: terminal }), context = await c.bootstrap(config), before = effects(f).length;
  for (const method of ['verifyDisplay', 'projectCaption', 'closeDisplay']) await assert.rejects(c[method](a.s, context, config), /exact owned/);
  assert.equal(effects(f).length, before);
});
test('explicit naming reclaims manual workspace and tab even with naming disabled', async t => {
  const f = await fixture(t), a = await f.owner(1);
  f.workspaces[0].label = 'Manual workspace'; f.tabs[0].label = 'Manual tab';
  await a.c.projectNames(a.s, a.context, config);
  assert.equal((await a.c.projectNames(a.s, a.context, config)).tab, 'manual');
  const disabled = {...config, workspace_naming: false, tab_naming: false};
  const reclaimed = await a.c.projectNames(a.s, a.context, disabled, undefined, 'explicit');
  assert.equal(reclaimed.tab, 'renamed'); assert.equal(reclaimed.workspace, 'renamed');
  assert.equal(f.tabs[0].label, 'Task 1');
  f.contexts[0].session.title = 'Later';
  assert.equal((await a.c.projectNames(a.s, a.context, config)).tab, 'renamed');
  assert.equal(f.tabs[0].label, 'Later');
});
test('matching tab is adopted and tracks future automatic titles', async t => {
  const f = await fixture(t), a = await f.owner(1); f.tabs[0].label = 'Task 1';
  assert.equal((await a.c.projectNames(a.s, a.context, config)).tab, 'unchanged');
  f.contexts[0].session.title = 'Changed'; assert.equal((await a.c.projectNames(a.s, a.context, config)).tab, 'renamed'); assert.equal(f.tabs[0].label, 'Changed');
});
test('tab rename followed by provenance failure adopts matching text on retry', async t => {
  const f = await fixture(t), a = await f.owner(1), write = NameStore.prototype.write;
  NameStore.prototype.write = async function(scope, ...args) { if (scope.kind === 'tab') throw Error('name save failed'); return write.call(this, scope, ...args); };
  try { await assert.rejects(a.c.projectNames(a.s, a.context, config), /name save failed/); } finally { NameStore.prototype.write = write; }
  assert.equal(f.tabs[0].label, 'Task 1'); assert.equal((await a.c.projectNames(a.s, a.context, config)).tab, 'unchanged');
  f.contexts[0].session.title = 'Later'; await a.c.projectNames(a.s, a.context, config); assert.equal(f.tabs[0].label, 'Later');
});
test('editor and later first-pane owner share the same tab naming provenance scope', async t => {
  const f = await fixture(t), a = await f.owner(1); await a.open(); const snapshot = await a.s.read(), target = renderStreamsCard(snapshot, 100, 10).documents.get('a');
  await a.c.openDocument(a.s, a.context, snapshot, target, config);
  const editor = f.panes.find(p => p.tokens.strm_doc), names = new NameStore(f.stateDirectory);
  const scope = await names.scope({ socket: f.socket, workspaceId: 'w', bindingNonce: f.workspaces[0].tokens.strm_binding, kind: 'tab', surfaceId: editor.tab_id });
  assert.equal((await names.read(scope)).record.lastApplied, target.path);
  const history = await names.read(scope); await names.write(scope, { ...history.record, mode: 'manual' }, history.revision);
  const next = f.contexts[1]; next.native = { frontend: 'herdr', pane_id: editor.pane_id, tab_id: editor.tab_id, workspace_id: 'w' };
  const c = f.client(editor.terminal_id), source = f.source(2), context = await c.bootstrap(config);
  assert.equal((await c.projectNames(source, context, config)).tab, 'manual');
  assert.equal(f.tabs.find(t => t.tab_id === editor.tab_id).label, target.path);
});
test('owned editor reuse matches exact canonical tuple and checkout lifetime, preserving manual tab', async t => {
  const f = await fixture(t), a = await f.owner(1); await a.open(); const snapshot = await a.s.read(), target = renderStreamsCard(snapshot, 100, 10).documents.get('a');
  await a.c.openDocument(a.s, a.context, snapshot, target, config); const editor = f.panes.find(p => p.tokens.strm_doc); const tab = f.tabs.find(t => t.tab_id === editor.tab_id); tab.label = 'User editor';
  await a.c.openDocument(a.s, a.context, snapshot, target, config); assert.equal(launches(f).filter(a => a.includes('document')).length, 1); assert.equal(tab.label, 'User editor');
  editor.tokens.strm_doc_pane = 'foreign'; await assert.rejects(a.c.openDocument(a.s, a.context, snapshot, target, config), /tuple changed/);
});
test('missing document or app error never launches editor nor alters shared references', async t => {
  const f = await fixture(t), a = await f.owner(1); await a.open(); const before = JSON.stringify(f.contexts[0].stream);
  await rm(f.contexts[0].document_locations[0].file); let snapshot = await a.s.read(), target = renderStreamsCard(snapshot, 100, 10).documents.get('a');
  await assert.rejects(a.c.openDocument(a.s, a.context, snapshot, target, config), /not present/);
  Object.assign(f.contexts[0].document_locations[0], { file: null, error: 'Registry mapping unavailable' }); snapshot = await a.s.read(); target = renderStreamsCard(snapshot, 100, 10).documents.get('a');
  await assert.rejects(a.c.openDocument(a.s, a.context, snapshot, target, config), /Registry mapping unavailable/); assert.equal(JSON.stringify(f.contexts[0].stream), before); assert.equal(launches(f).length, 1);
});
test('unverified legacy tokens cannot masquerade as Streams pairing', async t => {
  const f = await fixture(t), a = await f.owner(1); f.panes[0].tokens = { wca_agent: 'legacy', wca_nonce: 'old', wca_pi: 'term1', wca_card: 'term3', wca_role: 'agent' };
  assert.equal(classifyStreamsPair(f.snapshot(), 'identity', 'term1').kind, 'empty'); await assert.rejects(a.open(), /Legacy publisher/); assert.equal(launches(f).length, 0);
});
for (const change of ['owner', 'card', 'binding', 'nonce', 'duplicate', 'scope', 'cancel']) test(`reserved pair rejects late ${change} change without stamping owner or closing anything`, async t => {
  const f = await fixture(t), a = await f.owner(1), controller = new AbortController();
  f.hook((args, output) => {
    if (args[1] !== 'report-metadata' || !args.includes('strm_role=card')) return output;
    const card = f.panes.find(p => p.tokens.strm_role === 'card');
    if (change === 'owner') f.panes[0].terminal_id = 'replacement';
    if (change === 'card') card.tab_id = 'w:t2';
    if (change === 'binding') f.workspaces[0].tokens.strm_binding = 'replacement';
    if (change === 'nonce') card.tokens.strm_nonce = 'replacement';
    if (change === 'duplicate') f.panes[1].tokens = { ...card.tokens };
    if (change === 'scope') f.contexts[0].catalog_source = 'other';
    if (change === 'cancel') controller.abort();
    return output;
  });
  await assert.rejects(a.open('automatic', controller.signal));
  assert.ok(!f.calls.some(a => a.includes('strm_role=agent'))); assert.ok(!f.calls.some(a => a[2] === 'close')); assert.equal(launches(f).length, 1);
});
test('binding lifetime changes and moved card pane IDs cannot replay existing proofs', async t => {
  const f = await fixture(t), a = await f.owner(1), pair = await a.open(); f.workspaces[0].tokens.strm_binding = 'replacement';
  await assert.rejects(a.c.projectNames(a.s, a.context, config), /lifetime changed/);
  const reloaded = await f.owner(1); f.panes.find(p => p.terminal_id === pair.card.terminal_id).pane_id = 'w:p99';
  await assert.rejects(reloaded.c.verifyDisplay(reloaded.s, reloaded.context, config), /durable native launch evidence/);
});
test('removed ownership markers never allow duplicate launch over recorded live resources', async t => {
  const f = await fixture(t), a = await f.owner(1); await a.open();
  for (const p of f.panes) if (p.tokens.strm_agent) p.tokens = {};
  await assert.rejects(a.open('explicit'), /still live/); assert.equal(launches(f).length, 1);
});
test('caption follows shared title but preserves an independent native manual label', async t => {
  const f = await fixture(t), a = await f.owner(1), pair = await a.open();
  await a.c.projectCaption(a.s, a.context, config); const card = f.panes.find(p => p.terminal_id === pair.card.terminal_id); assert.equal(card.label, 'Shared stream');
  card.label = 'User card caption'; f.contexts[0].stream.title = 'Next title';
  await a.c.projectCaption(a.s, a.context, config); assert.equal(card.label, 'User card caption');
  card.label = 'Shared stream'; await a.c.projectCaption(a.s, a.context, config); assert.equal(card.label, 'Shared stream');
});
test('caption renamed back to the native default literal becomes a durable manual pin', async t => {
  const f = await fixture(t), a = await f.owner(1), pair = await a.open(); await a.c.projectCaption(a.s, a.context, config);
  const card = f.panes.find(p => p.terminal_id === pair.card.terminal_id); card.label = 'Streams work card';
  f.contexts[0].stream.title = 'Streams work card'; await a.c.projectCaption(a.s, a.context, config);
  assert.equal(card.tokens.strm_caption_manual, '1');
  f.contexts[0].stream.title = 'Next shared'; await a.c.projectCaption(a.s, a.context, config); assert.equal(card.label, 'Streams work card');
});
for (const fail of ['before-save', 'partial-save']) test(`caption rename with ${fail} uncertainty never adopts matching text`, async t => {
  const f = await fixture(t), a = await f.owner(1), pair = await a.open(), card = f.panes.find(p => p.terminal_id === pair.card.terminal_id);
  f.hook((args, output) => {
    if (fail === 'before-save' && args[0] === 'pane' && args[1] === 'rename') throw Error('ack lost');
    if (fail === 'partial-save' && args[1] === 'report-metadata' && args.some(s => s.startsWith('strm_caption='))) delete card.tokens.strm_caption;
    return output;
  });
  await assert.rejects(a.c.projectCaption(a.s, a.context, config)); assert.equal(card.label, 'Shared stream'); assert.equal(card.tokens.strm_caption, undefined);
  f.hook(undefined); await a.c.projectCaption(a.s, a.context, config); assert.equal(card.tokens.strm_caption_manual, '1');
  f.contexts[0].stream.title = 'Later'; await a.c.projectCaption(a.s, a.context, config); assert.equal(card.label, 'Shared stream');
});
test('uncertain editor launch blocks duplicates across reload and never auto-closes unverified resource', async t => {
  const f = await fixture(t), a = await f.owner(1); await a.open(); const snapshot = await a.s.read(), target = renderStreamsCard(snapshot, 100, 10).documents.get('a');
  f.hook((args, output) => { if (args[0] === 'plugin' && args[2] === 'open' && args.includes('document')) throw Error('editor timeout after launch'); return output; });
  await assert.rejects(a.c.openDocument(a.s, a.context, snapshot, target, config)); f.hook(undefined);
  const reload = await f.owner(1); await assert.rejects(reload.c.openDocument(reload.s, reload.context, snapshot, target, config), /Uncertain editor/);
  assert.equal(launches(f).filter(a => a.includes('document')).length, 1); assert.ok(!f.calls.some(a => a[2] === 'close'));
});
test('a different checkout ID at the same canonical file cannot reuse an older editor', async t => {
  const f = await fixture(t), a = await f.owner(1); await a.open(); let snapshot = await a.s.read(), target = renderStreamsCard(snapshot, 100, 10).documents.get('a');
  await a.c.openDocument(a.s, a.context, snapshot, target, config);
  f.contexts[0].checkouts[0].checkout_id = f.contexts[0].document_locations[0].checkout_id = 'replacement-checkout';
  snapshot = await a.s.read(); target = renderStreamsCard(snapshot, 100, 10).documents.get('a'); await a.c.openDocument(a.s, a.context, snapshot, target, config);
  assert.equal(launches(f).filter(a => a.includes('document')).length, 2);
});
test('a narrative progress edit during native snapshot revalidation succeeds while a native owner change rejects', async t => {
  const f = await fixture(t), a = await f.owner(1); await a.open();
  let once = true;
  f.hook((args, output) => { if (once && args[0] === 'api') { once = false; f.contexts[0].session.state = 'New progress'; } return output; });
  await a.c.projectSummaries(a.s, a.context, config);
  assert.equal(f.panes[0].tokens.strm_state_short, 'Own 1');
  const before = effects(f).length; once = true;
  f.hook((args, output) => { if (once && args[0] === 'api') { once = false; f.contexts[0].native.tab_id = 'w:t2'; } return output; });
  await assert.rejects(a.c.projectSummaries(a.s, a.context, config), /context changed/);
  assert.equal(effects(f).length, before);
});
test('openDisplay resizes split immediately after launch and absent pair reconciles a stale pending gate', async t => {
  const f = await fixture(t), a = await f.owner(1), pair = await a.open();
  const openIdx = f.calls.findIndex(args => args[0] === 'plugin' && args[2] === 'open');
  const resizeIdx = f.calls.findIndex(args => args[0] === 'pane' && args[1] === 'resize');
  const cardStampIdx = f.calls.findIndex(args => args[1] === 'report-metadata' && args.includes('strm_role=card'));
  assert.ok(openIdx >= 0 && resizeIdx > openIdx && resizeIdx < cardStampIdx);
  // Simulate a card that finished stamping both panes before crashing prior to ready, and was later closed.
  const state = new StreamsNativeState(f.stateDirectory);
  const saved = await a.s.read(), key = streamsDigest([f.socket, saved.scope, ['w:p1', 'term1', 'w', 'w:t1']]);
  await state.write({ version: 1, key, phase: 'pending', nonce: pair.nonce, paneId: pair.card.pane_id, terminalId: pair.card.terminal_id, tabId: pair.card.tab_id });
  f.panes.splice(f.panes.findIndex(p => p.terminal_id === pair.card.terminal_id), 1);
  await a.c.projectSummaries(a.s, a.context, config);
  assert.equal((await a.open('automatic')).kind, 'suppressed');
  assert.equal((await a.open('explicit')).kind, 'paired');
  assert.ok(!(await readdir(f.stateDirectory)).includes(`${key}.json.pending`));
});
