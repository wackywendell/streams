import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, rm, realpath, unlink, mkdir, symlink } from 'node:fs/promises';
import { tmpdir, homedir } from 'node:os';
import { join, relative } from 'node:path';
import { spawnSync } from 'node:child_process';
import { HerdrClient, cardIdentity, withWorkspaceLock } from '../dist/herdr/client.js';
import { DEFAULT_CONFIG } from '../dist/config.js';

const snapshot = path => ({ slug: 'test', path, revision: 'revision', card: {
  title: 'Card ; $(touch nope)', status: 'active', state: 'Making progress', cwd: '/repo', documents: [], issues: [], prs: [],
} });
const pane = (id = 'w1:p1', terminal = 'term1') => ({ pane_id: id, terminal_id: terminal, workspace_id: 'w1', tab_id: 'w1:t1', tokens: {} });
async function fixture(t) {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'work-cards-herdr-')));
  t.after(() => rm(root, { recursive: true, force: true }));
  const path = join(root, 'test.yaml'); await writeFile(path, 'fixture');
  const caller = pane(); const panes = [caller]; const workspace = { workspace_id: 'w1', tokens: {} }; const calls = [];
  let plugins = []; let nextPane = 2;
  const plugin = { plugin_id: 'work-cards', enabled: true, manifest_path: join(root, 'herdr-plugin.toml'), plugin_root: root,
    source: { kind: 'local' }, version: '0.1.0', min_herdr_version: '0.9.3', platforms: ['macos', 'linux'],
    panes: [{ id: 'card', title: 'Work card', placement: 'split', command: ['node', 'dist/herdr/pane.js'] },
      { id: 'document', title: 'Document', placement: 'tab', command: ['/bin/sh', 'bin/open-document.sh'] }] };
  const result = (type, rest = {}) => JSON.stringify({ result: { type, ...rest } });
  const run = async args => {
    calls.push(args);
    if (args[0] === 'status') return JSON.stringify({ server: { running: true, version: '0.9.3', compatible: true, endpoint_compatible: true, socket: join(root, 'herdr.sock') } });
    if (args[0] === 'pane' && args[1] === 'current') return result('pane_current', { pane: caller });
    if (args[0] === 'pane' && args[1] === 'list') return result('pane_list', { panes });
    if (args[0] === 'workspace' && args[1] === 'get') return result('workspace_info', { workspace });
    if (args[0] === 'plugin' && args[1] === 'list') return result('plugin_list', { plugins });
    if (args[0] === 'plugin' && args[1] === 'link') { plugins = [plugin]; return result('plugin_linked', { plugin }); }
    if (args.slice(0, 3).join(' ') === 'plugin pane open') {
      const number = nextPane++; const opened = pane(`w1:p${number}`, `term${number}`);
      const entrypoint = args[args.indexOf('--entrypoint') + 1];
      if (entrypoint === 'document') opened.tab_id = `w1:t${number}`;
      panes.push(opened);
      return result('plugin_pane_opened', { plugin_pane: { plugin_id: 'work-cards', entrypoint, pane: opened } });
    }
    if (args.slice(0, 3).join(' ') === 'plugin pane close') { panes.splice(panes.findIndex(p => p.pane_id === args[3]), 1); return result('plugin_pane_closed', { pane_id: args[3] }); }
    if (args[1] === 'report-metadata') {
      const target = args[0] === 'workspace' ? workspace : panes.find(p => p.pane_id === args[2]);
      for (let i = 3; i < args.length; i++) {
        if (args[i] === '--token') { const [key, ...value] = args[++i].split('='); target.tokens[key] = value.join('='); }
        if (args[i] === '--clear-token') delete target.tokens[args[++i]];
      }
      return '';
    }
    if (args[0] === 'tab' && args[1] === 'rename') return result('tab_info', { tab: { tab_id: args[2], workspace_id: 'w1', label: args.slice(3).join(' ') } });
    if (args[0] === 'tab' && args[1] === 'focus') return result('documented-but-unchecked-focus-result');
    if (args[1] === 'rename') return result('workspace_info', { workspace });
    if (args[1] === 'resize') return result('pane_resize', { resize: { pane_id: caller.pane_id, focused_pane_id: caller.pane_id, changed: true, layout: {} } });
    throw new Error(`Unexpected ${JSON.stringify(args)}`);
  };
  const env = { HERDR_ENV: '1', HERDR_PANE_ID: caller.pane_id, HERDR_SOCKET_PATH: join(root, 'herdr.sock'), HOME: root };
  const client = new HerdrClient({ env, run, pluginRoot: root });
  return { root, path, caller, panes, workspace, calls, client, env, run, plugin };
}

test('outside Herdr refuses before running any command', async () => {
  const client = new HerdrClient({ env: {}, run: () => { throw new Error('must not run'); } });
  await assert.rejects(client.attachCard(snapshot('/anything'), DEFAULT_CONFIG), /not.*Herdr/i);
});
test('attach opens an explicit no-focus bottom split, passes paths as argv and never publishes summary', async t => {
  const f = await fixture(t); const attached = await f.client.attachCard(snapshot(f.path), DEFAULT_CONFIG);
  assert.equal(attached.workspaceId, 'w1'); assert.equal(attached.paneId, 'w1:p2');
  const open = f.calls.find(a => a.slice(0, 3).join(' ') === 'plugin pane open');
  assert.ok(open.includes('--no-focus')); assert.equal(open[open.indexOf('--target-pane') + 1], 'w1:p1');
  assert.equal(open[open.indexOf('--direction') + 1], 'down');
  assert.ok(open.includes(`WORK_CARDS_CARD=${f.path}`));
  assert.ok(open.includes('EDITOR='));
  assert.equal(open.some(value => value.startsWith('PATH=')), false);
  assert.equal(f.workspace.tokens.wc_card, await cardIdentity(f.path));
  assert.equal(f.workspace.tokens.wc_summary, undefined);
  const rename = f.calls.find(a => a[1] === 'rename'); assert.equal(rename[3], snapshot(f.path).card.title);
  assert.equal(f.calls.some(a => a.includes('--focus') || a[1] === 'run'), false);
});
test('same card reuses a verified pane without duplication; a different card fails closed', async t => {
  const f = await fixture(t); const first = await f.client.attachCard(snapshot(f.path), DEFAULT_CONFIG);
  const second = await f.client.attachCard(snapshot(f.path), DEFAULT_CONFIG);
  assert.equal(second.paneId, first.paneId); assert.equal(second.nonce, first.nonce);
  assert.equal(f.calls.filter(a => a.slice(0, 3).join(' ') === 'plugin pane open').length, 1);
  const other = join(f.root, 'other.yaml'); await writeFile(other, 'another');
  await assert.rejects(f.client.attachCard(snapshot(other), DEFAULT_CONFIG), /conflict/i);
});
test('duplicate and inconsistent pane markers refuse attach', async t => {
  const f = await fixture(t); await f.client.attachCard(snapshot(f.path), DEFAULT_CONFIG);
  f.panes.push({ ...pane('w1:p3', 'term3'), tokens: { ...f.panes[1].tokens } });
  await assert.rejects(f.client.attachCard(snapshot(f.path), DEFAULT_CONFIG), /multiple|conflict/i);
  f.panes.pop(); f.panes[1].tokens.wc_nonce = 'another';
  await assert.rejects(f.client.attachCard(snapshot(f.path), DEFAULT_CONFIG), /inconsistent|conflict/i);
});
test('pane publisher validates live ownership, updates summary, and refuses a rebound workspace', async t => {
  const f = await fixture(t); const attached = await f.client.attachCard(snapshot(f.path), DEFAULT_CONFIG);
  await assert.rejects(f.client.projectCard(snapshot(f.path), DEFAULT_CONFIG, attached), /publisher|owner|pane/i);
  const publisher = new HerdrClient({ env: { ...f.env, HERDR_PANE_ID: attached.paneId, WORK_CARDS_NONCE: attached.nonce }, pluginRoot: f.root,
    run: async a => a[0] === 'pane' && a[1] === 'current'
      ? JSON.stringify({ result: { type: 'pane_current', pane: f.panes[1] } }) : f.run(a) });
  await publisher.projectCard(snapshot(f.path), DEFAULT_CONFIG, attached);
  assert.equal(f.workspace.tokens.wc_summary, 'active · Making progress');
  const writes = f.calls.filter(a => a[1] === 'report-metadata').length;
  const scans = f.calls.filter(a => a[1] === 'list').length;
  await publisher.projectCard(snapshot(f.path), DEFAULT_CONFIG, attached);
  assert.equal(f.calls.filter(a => a[1] === 'report-metadata').length, writes, 'unchanged sidebar tokens do not churn');
  assert.equal(f.calls.filter(a => a[1] === 'list').length, scans, 'polling does not rescan pane/plugin inventories');
  const long = snapshot(f.path); long.card.state = 'x'.repeat(120);
  await publisher.projectCard(long, DEFAULT_CONFIG, attached);
  assert.equal(f.workspace.tokens.wc_summary.length, 80);
  const longWrites = f.calls.filter(a => a[1] === 'report-metadata').length;
  await publisher.projectCard(long, DEFAULT_CONFIG, attached);
  assert.equal(f.calls.filter(a => a[1] === 'report-metadata').length, longWrites);
  f.workspace.tokens.wc_nonce = 'rebound';
  await assert.rejects(publisher.projectCard(snapshot(f.path), DEFAULT_CONFIG, attached), /conflict/i);
  assert.equal(f.workspace.tokens.wc_nonce, 'rebound');
});
test('sidebar projects only canonical Now, preserving cap, dedupe and prose fallback', async t => {
  const f = await fixture(t); const attached = await f.client.attachCard(snapshot(f.path), DEFAULT_CONFIG);
  const publisher = new HerdrClient({ env: { ...f.env, HERDR_PANE_ID: attached.paneId, WORK_CARDS_NONCE: attached.nonce }, pluginRoot: f.root,
    run: async a => a[0] === 'pane' && a[1] === 'current'
      ? JSON.stringify({ result: { type: 'pane_current', pane: f.panes[1] } }) : f.run(a) });
  const saved = snapshot(f.path);
  saved.card.state = 'Goal: Recovery | Now: Retry options documented | Remaining: Implement retry';
  await publisher.projectCard(saved, DEFAULT_CONFIG, attached);
  assert.equal(f.workspace.tokens.wc_summary, 'active · Retry options documented');
  const writes = f.calls.filter(a => a[1] === 'report-metadata').length;
  saved.card.state = 'Goal: Reliable recovery | Now: Retry options documented | Remaining: Decide first';
  await publisher.projectCard(saved, DEFAULT_CONFIG, attached);
  assert.equal(f.calls.filter(a => a[1] === 'report-metadata').length, writes);
  saved.card.state = 'Goal: Recovery | Now: ' + '😀'.repeat(100) + ' | Remaining: Retry';
  await publisher.projectCard(saved, DEFAULT_CONFIG, attached);
  assert.equal(f.workspace.tokens.wc_summary, 'active · ' + '😀'.repeat(71));
  assert.equal(Array.from(f.workspace.tokens.wc_summary).length, 80);
  for (const state of ['Legacy progress', 'Goal: Recovery | Now: Ready',
    'Goal: Recovery | Now: Ready | Remaining: Retry | Other: Extra',
    'Goal: Recovery | Now: Ready Remaining: Hidden | Remaining: Retry']) {
    saved.card.state = state;
    await publisher.projectCard(saved, DEFAULT_CONFIG, attached);
    assert.equal(f.workspace.tokens.wc_summary, Array.from('active · ' + state).slice(0, 80).join(''));
  }
});

test('detach closes only matching pane and clears only its own tokens', async t => {
  const f = await fixture(t); await f.client.attachCard(snapshot(f.path), DEFAULT_CONFIG);
  f.workspace.tokens.unrelated = 'preserved';
  await f.client.detachCard({ path: f.path }, DEFAULT_CONFIG);
  assert.equal(f.panes.length, 1); assert.deepEqual(f.workspace.tokens, { unrelated: 'preserved' });
});
test('restored terminal identity cannot be detached or reused', async t => {
  const f = await fixture(t); await f.client.attachCard(snapshot(f.path), DEFAULT_CONFIG);
  f.panes[1].terminal_id = 'restored-shell';
  await assert.rejects(f.client.detachCard(snapshot(f.path), DEFAULT_CONFIG), /conflict/i);
  assert.equal(f.panes.length, 2);
});
test('malformed response and conflicting registration fail without opening', async t => {
  const f = await fixture(t);
  const bad = new HerdrClient({ env: f.env, pluginRoot: f.root, run: async a => a[1] === 'current' ? '{"result":{"type":"pane_current","pane":{}}}' : f.run(a) });
  await assert.rejects(bad.attachCard(snapshot(f.path), DEFAULT_CONFIG), /invalid|unexpected/i);
  const conflict = new HerdrClient({ env: f.env, pluginRoot: f.root, run: async a => a[0] === 'plugin' && a[1] === 'list'
    ? JSON.stringify({ result: { type: 'plugin_list', plugins: [{ ...f.plugin, enabled: false }] } }) : f.run(a) });
  await assert.rejects(conflict.attachCard(snapshot(f.path), DEFAULT_CONFIG), /registration|disabled/i);
  assert.equal(f.calls.some(a => a.slice(0, 3).join(' ') === 'plugin pane open'), false);
});
test('explicit attach recovers complete markers left by q after the owner pane was removed', async t => {
  const f = await fixture(t); const old = await f.client.attachCard(snapshot(f.path), DEFAULT_CONFIG);
  f.panes.splice(1, 1); // Herdr removes the exited plugin pane, but leaves workspace metadata.
  const unrelated = pane('w1:p9', 'term9'); unrelated.tokens.other = 'preserved'; f.panes.push(unrelated);
  f.workspace.tokens.other = 'preserved'; f.workspace.tokens.wc_summary = 'old'; f.workspace.tokens.wc_status = 'active'; f.calls.length = 0;
  const attached = await f.client.attachCard(snapshot(f.path), DEFAULT_CONFIG);
  assert.notEqual(attached.nonce, old.nonce); assert.notEqual(attached.paneId, old.paneId);
  assert.equal(f.workspace.tokens.wc_pane, attached.paneId); assert.equal(f.workspace.tokens.wc_summary, undefined);
  assert.equal(f.workspace.tokens.other, 'preserved'); assert.deepEqual(unrelated.tokens, { other: 'preserved' });
  assert.ok(f.panes.includes(f.caller)); assert.ok(f.panes.includes(unrelated));
  assert.equal(f.calls.filter(a => a.slice(0, 3).join(' ') === 'plugin pane open').length, 1);
  assert.equal(f.calls.some(a => a.slice(0, 3).join(' ') === 'plugin pane close'), false);
  const clearing = f.calls.find(a => a[1] === 'report-metadata'); assert.ok(clearing.includes('--clear-token'));
});
test('explicit detach recovers an absent owner without closing panes or unrelated tokens', async t => {
  const f = await fixture(t); await f.client.attachCard(snapshot(f.path), DEFAULT_CONFIG);
  f.panes.splice(1, 1); const unrelated = pane('w1:p9', 'term9'); unrelated.tokens.other = 'kept'; f.panes.push(unrelated);
  f.workspace.tokens.other = 'kept'; f.workspace.tokens.wc_status = 'active'; f.workspace.tokens.wc_summary = 'old'; f.calls.length = 0;
  await f.client.detachCard(snapshot(f.path), DEFAULT_CONFIG);
  assert.deepEqual(f.workspace.tokens, { other: 'kept' }); assert.deepEqual(f.panes, [f.caller, unrelated]);
  assert.equal(f.calls.some(a => a.slice(0, 3).join(' ') === 'plugin pane close'), false);
  assert.equal(f.calls.some(a => a.slice(0, 3).join(' ') === 'plugin pane open'), false);
  assert.equal(f.calls.filter(a => a[1] === 'report-metadata').length, 1);
});
test('absent-owner recovery refuses conflicting, incomplete, live or ambiguous evidence without mutations', async t => {
  const cases = {
    'different card': f => { f.workspace.tokens.wc_card = 'f'.repeat(64); },
    'missing card identity': f => { delete f.workspace.tokens.wc_card; },
    'missing nonce': f => { delete f.workspace.tokens.wc_nonce; },
    'missing pane ID': f => { delete f.workspace.tokens.wc_pane; },
    'missing terminal ID': f => { delete f.workspace.tokens.wc_terminal; },
    'malformed nonce': f => { f.workspace.tokens.wc_nonce = 'not-a-generated-nonce'; },
    'malformed pane ID': f => { f.workspace.tokens.wc_pane = 'not-a-pane'; },
    'pane in a different workspace': f => { f.workspace.tokens.wc_pane = 'w9:p2'; },
    'malformed terminal ID': f => { f.workspace.tokens.wc_terminal = ' '; },
    'restored shell at the same pane ID': f => { f.panes.push(pane(f.workspace.tokens.wc_pane, 'restored-terminal')); },
    'live terminal at another pane ID': f => { f.panes.push(pane('w1:p9', f.workspace.tokens.wc_terminal)); },
    'another marked pane': f => { const p = pane('w1:p9', 'term9'); p.tokens.wc_role = 'card'; f.panes.push(p); },
    'partial pane markers': f => { const p = pane('w1:p9', 'term9'); p.tokens.wc_nonce = f.workspace.tokens.wc_nonce; f.panes.push(p); },
    'duplicate unmarked pane IDs': f => { f.panes.push(pane('w1:p9', 'term9'), pane('w1:p9', 'term10')); },
    'duplicate unmarked terminal IDs': f => { f.panes.push(pane('w1:p9', 'term9'), pane('w1:p10', 'term9')); },
    'inventory lacks live caller': f => { f.panes.splice(0, 1); },
  };
  for (const [name, mutate] of Object.entries(cases)) await t.test(name, async sub => {
    const f = await fixture(sub); await f.client.attachCard(snapshot(f.path), DEFAULT_CONFIG); f.panes.splice(1, 1); mutate(f); f.calls.length = 0;
    const before = structuredClone({ workspace: f.workspace, panes: f.panes });
    await assert.rejects(f.client.attachCard(snapshot(f.path), DEFAULT_CONFIG), /conflict|invalid|ambiguous|inventory/i);
    await assert.rejects(f.client.detachCard(snapshot(f.path), DEFAULT_CONFIG), /conflict|invalid|ambiguous|inventory/i);
    assert.deepEqual({ workspace: f.workspace, panes: f.panes }, before);
    assert.equal(f.calls.some(a => a[1] === 'report-metadata' || a.slice(0, 3).join(' ') === 'plugin pane close' || a.slice(0, 3).join(' ') === 'plugin pane open'), false);
  });
});
test('detach of a deleted YAML still closes only its verified live owner and never recreates the file', async t => {
  const f = await fixture(t); const saved = snapshot(f.path); const attached = await f.client.attachCard(saved, DEFAULT_CONFIG);
  f.workspace.tokens.unrelated = 'kept'; await unlink(f.path); f.calls.length = 0;
  await assert.rejects(cardIdentity(f.path), { code: 'ENOENT' });
  await assert.rejects(f.client.attachCard(saved, DEFAULT_CONFIG), { code: 'ENOENT' });
  await assert.rejects(f.client.projectCard(saved, DEFAULT_CONFIG, attached), { code: 'ENOENT' });
  await f.client.detachCard(saved, DEFAULT_CONFIG);
  assert.deepEqual(f.panes, [f.caller]); assert.deepEqual(f.workspace.tokens, { unrelated: 'kept' });
  assert.deepEqual(f.calls.filter(a => a.slice(0, 3).join(' ') === 'plugin pane close'), [['plugin', 'pane', 'close', attached.paneId]]);
  await assert.rejects(realpath(f.path), { code: 'ENOENT' });
});
test('detach of a deleted YAML recovers a verified absent owner without closing any pane', async t => {
  const f = await fixture(t); const saved = snapshot(f.path); await f.client.attachCard(saved, DEFAULT_CONFIG);
  f.panes.splice(1, 1); f.workspace.tokens.unrelated = 'kept'; await unlink(f.path); f.calls.length = 0;
  await f.client.detachCard(saved, DEFAULT_CONFIG);
  assert.deepEqual(f.panes, [f.caller]); assert.deepEqual(f.workspace.tokens, { unrelated: 'kept' });
  assert.equal(f.calls.some(a => a.slice(0, 3).join(' ') === 'plugin pane close'), false);
  await assert.rejects(realpath(f.path), { code: 'ENOENT' });
});
test('deleted-card detach refuses wrong identity, nonce mismatch and restored live owners without mutation', async t => {
  const cases = {
    'wrong identity': f => { f.workspace.tokens.wc_card = 'f'.repeat(64); },
    'mismatched nonce': f => { f.panes[1].tokens.wc_nonce = 'different'; },
    'restored live owner': f => { f.panes[1].terminal_id = 'restored-terminal'; },
  };
  for (const [name, mutate] of Object.entries(cases)) await t.test(name, async sub => {
    const f = await fixture(sub); const saved = snapshot(f.path); await f.client.attachCard(saved, DEFAULT_CONFIG); mutate(f);
    await unlink(f.path); f.calls.length = 0; const before = structuredClone({ workspace: f.workspace, panes: f.panes });
    await assert.rejects(f.client.detachCard(saved, DEFAULT_CONFIG), /conflict/i);
    assert.deepEqual({ workspace: f.workspace, panes: f.panes }, before);
    assert.equal(f.calls.some(a => a[1] === 'report-metadata' || a.slice(0, 3).join(' ') === 'plugin pane close'), false);
  });
});
test('deleted YAML identity canonicalizes a symlinked containing directory for detach', async t => {
  const f = await fixture(t); const cards = join(f.root, 'cards'); await mkdir(cards);
  const alias = join(f.root, 'alias'); await symlink(cards, alias);
  const actual = join(cards, 'card.yaml'); await writeFile(actual, 'fixture'); const saved = snapshot(join(alias, 'card.yaml'));
  const expected = await cardIdentity(actual); await f.client.attachCard(saved, DEFAULT_CONFIG);
  assert.equal(f.workspace.tokens.wc_card, expected); await unlink(actual);
  await f.client.detachCard(saved, DEFAULT_CONFIG); assert.deepEqual(f.panes, [f.caller]); assert.deepEqual(f.workspace.tokens, {});
});
test('missing parent, retargeted parent alias and dangling final alias cannot prove cleanup identity', async t => {
  for (const mode of ['missing parent', 'retargeted parent alias', 'dangling final alias']) await t.test(mode, async sub => {
    const f = await fixture(sub); const cards = join(f.root, 'cards'); await mkdir(cards);
    const actual = join(cards, 'card.yaml'); await writeFile(actual, 'fixture');
    const alias = join(f.root, mode === 'dangling final alias' ? 'alias.yaml' : 'alias');
    await symlink(mode === 'dangling final alias' ? actual : cards, alias);
    const saved = snapshot(mode === 'dangling final alias' ? alias : join(alias, 'card.yaml'));
    await f.client.attachCard(saved, DEFAULT_CONFIG); await unlink(actual);
    if (mode === 'missing parent') await rm(cards, { recursive: true });
    if (mode === 'retargeted parent alias') {
      const other = join(f.root, 'other'); await mkdir(other); await unlink(alias); await symlink(other, alias);
    }
    f.calls.length = 0; const before = structuredClone({ workspace: f.workspace, panes: f.panes });
    await assert.rejects(f.client.detachCard(saved, DEFAULT_CONFIG));
    assert.deepEqual({ workspace: f.workspace, panes: f.panes }, before);
    assert.equal(f.calls.some(a => a[1] === 'report-metadata' || a.slice(0, 3).join(' ') === 'plugin pane close'), false);
  });
});
test('non-ENOENT source errors are not converted into a cleanup identity', async t => {
  const f = await fixture(t); const saved = snapshot(f.path); await f.client.attachCard(saved, DEFAULT_CONFIG);
  await unlink(f.path); await symlink(f.path, f.path); f.calls.length = 0;
  await assert.rejects(f.client.detachCard(saved, DEFAULT_CONFIG), { code: 'ELOOP' });
  assert.equal(f.panes.length, 2); assert.equal(f.calls.some(a => a[1] === 'report-metadata' || a.slice(0, 3).join(' ') === 'plugin pane close'), false);
});
test('workspace lock is exclusive, fails immediately and never steals a stale lock', async t => {
  const f = await fixture(t); const socket = join(f.root, 'herdr.sock');
  await withWorkspaceLock(socket, 'w1', async () => {
    await assert.rejects(withWorkspaceLock(socket, 'w1', async () => {}), /lock.*Inspect|Inspect.*lock/i);
  });
  await withWorkspaceLock(socket, 'w1', async () => {});
});

async function documentFixture(t, intercept) {
  const f = await fixture(t);
  await f.client.attachCard(snapshot(f.path), DEFAULT_CONFIG);
  const owner = f.panes[1];
  const env = { ...f.env, HERDR_PANE_ID: owner.pane_id, WORK_CARDS_NONCE: f.workspace.tokens.wc_nonce, EDITOR: 'nano -w' };
  const attempts = [];
  const run = async args => {
    attempts.push([...args]);
    if (intercept) { const reply = await intercept(args, f); if (reply !== undefined) return reply; }
    if (args[0] === 'pane' && args[1] === 'current') return JSON.stringify({ result: { type: 'pane_current', pane: owner } });
    return f.run(args);
  };
  const client = new HerdrClient({ env, run, pluginRoot: f.root });
  const saved = snapshot(f.path); saved.card.cwd = f.root;
  saved.card.documents = [{ path: '-notes $;\'".md', description: '' }];
  await writeFile(join(f.root, saved.card.documents[0].path), 'hello');
  f.calls.length = 0;
  return { ...f, owner, env, client, saved, attempts, relative: saved.card.documents[0].path };
}
const mutations = calls => calls.filter(a => a[1] === 'report-metadata' || a[1] === 'rename' || a[1] === 'focus' || a[1] === 'link' || a.slice(0, 3).join(' ') === 'plugin pane open');

test('document opens a new exact-workspace tab, titles basename safely, then focuses only that tab', async t => {
  const f = await documentFixture(t); const before = structuredClone(f.workspace);
  await f.client.openDocument(f.saved, f.relative, DEFAULT_CONFIG);
  const calls = mutations(f.attempts);
  assert.equal(calls.length, 4);
  const [open, stamp, rename, focus] = calls;
  assert.deepEqual(stamp.slice(0, 3), ['pane', 'report-metadata', 'w1:p3']);
  assert.deepEqual(open.slice(0, 3), ['plugin', 'pane', 'open']);
  assert.equal(open[open.indexOf('--entrypoint') + 1], 'document');
  assert.equal(open[open.indexOf('--placement') + 1], 'tab');
  assert.equal(open[open.indexOf('--workspace') + 1], 'w1');
  assert.equal(open[open.indexOf('--cwd') + 1], f.root);
  assert.ok(open.includes('--no-focus'));
  assert.ok(open.includes('WORK_CARDS_DOCUMENT_COMMAND=nano -w'));
  assert.equal(open.some(value => value.startsWith('PATH=')), false);
  assert.ok(open.includes('WORK_CARDS_DOCUMENT_CWD=' + f.root));
  assert.ok(open.includes('WORK_CARDS_DOCUMENT_PATH=' + join(f.root, f.relative)));
  assert.deepEqual(rename, ['tab', 'rename', 'w1:t3', f.relative]);
  assert.deepEqual(focus, ['tab', 'focus', 'w1:t3']);
  assert.deepEqual(f.workspace, before);
  assert.ok(f.panes.includes(f.owner));
});
test('document reuses exact stamped tab across clients and card nonces, then launches after close', async t => {
  const f = await documentFixture(t);
  await f.client.openDocument(f.saved, f.relative, DEFAULT_CONFIG);
  const editor = f.panes.at(-1);
  assert.deepEqual(Object.keys(editor.tokens).sort(), ['wcd_key', 'wcd_pane', 'wcd_tab', 'wcd_terminal']);
  f.attempts.length = 0; f.client.env.EDITOR = 'different-editor';
  await f.client.openDocument(f.saved, f.relative, DEFAULT_CONFIG);
  assert.deepEqual(mutations(f.attempts), [['tab', 'focus', editor.tab_id]]);
  await f.client.detachCard(f.saved, DEFAULT_CONFIG);
  const attached = await new HerdrClient({ env: f.env, run: f.run, pluginRoot: f.root }).attachCard(f.saved, DEFAULT_CONFIG);
  const owner = f.panes.find(p => p.pane_id === attached.paneId);
  const calls = [];
  const fresh = new HerdrClient({ env: { ...f.env, HERDR_PANE_ID: owner.pane_id, WORK_CARDS_NONCE: attached.nonce }, pluginRoot: f.root,
    run: async args => { calls.push(args); return args[0] === 'pane' && args[1] === 'current'
      ? JSON.stringify({ result: { type: 'pane_current', pane: owner } }) : f.run(args); } });
  await fresh.openDocument(f.saved, f.relative, DEFAULT_CONFIG);
  assert.deepEqual(mutations(calls), [['tab', 'focus', editor.tab_id]]);
  f.panes.splice(f.panes.indexOf(editor), 1); calls.length = 0;
  await fresh.openDocument(f.saved, f.relative, DEFAULT_CONFIG);
  assert.equal(mutations(calls)[0].slice(0, 3).join(' '), 'plugin pane open');
});

test('canonical aliases reuse, but distinct paths/checkouts and unmarked tabs launch separately', async t => {
  const f = await documentFixture(t);
  f.panes.push({ ...pane('w1:p90', 'term90'), tab_id: 'w1:t90' });
  await f.client.openDocument(f.saved, f.relative, DEFAULT_CONFIG);
  const editor = f.panes.at(-1);
  // An unmarked split in the document's tab is not a second document record.
  f.panes.push({ ...pane('w1:p91', 'term91'), tab_id: editor.tab_id });
  const alias = join(f.root, 'alias'); await symlink(f.root, alias);
  const pathAlias = 'document-alias.md'; await symlink(join(f.root, f.relative), join(f.root, pathAlias));
  f.saved.card.cwd = alias; f.saved.card.documents.push({ path: pathAlias, description: '' });
  f.attempts.length = 0;
  await f.client.openDocument(f.saved, pathAlias, DEFAULT_CONFIG);
  assert.deepEqual(mutations(f.attempts), [['tab', 'focus', editor.tab_id]]);
  await mkdir(join(f.root, 'other')); await writeFile(join(f.root, 'other', f.relative), 'other');
  f.saved.card.cwd = f.root; f.saved.card.documents.push({ path: 'other/' + f.relative, description: '' });
  f.attempts.length = 0;
  await f.client.openDocument(f.saved, 'other/' + f.relative, DEFAULT_CONFIG);
  assert.equal(mutations(f.attempts)[0].slice(0, 3).join(' '), 'plugin pane open');
  // Same canonical document but a different canonical checkout is a distinct identity.
  f.saved.card.cwd = join(f.root, 'other');
  f.attempts.length = 0;
  await f.client.openDocument(f.saved, f.relative, DEFAULT_CONFIG);
  assert.equal(mutations(f.attempts)[0].slice(0, 3).join(' '), 'plugin pane open');
});

test('document matching partial, stale and duplicate records refuse without mutation', async t => {
  for (const mode of ['partial', 'pane', 'terminal', 'tab', 'duplicate']) await t.test(mode, async sub => {
    const f = await documentFixture(sub); await f.client.openDocument(f.saved, f.relative, DEFAULT_CONFIG);
    const editor = f.panes.at(-1);
    if (mode === 'partial') delete editor.tokens.wcd_tab;
    else if (mode === 'duplicate') f.panes.push({ ...pane('w1:p9', 'term9'), tab_id: 'w1:t9', tokens: { ...editor.tokens } });
    else editor.tokens['wcd_' + mode] = 'wrong';
    f.attempts.length = 0;
    await assert.rejects(f.client.openDocument(f.saved, f.relative, DEFAULT_CONFIG), /document.*record|record.*document/i);
    assert.deepEqual(mutations(f.attempts), []);
  });
});

test('document metadata must round-trip before rename/focus; fully stamped partial launches can reuse', async t => {
  for (const mode of ['empty', 'semantic', 'throw', 'rename']) await t.test(mode, async sub => {
    let fail = true;
    const f = await documentFixture(sub, async args => {
      if (!fail) return;
      if (args[0] === 'pane' && args[1] === 'report-metadata') {
        if (mode === 'throw') throw new Error('metadata failed');
        if (mode !== 'rename') return mode === 'semantic' ? JSON.stringify({ error: 'metadata refused' }) : '';
      }
      if (mode === 'rename' && args[1] === 'rename') throw new Error('rename failed');
    });
    await assert.rejects(f.client.openDocument(f.saved, f.relative, DEFAULT_CONFIG), /w1:p3.*term3.*w1:t3/);
    assert.equal(f.attempts.some(a => a[1] === 'focus'), false);
    if (mode === 'rename') {
      fail = false; f.attempts.length = 0;
      await f.client.openDocument(f.saved, f.relative, DEFAULT_CONFIG);
      assert.deepEqual(mutations(f.attempts), [['tab', 'focus', 'w1:t3']]);
    }
  });
});

test('document validation, FIFO and escaping symlinks fail before any Herdr mutation', async t => {
  const f = await documentFixture(t);
  for (const path of ['../private', 'unlisted.md']) {
    await assert.rejects(f.client.openDocument(f.saved, path, DEFAULT_CONFIG));
    assert.deepEqual(mutations(f.attempts), []);
  }
  await assert.rejects(f.client.openDocument(f.saved, f.relative, { ...DEFAULT_CONFIG, document_command: 'bad\n' }), /command/);
  await rm(join(f.root, f.relative)); await mkdir(join(f.root, f.relative));
  await assert.rejects(f.client.openDocument(f.saved, f.relative, DEFAULT_CONFIG), /regular file/);
  await rm(join(f.root, f.relative), { recursive: true });
  const outside = join(f.root, '..', 'work-cards-outside-' + process.pid);
  await writeFile(outside, 'private'); t.after(() => rm(outside, { force: true }));
  await symlink(outside, join(f.root, f.relative));
  await assert.rejects(f.client.openDocument(f.saved, f.relative, DEFAULT_CONFIG), /outside/);
  await unlink(join(f.root, f.relative));
  assert.equal(spawnSync('mkfifo', [join(f.root, f.relative)]).status, 0);
  await assert.rejects(f.client.openDocument(f.saved, f.relative, DEFAULT_CONFIG), /regular file/);
  assert.deepEqual(mutations(f.attempts), []);
});
test('document refuses rebound, duplicate, missing, moved and wrong-nonce owners without mutations', async t => {
  for (const mode of ['rebound', 'duplicate', 'missing', 'moved', 'nonce', 'socket', 'pane env']) await t.test(mode, async sub => {
    const f = await documentFixture(sub);
    if (mode === 'rebound') f.workspace.tokens.wc_card = 'f'.repeat(64);
    if (mode === 'duplicate') f.panes.push({ ...pane('w1:p9', 'term9'), tokens: { ...f.owner.tokens } });
    if (mode === 'missing') f.panes.splice(1, 1);
    if (mode === 'moved') f.owner.workspace_id = 'w2';
    if (mode === 'nonce') f.client.env.WORK_CARDS_NONCE = 'wrong';
    if (mode === 'socket') f.client.env.HERDR_SOCKET_PATH = join(f.root, 'wrong.sock');
    if (mode === 'pane env') f.client.env.HERDR_PANE_ID = 'w1:p9';
    await assert.rejects(f.client.openDocument(f.saved, f.relative, DEFAULT_CONFIG));
    assert.deepEqual(mutations(f.attempts), []);
  });
});
test('document rejects stale plugin registrations and directs explicit relink without replacing them', async t => {
  const f = await documentFixture(t, async (args, f) => {
    if (args[0] === 'plugin' && args[1] === 'list') return JSON.stringify({ result: { type: 'plugin_list', plugins: [{ ...f.plugin, panes: f.plugin.panes.slice(0, 1) }] } });
  });
  await assert.rejects(f.client.openDocument(f.saved, f.relative, DEFAULT_CONFIG), /relink|plugin link/i);
  assert.deepEqual(mutations(f.attempts), []);
});
test('document partial failures include known new tab and never close or retry', async t => {
  for (const mode of ['open', 'rename', 'rename identity', 'focus', 'focus error']) await t.test(mode, async sub => {
    const f = await documentFixture(sub, async (args, f) => {
      if ((mode === 'open' && args.slice(0, 3).join(' ') === 'plugin pane open') ||
          (mode === 'rename' && args[1] === 'rename') || (mode === 'focus' && args[1] === 'focus')) throw new Error('injected failure');
      if (mode === 'rename identity' && args[1] === 'rename') return JSON.stringify({ result: { type: 'tab_info', tab: { tab_id: 'wrong', workspace_id: 'w1' } } });
      if (mode === 'focus error' && args[1] === 'focus') return JSON.stringify({ error: 'focus failed' });
    });
    await assert.rejects(f.client.openDocument(f.saved, f.relative, DEFAULT_CONFIG), mode === 'open' ? /inspect|partial/i : /w1:t3/);
    assert.equal(f.attempts.some(a => a[1] === 'run' || a.slice(0, 3).join(' ') === 'plugin pane close'), false);
    assert.equal(f.attempts.filter(a => a.slice(0, 3).join(' ') === 'plugin pane open').length, 1);
    const expected = [['plugin', 'pane', 'open']];
    if (mode !== 'open') expected.push(['pane', 'report-metadata'], ['tab', 'rename']);
    if (mode.startsWith('focus')) expected.push(['tab', 'focus']);
    assert.deepEqual(mutations(f.attempts).map(a => a.slice(0, a[0] === 'plugin' ? 3 : 2)), expected);
  });
});
test('document rejects returned plugin/entrypoint/workspace or any reused pane/tab/terminal before rename', async t => {
  for (const mode of ['plugin', 'entrypoint', 'workspace', 'pane', 'terminal', 'tab']) await t.test(mode, async sub => {
    const f = await documentFixture(sub, async (args, f) => {
      if (args.slice(0, 3).join(' ') !== 'plugin pane open') return;
      const p = { ...pane('w1:p9', 'term9'), tab_id: 'w1:t9' };
      if (mode === 'workspace') p.workspace_id = 'w2';
      if (mode === 'pane') p.pane_id = f.owner.pane_id;
      if (mode === 'terminal') p.terminal_id = f.owner.terminal_id;
      if (mode === 'tab') p.tab_id = f.owner.tab_id;
      return JSON.stringify({ result: { type: 'plugin_pane_opened', plugin_pane: {
        plugin_id: mode === 'plugin' ? 'other' : 'work-cards', entrypoint: mode === 'entrypoint' ? 'card' : 'document', pane: p,
      } } });
    });
    await assert.rejects(f.client.openDocument(f.saved, f.relative, DEFAULT_CONFIG), /inspect|partial/i);
    const opens = f.attempts.filter(a => a.slice(0, 3).join(' ') === 'plugin pane open');
    assert.equal(opens.length, 1);
    assert.deepEqual(mutations(f.attempts), opens);
  });
});

test('document canonicalizes home checkout aliases, applies command override and titles displayed basename', async t => {
  const f = await documentFixture(t);
  const alias = join(f.root, 'alias'); await symlink(f.root, alias);
  await mkdir(join(f.root, 'docs')); await writeFile(join(f.root, 'docs', '-label.md'), 'document');
  f.saved.card.cwd = '~/' + relative(homedir(), alias);
  f.saved.card.documents = [{ path: 'docs/-label.md', description: '' }];
  await f.client.openDocument(f.saved, 'docs/-label.md', { ...DEFAULT_CONFIG, document_command: 'less -R' });
  const [open, , rename] = mutations(f.attempts);
  assert.ok(open.includes('WORK_CARDS_DOCUMENT_CWD=' + f.root));
  assert.ok(open.includes('WORK_CARDS_DOCUMENT_PATH=' + join(f.root, 'docs', '-label.md')));
  assert.ok(open.includes('WORK_CARDS_DOCUMENT_COMMAND=less -R'));
  assert.deepEqual(rename, ['tab', 'rename', 'w1:t3', '-label.md']);
});
test('attach forwards caller EDITOR verbatim rather than server inherited environment', async t => {
  const f = await fixture(t);
  f.client.env.EDITOR = '"/editor path" -f';
  await f.client.attachCard(snapshot(f.path), DEFAULT_CONFIG);
  const open = mutations(f.calls).find(a => a.slice(0, 3).join(' ') === 'plugin pane open');
  assert.ok(open.includes('EDITOR="/editor path" -f'));
});
test('malformed nonblank EDITOR is rejected before Herdr calls, while unset uses less', async t => {
  const f = await documentFixture(t);
  f.client.env.EDITOR = 'vim\n';
  await assert.rejects(f.client.openDocument(f.saved, f.relative, DEFAULT_CONFIG), /EDITOR/);
  assert.deepEqual(f.attempts, []);
  f.client.env.EDITOR = '';
  await f.client.openDocument(f.saved, f.relative, DEFAULT_CONFIG);
  assert.ok(mutations(f.attempts)[0].includes('WORK_CARDS_DOCUMENT_COMMAND=less'));
});
test('document refuses a caller that moves between initial lookup and the locked lookup', async t => {
  let reads = 0;
  const f = await documentFixture(t, async (args, f) => {
    if (args[0] === 'pane' && args[1] === 'current' && ++reads === 2) {
      return JSON.stringify({ result: { type: 'pane_current', pane: { ...f.panes[1], tab_id: 'w1:t9' } } });
    }
  });
  await assert.rejects(f.client.openDocument(f.saved, f.relative, DEFAULT_CONFIG), /moved/);
  assert.deepEqual(mutations(f.attempts), []);
});

test('attach and document forward only explicitly defined caller PATH, not Herdr identities or entire env', async t => {
  const f = await fixture(t);
  const path = '/custom mise/bin:/caller bin:/usr/bin:/bin';
  f.client.env.PATH = path; f.client.env.EDITOR = 'custom-editor';
  f.client.env.SECRET_TEST_VALUE = 'do not forward';
  await f.client.attachCard(snapshot(f.path), DEFAULT_CONFIG);
  const cardOpen = f.calls.find(a => a.slice(0, 3).join(' ') === 'plugin pane open');
  const envValues = args => args.flatMap((value, i) => value === '--env' ? [args[i + 1]] : []);
  assert.ok(envValues(cardOpen).includes('PATH=' + path));
  assert.ok(envValues(cardOpen).includes('EDITOR=custom-editor'));
  assert.ok(envValues(cardOpen).every(value => !value.startsWith('HERDR_') && !value.startsWith('SECRET_TEST_VALUE=')));
  const doc = await documentFixture(t);
  doc.client.env.PATH = path; doc.client.env.SECRET_TEST_VALUE = 'do not forward';
  await doc.client.openDocument(doc.saved, doc.relative, DEFAULT_CONFIG);
  const documentOpen = mutations(doc.attempts)[0];
  assert.ok(envValues(documentOpen).includes('PATH=' + path));
  assert.deepEqual(envValues(documentOpen).map(value => value.split('=')[0]).sort(),
    ['PATH', 'WORK_CARDS_DOCUMENT_COMMAND', 'WORK_CARDS_DOCUMENT_CWD', 'WORK_CARDS_DOCUMENT_PATH']);
});
