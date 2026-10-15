import test from 'node:test';
import { createHash } from 'node:crypto';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, realpath, symlink, unlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { WorkspaceClient } from '../dist/herdr/workspace-client.js';
import { evaluatedGoal, serializeWorkspaceV1, agentAuthority } from '../dist/records.js';
import { withWorkspaceLock } from '../dist/herdr/workspace-lock.js';
import { setTimeout as delay } from 'node:timers/promises';
import { RecordStore } from '../dist/records.js';
import { DEFAULT_CONFIG } from '../dist/config.js';
import { resolvedReferences } from '../dist/presentation.js';

const boundary = { compactionDigest: 'c', transcriptLength: 0, prefixDigest: 'p' };
async function fixture() {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'wc-native-')));
  const cwd = join(root, 'checkout'); await mkdir(cwd); await writeFile(join(cwd, 'plan.md'), 'plan');
  const store = new RecordStore(join(root, 'state')); await mkdir(store.directory);
  const socket = join(root, 'h.sock');
  const panes = [1,2].map(n => ({ pane_id: `w:p${n}`, terminal_id: `term${n}`, workspace_id: 'w', tab_id: `w:t${n}`, tokens: {} }));
  const workspaces = [{ workspace_id: 'w', tokens: {} }];
  const tabs = [1,2].map(n => ({ tab_id: `w:t${n}`, workspace_id: 'w', label: `${n}`, number: n }));
  const calls = []; let next = 3; let fail;
  const inventory = () => ({ workspaces, tabs, panes, layouts: tabs.map(t => ({ tab_id: t.tab_id, workspace_id: t.workspace_id, panes: panes.filter(p => p.tab_id === t.tab_id).map(p => ({ pane_id: p.pane_id })) })) });
  const run = terminal => async args => {
    calls.push(args);
    if (fail?.(args)) throw new Error('injected failure');
    if (args[0] === 'status') return JSON.stringify({ server: { running: true, compatible: true, endpoint_compatible: true, version: '0.9.4', socket } });
    const result = (type, fields) => JSON.stringify({ result: { type, ...fields } });
    if (args[0] === 'api') return result('session_snapshot', { snapshot: inventory() });
    if (args[0] === 'pane' && args[1] === 'current') return result('pane_current', { pane: panes.find(p => p.terminal_id === terminal) });
    if (args[0] === 'pane' && args[1] === 'resize') return result('pane_resize', { resize: { pane_id: args[3] } });
    if (args[1] === 'report-metadata') {
      const target = (args[0] === 'pane' ? panes : workspaces).find(p => (p.pane_id ?? p.workspace_id) === args[2]);
      for (let i = 5; i < args.length; i += 2) { const [key, ...v] = args[i+1].split('='); if (args[i] === '--clear-token') delete target.tokens[key]; else target.tokens[key] = v.join('='); }
      return '';
    }
    if (args[0] === 'plugin' && args[1] === 'list') return result('plugin_list', { plugins: [] });
    if (args[0] === 'plugin' && args[1] === 'link') return result('plugin_linked', { plugin: { plugin_id: 'work-cards', enabled: true, plugin_root: root, manifest_path: join(root,'herdr-plugin.toml'), source: { kind: 'local' }, version:'0.1.0', min_herdr_version:'0.9.3', platforms:['macos','linux'], panes:[{id:'card', title:'Work card',placement:'split',command:['node','dist/herdr/pane.js']},{id:'document',title:'Document',placement:'tab',command:['/bin/sh','bin/open-document.sh']}] } });
    if (args[0] === 'plugin' && args[1] === 'pane' && args[2] === 'open') {
      const arg = name => args[args.indexOf(name)+1];
      const entrypoint = arg('--entrypoint'); const owner = panes.find(p => p.pane_id === arg('--target-pane'));
      const n = next++; const tab_id = entrypoint === 'card' ? owner.tab_id : `w:t${n}`;
      const p = { pane_id: `w:p${n}`, terminal_id: `term${n}`, tab_id, workspace_id: owner?.workspace_id ?? arg('--workspace'), tokens:{}, ...(entrypoint === 'card' ? { label: 'Work card' } : {}) };
      if (entrypoint !== 'card') tabs.push({tab_id,workspace_id:p.workspace_id,label:String(n),number:n});
      panes.push(p); return result('plugin_pane_opened', { plugin_pane: {plugin_id:'work-cards',entrypoint,pane:p} });
    }
    if (args[0] === 'plugin' && args[2] === 'close') { const i=panes.findIndex(p => p.pane_id === args[3]); panes.splice(i,1); return result('plugin_pane_closed',{pane_id:args[3]}); }
    if (args[0] === 'pane' && args[1] === 'rename') { const p = panes.find(p=>p.pane_id===args[2]); if (args.length === 4 && args[3] === '--clear') delete p.label; else p.label=args.slice(3).join(' '); return result('pane_info',{pane:p}); }
    if (args[0] === 'tab' && args[1] === 'rename') { const t = tabs.find(t=>t.tab_id===args[2]); t.label=args[3]; return result('tab_info',{tab:t}); }
    if (args[0] === 'tab' && args[1] === 'focus') return JSON.stringify({result:{}});
    throw new Error(`Unexpected call ${args.join(' ')}`);
  };
  const client = terminal => new WorkspaceClient({ env:{HERDR_ENV:'1',HERDR_PANE_ID:'stale-bootstrap-id',HERDR_SOCKET_PATH:socket},run:run(terminal),pluginRoot:root });
  const joinAgent = async (terminal, sessionId=terminal) => {
    const c=client(terminal), context=await c.bootstrap(DEFAULT_CONFIG);
    const membership=await c.joinWorkspace(store, context,{sessionId,cwd,title:sessionId,boundary},DEFAULT_CONFIG);
    return {c,context,...membership};
  };
  return {root,cwd,store,panes,workspaces,tabs,calls,client,joinAgent,setFail:f=>{fail=f;},run};
}
const resizes = calls => calls.filter(args => args[0] === 'pane' && args[1] === 'resize');

test('new display applies the configured initial fraction once without focus; reuse preserves geometry', async t => {
  for (const fraction of [DEFAULT_CONFIG.pane_height_fraction, 0.75, 0.5]) await t.test(String(fraction), async () => {
    const f = await fixture(), a = await f.joinAgent('term1');
    const config = { ...DEFAULT_CONFIG, pane_height_fraction: fraction };
    const opened = await a.c.openDisplay(f.store, a.context, a.agent, 'automatic', config);
    assert.equal(opened.kind, 'paired');
    const target = fraction < 0.5 ? opened.pair.agent : opened.pair.card;
    const expected = fraction === 0.5 ? [] : [['pane', 'resize', '--pane', target.pane_id, '--direction', fraction < 0.5 ? 'down' : 'up', '--amount', String(Math.abs(0.5 - fraction))]];
    assert.deepEqual(resizes(f.calls), expected);
    const reused = await a.c.openDisplay(f.store, a.context, opened.agent, 'explicit', config);
    assert.equal(reused.pair.nonce, opened.pair.nonce);
    assert.deepEqual(resizes(f.calls), expected);
    assert.equal(f.calls.some(args => args.includes('focus')), false);
    const launch = f.calls.find(args => args[0] === 'plugin' && args[2] === 'open');
    assert.ok(launch.includes('--no-focus'));
  });
});

test('nested initial sizing changes only the shared agent/card separator, not unrelated geometry', async t => {
  for (const fraction of [0.2, 0.75]) await t.test(String(fraction), async () => {
    const f = await fixture(), a = await f.joinAgent('term1');
    const owner = f.panes[0], unrelated = f.panes[1]; unrelated.tab_id = owner.tab_id;
    // A downward outer split has the unrelated pane above the new downward agent/card split.
    const layout = { outer: 0.5, inner: 0.5 };
    const unrelatedRect = () => ({ x: 0, y: 0, width: 1000, height: layout.outer * 1000 });
    const before = unrelatedRect(), run = a.c.run;
    a.c.run = async args => {
      if (args[0] === 'pane' && args[1] === 'resize') {
        const target = f.panes.find(p => p.pane_id === args[3]);
        const direction = args[args.indexOf('--direction') + 1], amount = Number(args[args.indexOf('--amount') + 1]);
        // Herdr v0.9.5 searches the requested edge before considering the opposite edge.
        // Agent/up therefore selects the outer separator; card/up selects the inner separator.
        if (target === owner && direction === 'up') layout.outer -= amount;
        else if (target === owner && direction === 'down') layout.inner += amount;
        else if (target?.tokens.wca_role === 'card' && direction === 'up') layout.inner -= amount;
        else assert.fail('Unexpected nested resize target/direction');
      }
      return run(args);
    };
    const opened = await a.c.openDisplay(f.store, a.context, a.agent, 'automatic', { ...DEFAULT_CONFIG, pane_height_fraction: fraction });
    assert.equal(opened.kind, 'paired');
    assert.equal(resizes(f.calls).length, 1);
    assert.equal(layout.outer, 0.5);
    assert.deepEqual(unrelatedRect(), before);
    assert.equal(layout.inner, fraction === 0.2 ? 0.8 : 0.25);
  });
});

test('q absence and automatic suppression do not resize; explicit reopen sizes only the new pair', async () => {
  const f = await fixture(), a = await f.joinAgent('term1');
  const first = await a.c.openDisplay(f.store, a.context, a.agent, 'automatic', DEFAULT_CONFIG);
  assert.equal(resizes(f.calls).length, 1);
  f.panes.splice(f.panes.findIndex(p => p.terminal_id === first.pair.card.terminal_id), 1);
  assert.equal((await a.c.openDisplay(f.store, a.context, first.agent, 'automatic', DEFAULT_CONFIG)).kind, 'suppressed');
  assert.equal(resizes(f.calls).length, 1);
  const reopened = await a.c.openDisplay(f.store, a.context, first.agent, 'explicit', DEFAULT_CONFIG);
  assert.equal(reopened.kind, 'paired'); assert.notEqual(reopened.pair.nonce, first.pair.nonce);
  assert.equal(resizes(f.calls).length, 2);
  assert.equal((await a.c.openDisplay(f.store, a.context, reopened.agent, 'automatic', DEFAULT_CONFIG)).kind, 'paired');
  assert.equal(resizes(f.calls).length, 2);
});

test('late invalidation after pair round-trip prevents resizing and propagates', async t => {
  for (const change of ['move', 'pane', 'card', 'nonce', 'membership', 'binding', 'attempt', 'abort']) await t.test(change, async () => {
    const f = await fixture(), a = await f.joinAgent('term1'), controller = new AbortController();
    const ownership = a.c.ownership.bind(a.c); let changed = false;
    a.c.ownership = async (...args) => {
      const pair = await ownership(...args);
      if (pair.kind === 'paired' && !changed) {
        changed = true;
        if (change === 'move') f.panes[0].tab_id = 'w:t2';
        if (change === 'pane') f.panes[0].pane_id = 'w:p99';
        if (change === 'card') f.panes.find(p => p.tokens.wca_role === 'card').pane_id = 'w:p99';
        if (change === 'nonce') for (const p of f.panes.filter(p => p.tokens.wca_role)) p.tokens.wca_nonce = 'other';
        if (change === 'binding') f.workspaces[0].tokens.wcw_nonce = 'other';
        if (change === 'abort') controller.abort(new Error('cancelled before resize'));
        if (change === 'membership' || change === 'attempt') {
          const current = await f.store.readAgent('term1');
          if (change === 'membership') await f.store.resetEvidence(agentAuthority(current.record), boundary, current.revision);
          else await f.store.setDisplayAttempt(agentAuthority(current.record), { attempt: 'other', nonce: 'other' }, current.revision);
        }
      }
      return pair;
    };
    await assert.rejects(a.c.openDisplay(f.store, a.context, a.agent, 'automatic', DEFAULT_CONFIG, controller.signal), /stale|changed|cancelled/i);
    assert.equal(changed, true); assert.deepEqual(resizes(f.calls), []);
    assert.ok((await f.store.readAgent('term1')).record.displayAttempt);
    assert.equal(f.calls.some(args => args[0] === 'plugin' && args[2] === 'close'), false);
  });
});

test('resize failure or malformed acknowledgement retains the pair and attempt without retrying', async t => {
  for (const fraction of [0.2, 0.75]) for (const failure of ['command', 'json', 'type', 'missing', 'pane']) await t.test(`${fraction}: ${failure}`, async () => {
    const f = await fixture(), a = await f.joinAgent('term1'), run = a.c.run;
    a.c.run = async args => {
      const response = await run(args);
      if (args[0] !== 'pane' || args[1] !== 'resize') return response;
      if (failure === 'command') throw new Error('geometry unavailable');
      if (failure === 'json') return '{';
      if (failure === 'type') return JSON.stringify({ result: { type: 'wrong', resize: { pane_id: args[3] } } });
      if (failure === 'missing') return JSON.stringify({ result: { type: 'pane_resize' } });
      return JSON.stringify({ result: { type: 'pane_resize', resize: { pane_id: 'foreign' } } });
    };
    const config = { ...DEFAULT_CONFIG, pane_height_fraction: fraction };
    const opened = await a.c.openDisplay(f.store, a.context, a.agent, 'automatic', config);
    assert.equal(opened.kind, 'paired'); assert.equal(resizes(f.calls).length, 1);
    const target = fraction < 0.5 ? opened.pair.agent : opened.pair.card;
    assert.equal(resizes(f.calls)[0][3], target.pane_id);
    const persisted = await f.store.readAgent('term1');
    assert.deepEqual(persisted.record.displayAttempt, opened.agent.record.displayAttempt);
    assert.equal((await a.c.ownership(f.store, a.context, persisted, DEFAULT_CONFIG)).kind, 'paired');
    const retried = await a.c.openDisplay(f.store, a.context, persisted, 'automatic', config);
    assert.equal(retried.pair.nonce, opened.pair.nonce);
    assert.equal(resizes(f.calls).length, 1);
    assert.equal(f.calls.filter(args => args[0] === 'plugin' && args[2] === 'open').length, 1);
    assert.equal(f.calls.some(args => args[0] === 'plugin' && args[2] === 'close'), false);
  });
});

test('cancellation during a failed resize is not swallowed as a geometry failure', async () => {
  const f = await fixture(), a = await f.joinAgent('term1'), controller = new AbortController(), run = a.c.run;
  a.c.run = async args => {
    const response = await run(args);
    if (args[0] === 'pane' && args[1] === 'resize') { controller.abort(new Error('cancelled during resize')); throw new Error('geometry failed'); }
    return response;
  };
  await assert.rejects(a.c.openDisplay(f.store, a.context, a.agent, 'automatic', DEFAULT_CONFIG, controller.signal), /cancelled during resize/);
  const current = await f.store.readAgent('term1');
  assert.equal((await a.c.ownership(f.store, a.context, current, DEFAULT_CONFIG)).kind, 'paired');
  assert.equal((await a.c.openDisplay(f.store, a.context, current, 'automatic', DEFAULT_CONFIG)).kind, 'paired');
  assert.equal(resizes(f.calls).length, 1);
});

test('selected workspace rechecks cross-group bindings after record I/O', async () => {
  const f = await fixture();
  await f.store.createWorkspace({ id: 'selected', creationToken: 'token', title: 'Selected' });
  f.workspaces.push({ workspace_id: 'other', tokens: {} });
  const c = f.client('term1'), context = await c.bootstrap(DEFAULT_CONFIG);
  const read = f.store.readWorkspace.bind(f.store); let once = true;
  f.store.readWorkspace = async id => {
    const result = await read(id);
    if (once) { once = false; f.workspaces[1].tokens = { wcw_id: 'selected' }; }
    return result;
  };
  await assert.rejects(c.joinWorkspace(f.store, context, { sessionId: 'term1', cwd: f.cwd, title: 'Task', boundary, targetWorkspaceId: 'selected' }, DEFAULT_CONFIG), /another live native binding/);
  assert.deepEqual(f.workspaces[0].tokens, {});
});

test('concurrent selections serialize the same durable target across native groups and store aliases', async () => {
  const f = await fixture();
  await f.store.createWorkspace({ id: 'selected', creationToken: 'token', title: 'Selected' });
  f.workspaces.push({ workspace_id: 'other', tokens: {} });
  f.tabs[1].workspace_id = 'other'; f.panes[1].workspace_id = 'other';
  await symlink(f.store.directory, join(f.root, 'alias'));
  const stores = [f.store, new RecordStore(join(f.root, 'alias'))];
  let release, entered;
  const held = new Promise(r => { release = r; }), ready = new Promise(r => { entered = r; });
  const read = f.store.readWorkspace.bind(f.store); let once = true;
  f.store.readWorkspace = async id => { const result = await read(id); if (once) { once = false; entered(); await held; } return result; };
  const clients = [f.client('term1'), f.client('term2')];
  const contexts = await Promise.all(clients.map(c => c.bootstrap(DEFAULT_CONFIG)));
  const select = n => clients[n].joinWorkspace(stores[n], contexts[n], { sessionId: `term${n + 1}`, cwd: f.cwd, title: 'Task', boundary, targetWorkspaceId: 'selected' }, DEFAULT_CONFIG);
  const first = select(0); await ready;
  const second = select(1); const result = Promise.allSettled([first, second]);
  await delay(80); release();
  const outcomes = await result;
  assert.equal(outcomes.filter(r => r.status === 'fulfilled').length, 1);
  assert.equal(f.workspaces.filter(w => w.tokens.wcw_id === 'selected').length, 1);
});

test('display preparation revalidates authority, attempt and ready binding before every later effect', async () => {
  for (const change of ['evidence','attempt','binding','status']) {
    const f=await fixture(), a=await f.joinAgent('term1'); const base=f.run('term1');
    const client=new WorkspaceClient({env:a.c.env,pluginRoot:f.root,run:async args=>{
      const output=await base(args);
      if(args[0]==='plugin'&&args[1]==='list') {
        const current=await f.store.readAgent('term1'), r=current.record;
        const auth={sessionId:r.sessionId,workspaceId:r.workspaceId,membershipToken:r.membershipToken,evidenceGeneration:r.evidenceGeneration};
        if(change==='evidence') await f.store.resetEvidence(auth,boundary,current.revision);
        if(change==='attempt') await f.store.setDisplayAttempt(auth,{attempt:'other',nonce:'other'},current.revision);
        if(change==='binding') f.workspaces[0].tokens.wcw_phase='reserved';
        if(change==='status') await f.store.updateOwn(auth,{now:'concurrent valid update'},current.revision);
      }
      return output;
    }});
    const operation=client.openDisplay(f.store,a.context,a.agent,'automatic',DEFAULT_CONFIG);
    if(change==='status') assert.equal((await operation).kind,'paired');
    else {
      await assert.rejects(operation,/stale|attempt|binding/i);
      assert.equal(f.calls.some(args=>args[0]==='plugin'&&(args[1]==='link'||args[2]==='open')),false);
      assert.equal(f.panes.some(p=>p.tokens.wca_role),false);
    }
  }
});
test('authority invalidation after link, launch or card stamp blocks every remaining display effect', async () => {
  for (const stage of ['link','open','card-stamp']) {
    const f=await fixture(), a=await f.joinAgent('term1'), base=f.run('term1'); let invalidatedAt;
    const client=new WorkspaceClient({env:a.c.env,pluginRoot:f.root,run:async args=>{
      const output=await base(args);
      const matches=stage==='link'?args[0]==='plugin'&&args[1]==='link':stage==='open'?args[0]==='plugin'&&args[2]==='open':args[1]==='report-metadata'&&args.includes('wca_role=card');
      if(matches) {
        const current=await f.store.readAgent('term1'), r=current.record;
        await f.store.resetEvidence({sessionId:r.sessionId,workspaceId:r.workspaceId,membershipToken:r.membershipToken,evidenceGeneration:r.evidenceGeneration},boundary,current.revision);
        invalidatedAt=f.calls.length;
      }
      return output;
    }});
    await assert.rejects(client.openDisplay(f.store,a.context,a.agent,'automatic',DEFAULT_CONFIG),/partial.*stale/i);
    assert.ok(invalidatedAt);
    assert.equal(f.calls.slice(invalidatedAt).some(args=>args[1]==='report-metadata'||args[0]==='plugin'&&(args[1]==='link'||args[2]==='open')),false);
    assert.equal(f.panes[0].tokens.wca_role,undefined);
    assert.ok((await f.store.readAgent('term1')).record.displayAttempt);
  }
});
async function documentFixture() {
  const f=await fixture(), a=await f.joinAgent('term1'), r=a.agent.record;
  const auth={sessionId:r.sessionId,workspaceId:r.workspaceId,membershipToken:r.membershipToken,evidenceGeneration:r.evidenceGeneration};
  const workspace=await f.store.addReferences(auth,{documents:[{checkout:r.checkout,path:'plan.md',description:''},{checkout:r.checkout,path:'alias.md',description:''},{checkout:r.checkout,path:'second.md',description:''}]});
  await writeFile(join(f.cwd,'second.md'),'second'); await symlink('plan.md',join(f.cwd,'alias.md'));
  const opened=await a.c.openDisplay(f.store,a.context,a.agent,'automatic',DEFAULT_CONFIG), card=f.client(opened.pair.card.terminal_id);
  card.env.WORK_CARDS_NONCE=opened.pair.nonce; const context=await card.bootstrap(DEFAULT_CONFIG);
  return {...f,a,workspace,opened,card,context,target:path=>({checkout:r.checkout,path})};
}
async function heldLock(f, operation) {
  let release, acquired;
  const ready = new Promise(resolve => { acquired = resolve; });
  const gate = new Promise(resolve => { release = resolve; });
  const holder = withWorkspaceLock(f.context.socket, f.context.workspaceId, async () => { acquired(); await gate; });
  await ready;
  try { return await operation(async () => { release(); await holder; }); }
  finally { release(); await holder; }
}
const effects = calls => calls.filter(a => a[0] === 'pane' && a[1] === 'rename' || a[1] === 'report-metadata' || a[0] === 'plugin' && (a[1] === 'link' || a[2] === 'open') || a[0] === 'tab');

test('unchanged verified projections bypass a held lock and publish nothing, including absent empty tokens', async () => {
  for (const empty of [false, true]) {
    const f = await documentFixture(), agent = f.panes[0];
    f.panes.find(p => p.terminal_id === f.context.terminalId).label = f.workspace.record.title;
    if (!empty) {
      const a = await f.store.readAgent('term1');
      await f.store.updateOwn(agentAuthority(a.record), { now: 'Current work' }, a.revision);
      const workspace = await f.store.readWorkspace(a.record.workspaceId);
      await f.store.applyPlan(agentAuthority(a.record), { goal: evaluatedGoal('Current goal', [f.target('plan.md')], 'goal') }, workspace.revision);
      f.workspaces[0].tokens.wcw_goal = 'Current goal';
    }
    agent.tokens.wca_status = 'active';
    if (!empty) agent.tokens.wca_now = 'Current work';
    const before = f.calls.length;
    await heldLock(f, () => f.a.c.projectSummaries(f.store, f.a.context, f.opened.agent, DEFAULT_CONFIG));
    assert.deepEqual(effects(f.calls.slice(before)), []);
    assert.equal(f.workspaces[0].tokens.wcw_phase, 'ready');
  }
});

test('projection rechecks records under lock and publishes only changed owned fields', async () => {
  const f = await documentFixture(); f.panes[0].tokens.wca_status = 'active';
  f.panes.find(p => p.terminal_id === f.context.terminalId).label = f.workspace.record.title;
  const a = await f.store.readAgent('term1');
  await f.store.updateOwn(agentAuthority(a.record), { now: 'Preflight work' }, a.revision);
  const paired = f.a.c.verifyMembership.bind(f.a.c); let reads = 0;
  f.a.c.verifyMembership = async (...args) => {
    const result = await paired(...args);
    if (++reads === 1) {
      const current = await f.store.readAgent('term1');
      await f.store.updateOwn(agentAuthority(current.record), { now: 'Latest work' }, current.revision);
    }
    return result;
  };
  const before = f.calls.length;
  await f.a.c.projectSummaries(f.store, f.a.context, f.opened.agent, DEFAULT_CONFIG);
  const writes = effects(f.calls.slice(before));
  assert.equal(f.panes[0].tokens.wca_now, 'Latest work');
  assert.equal(writes.length, 1);
  assert.deepEqual(writes[0].slice(5), ['--token', 'wca_now=Latest work']);
  assert.equal(f.workspaces[0].tokens.wcw_phase, 'ready');
});

test('Goal-only changes preserve pair, lifecycle and unrelated native metadata', async () => {
  const f = await documentFixture(); f.panes[0].tokens.wca_status = 'active';
  f.panes.find(p => p.terminal_id === f.context.terminalId).label = f.workspace.record.title;
  f.panes[0].tokens.unrelated = 'agent'; f.workspaces[0].tokens.unrelated = 'workspace';
  const workspace = await f.store.readWorkspace(f.workspace.record.id);
  await f.store.applyPlan(agentAuthority(f.opened.agent.record), { goal: evaluatedGoal('New goal', [f.target('plan.md')], 'goal') }, workspace.revision);
  const before = f.calls.length, previous = structuredClone(f.panes[0]);
  await f.a.c.projectSummaries(f.store, f.a.context, f.opened.agent, DEFAULT_CONFIG);
  const writes = effects(f.calls.slice(before));
  assert.equal(writes.length, 1); assert.equal(writes[0][0], 'workspace');
  assert.deepEqual(writes[0].slice(5), ['--token', 'wcw_goal=New goal']);
  assert.deepEqual(f.panes[0], previous);
  assert.equal(f.workspaces[0].tokens.wcw_phase, 'ready');
  assert.equal(f.workspaces[0].tokens.unrelated, 'workspace');
});

test('card captions project the shared title independently without renaming other surfaces', async () => {
  const f = await fixture(), a = await f.joinAgent('term1'), b = await f.joinAgent('term2');
  const displays = [];
  for (const owner of [a, b]) {
    const opened = await owner.c.openDisplay(f.store, owner.context, owner.agent, 'automatic', DEFAULT_CONFIG);
    const card = f.client(opened.pair.card.terminal_id); card.env.WORK_CARDS_NONCE = opened.pair.nonce;
    displays.push({ opened, card, context: await card.bootstrap(DEFAULT_CONFIG) });
  }
  const others = structuredClone({ panes: f.panes.slice(0, 2), tabs: f.tabs });
  for (const d of displays) await d.card.projectCaption(f.store, d.context, d.opened.agent, DEFAULT_CONFIG);
  const cards = displays.map(d => f.panes.find(p => p.terminal_id === d.context.terminalId));
  assert.deepEqual(cards.map(p => p.label), [a.workspace.record.title, a.workspace.record.title]);
  const current = await f.store.readWorkspace(a.workspace.record.id);
  await f.store.setWorkspaceTitle(agentAuthority(a.agent.record), 'Changed shared title', current.revision);
  const [first, second] = displays;
  await first.card.projectCaption(f.store, first.context, first.opened.agent, DEFAULT_CONFIG);
  assert.deepEqual(cards.map(p => p.label), ['Changed shared title', a.workspace.record.title]);
  await second.card.projectCaption(f.store, second.context, second.opened.agent, DEFAULT_CONFIG);
  assert.deepEqual(cards.map(p => p.label), ['Changed shared title', 'Changed shared title']);
  for (let i = 0; i < 2; i++) assert.deepEqual({ ...f.panes[i], tokens: others.panes[i].tokens }, others.panes[i]);
  assert.deepEqual(f.tabs, others.tabs);
  assert.equal(f.calls.some(args => args[0] === 'workspace' && args[1] === 'rename' || args.includes('focus') || args[2] === 'close'), false);
});

test('caption rereads a title changed after preflight and immediately before caption', async () => {
  const f = await documentFixture(), paired = f.card.pairedCard.bind(f.card); let reads = 0;
  f.card.pairedCard = async (...args) => {
    if (++reads === 2 || reads === 3) {
      const current = await f.store.readWorkspace(f.workspace.record.id);
      await f.store.setWorkspaceTitle(agentAuthority(f.opened.agent.record), reads === 2 ? 'Under lock' : 'Before caption', current.revision);
    }
    return paired(...args);
  };
  await f.card.projectCaption(f.store, f.context, f.opened.agent, DEFAULT_CONFIG);
  assert.equal(f.panes.find(p => p.terminal_id === f.context.terminalId).label, 'Before caption');
  assert.deepEqual(f.calls.filter(a => a[0] === 'pane' && a[1] === 'rename').map(a => a[3]), ['Before caption']);
});

test('caption uses one literal argv value with normalized Unicode and no invented length cap', async () => {
  const f = await documentFixture(), current = await f.store.readWorkspace(f.workspace.record.id);
  const title = '  --clear; $(not-a-command) café 🐕 ' + 'x'.repeat(300) + '  ';
  await f.store.setWorkspaceTitle(agentAuthority(f.opened.agent.record), title, current.revision);
  await f.card.projectCaption(f.store, f.context, f.opened.agent, DEFAULT_CONFIG);
  const expected = title.trim().toWellFormed();
  assert.deepEqual(f.calls.filter(a => a[0] === 'pane' && a[1] === 'rename'), [['pane', 'rename', f.opened.pair.card.pane_id, expected]]);
  const before = f.calls.length;
  await heldLock(f, () => f.card.projectCaption(f.store, f.context, f.opened.agent, DEFAULT_CONFIG));
  assert.deepEqual(effects(f.calls.slice(before)), []);
});

test('leading-dash Unicode caption is passed literally without a delimiter', async () => {
  const f = await documentFixture(), current = await f.store.readWorkspace(f.workspace.record.id);
  const title = '- Δ workspace 界';
  await f.store.setWorkspaceTitle(agentAuthority(f.opened.agent.record), title, current.revision);
  await f.card.projectCaption(f.store, f.context, f.opened.agent, DEFAULT_CONFIG);
  assert.deepEqual(f.calls.filter(a => a[0] === 'pane' && a[1] === 'rename'), [['pane', 'rename', f.opened.pair.card.pane_id, title]]);
  assert.equal(f.panes.find(p => p.terminal_id === f.context.terminalId).label, title);
});

test('reserved --clear captions reject visibly without renaming or changing records', async t => {
  for (const title of ['--clear', '  --clear  ']) await t.test(JSON.stringify(title), async () => {
    const f = await documentFixture(), current = await f.store.readWorkspace(f.workspace.record.id);
    await f.store.setWorkspaceTitle(agentAuthority(f.opened.agent.record), title, current.revision);
    const records = [await f.store.readAgent('term1'), await f.store.readWorkspace(f.workspace.record.id)];
    const card = f.panes.find(p => p.terminal_id === f.context.terminalId), label = card.label, before = f.calls.length;
    await assert.rejects(f.card.projectCaption(f.store, f.context, f.opened.agent, DEFAULT_CONFIG), /Projection may be partial.*Unsupported native CLI caption.*--clear/);
    assert.deepEqual(f.calls.slice(before).filter(a => a[0] === 'pane' && a[1] === 'rename'), []);
    assert.equal(card.label, label);
    assert.deepEqual([await f.store.readAgent('term1'), await f.store.readWorkspace(f.workspace.record.id)], records);
  });
});

test('already-matching reserved caption stays a no-op', async () => {
  const f = await documentFixture(), current = await f.store.readWorkspace(f.workspace.record.id);
  await f.store.setWorkspaceTitle(agentAuthority(f.opened.agent.record), '  --clear  ', current.revision);
  f.panes.find(p => p.terminal_id === f.context.terminalId).label = '--clear';
  f.panes[0].tokens.wca_status = 'active';
  const records = [await f.store.readAgent('term1'), await f.store.readWorkspace(f.workspace.record.id)], before = f.calls.length;
  await heldLock(f, () => f.card.projectCaption(f.store, f.context, f.opened.agent, DEFAULT_CONFIG));
  assert.deepEqual(effects(f.calls.slice(before)), []);
  assert.equal(f.panes.find(p => p.terminal_id === f.context.terminalId).label, '--clear');
  assert.deepEqual([await f.store.readAgent('term1'), await f.store.readWorkspace(f.workspace.record.id)], records);
});

test('fresh authority loss before caption prevents rename without publishing metadata', async t => {
  for (const invalid of ['binding', 'pair', 'nonce', 'membership', 'attempt', 'cancel']) await t.test(invalid, async () => {
    const f = await documentFixture(), controller = new AbortController(), paired = f.card.pairedCard.bind(f.card); let reads = 0;
    f.card.pairedCard = async (...args) => {
      const response = await paired(...args);
      if (++reads === 2) {
        if (invalid === 'binding') f.workspaces[0].tokens.wcw_phase = 'reserved';
        if (invalid === 'pair') f.panes[0].tab_id = 'w:t2';
        if (invalid === 'nonce') f.card.env.WORK_CARDS_NONCE = 'foreign';
        if (invalid === 'cancel') controller.abort();
        const agent = await f.store.readAgent('term1');
        if (invalid === 'membership') await f.store.resetEvidence(agentAuthority(agent.record), boundary, agent.revision);
        if (invalid === 'attempt') await f.store.setDisplayAttempt(agentAuthority(agent.record), { attempt: 'other', nonce: 'other' }, agent.revision);
      }
      return response;
    };
    await assert.rejects(f.card.projectCaption(f.store, f.context, f.opened.agent, DEFAULT_CONFIG, controller.signal), /Projection may be partial/);
    assert.equal(f.calls.some(a => a[0] === 'pane' && a[1] === 'rename'), false);
    assert.equal(f.panes[0].tokens.wca_status, undefined);
    assert.equal((await f.store.readWorkspace(f.workspace.record.id)).record.title, f.workspace.record.title);
  });
});

test('caption errors and mismatched acknowledgements preserve records, metadata and other panes', async t => {
  for (const failure of ['command', 'json', 'type', 'missing', 'pane_id', 'terminal_id', 'workspace_id', 'tab_id', 'label', 'label-type']) await t.test(failure, async () => {
    const f = await documentFixture(), run = f.card.run;
    const workspace = await f.store.readWorkspace(f.workspace.record.id);
    await f.store.applyPlan(agentAuthority(f.opened.agent.record), { goal: evaluatedGoal('Published goal', [f.target('plan.md')], 'goal') }, workspace.revision);
    const records = [await f.store.readAgent('term1'), await f.store.readWorkspace(f.workspace.record.id)];
    const others = structuredClone(f.panes.filter(p => p.terminal_id !== f.context.terminalId));
    f.card.run = async args => {
      if (args[0] !== 'pane' || args[1] !== 'rename') return run(args);
      if (failure === 'command') throw new Error('rename unavailable');
      if (failure === 'json') return '{';
      if (failure === 'type') return JSON.stringify({ result: { type: 'other' } });
      if (failure === 'missing') return JSON.stringify({ result: { type: 'pane_info' } });
      const pane = { ...f.opened.pair.card, label: records[1].record.title };
      pane[failure === 'label-type' ? 'label' : failure] = failure === 'label-type' ? 7 : 'foreign';
      return JSON.stringify({ result: { type: 'pane_info', pane } });
    };
    await assert.rejects(f.card.projectCaption(f.store, f.context, f.opened.agent, DEFAULT_CONFIG), /Projection may be partial/);
    assert.deepEqual([await f.store.readAgent('term1'), await f.store.readWorkspace(f.workspace.record.id)], records);
    assert.equal(f.panes[0].tokens.wca_status, undefined);
    assert.equal(f.workspaces[0].tokens.wcw_goal, undefined);
    assert.deepEqual(f.panes.filter(p => p.terminal_id !== f.context.terminalId).map(p => ({ ...p, tokens: others.find(o => o.terminal_id === p.terminal_id).tokens })), others);
    f.card.run = run;
    await f.card.projectCaption(f.store, f.context, f.opened.agent, DEFAULT_CONFIG);
    assert.equal(f.panes.find(p => p.terminal_id === f.context.terminalId).label, records[1].record.title);
  });
});

test('authority lost after projection preflight blocks all writes under the lock', async () => {
  const f = await documentFixture(), paired = f.card.pairedCard.bind(f.card); let reads = 0;
  f.card.pairedCard = async (...args) => {
    const result = await paired(...args);
    if (++reads === 1) f.workspaces[0].tokens.wcw_phase = 'reserved';
    return result;
  };
  const before = f.calls.length;
  await assert.rejects(f.card.projectCaption(f.store, f.context, f.opened.agent, DEFAULT_CONFIG), /binding/i);
  assert.deepEqual(effects(f.calls.slice(before)), []);
});

test('complete readonly display verification rejects stale bindings, separated pairs and membership changes without effects', async () => {
  for (const invalid of ['binding', 'pair', 'membership', 'nonce', 'cancel']) {
    const f = await documentFixture(), controller = new AbortController();
    if (invalid === 'binding') f.workspaces[0].tokens.wcw_phase = 'reserved';
    if (invalid === 'pair') f.panes[0].tab_id = 'w:t2';
    if (invalid === 'membership') {
      const { stringify } = await import('yaml');
      await writeFile(f.opened.agent.path, stringify({ ...f.opened.agent.record, membershipToken: 'changed' }));
    }
    if (invalid === 'nonce') f.card.env.WORK_CARDS_NONCE = 'foreign';
    if (invalid === 'cancel') controller.abort();
    const before = f.calls.length;
    await assert.rejects(f.card.verifyDisplay(f.store, f.context, f.opened.agent, DEFAULT_CONFIG, controller.signal));
    assert.deepEqual(effects(f.calls.slice(before)), []);
  }
});

test('document waits for the initial holder, then opens exactly once', async () => {
  const f = await documentFixture(); const before = f.calls.length;
  await heldLock(f, async release => {
    await Promise.all([
      f.card.openDocument(f.store, f.context, f.workspace, f.opened.agent, f.target('plan.md'), DEFAULT_CONFIG),
      delay(30).then(async () => { assert.deepEqual(effects(f.calls.slice(before)), []); await release(); })
    ]);
  });
  const calls = f.calls.slice(before);
  assert.equal(calls.filter(a => a[0] === 'plugin' && a[2] === 'open').length, 1);
  assert.equal(calls.filter(a => a[0] === 'tab' && a[1] === 'focus').length, 1);
});

for (const invalid of ['timeout', 'abort', 'move', 'membership', 'binding']) {
  test(`document waiting rejects ${invalid} without launch, focus or stamp`, async () => {
    const f = await documentFixture(), controller = new AbortController(), before = f.calls.length;
    await heldLock(f, async release => {
      const config = { ...DEFAULT_CONFIG, herdr_timeout_ms: invalid === 'timeout' ? 30 : 1000 };
      const rejection = assert.rejects(f.card.openDocument(f.store, f.context, f.workspace, f.opened.agent, f.target('plan.md'), config, controller.signal), /timed out|abort|moved|stale|membership|binding/i);
      if (invalid === 'timeout') { await rejection; return; }
      await Promise.all([rejection, delay(30).then(async () => {
        assert.deepEqual(effects(f.calls.slice(before)), []);
        if (invalid === 'abort') controller.abort();
        if (invalid === 'move') f.panes.find(p => p.terminal_id === f.context.terminalId).tab_id = 'w:t2';
        if (invalid === 'binding') f.workspaces[0].tokens.wcw_phase = 'reserved';
        if (invalid === 'membership') {
          const { stringify } = await import('yaml');
          await writeFile(f.opened.agent.path, stringify({ ...f.opened.agent.record, membershipToken: 'changed' }));
        }
        await release();
      })]);
    });
    assert.deepEqual(effects(f.calls.slice(before)), []);
  });
}

test('document failure after acquiring never retries a possibly partial launch', async () => {
  const f = await documentFixture(), before = f.calls.length;
  f.setFail(a => a[0] === 'plugin' && a[2] === 'open');
  await heldLock(f, async release => {
    await Promise.all([
      assert.rejects(f.card.openDocument(f.store, f.context, f.workspace, f.opened.agent, f.target('plan.md'), DEFAULT_CONFIG), /partial/),
      delay(30).then(release)
    ]);
  });
  assert.equal(f.calls.slice(before).filter(a => a[0] === 'plugin' && a[2] === 'open').length, 1);
});

test('reuse rereads native identity after the final awaited agent identity lookup', async () => {
  const f=await documentFixture(); await f.card.openDocument(f.store,f.context,f.workspace,f.opened.agent,f.target('plan.md'),DEFAULT_CONFIG);
  const identity=f.card.identity.bind(f.card); let count=0;
  f.card.identity=async(...args)=>{
    const result=await identity(...args);
    if(++count===2) {
      f.workspaces.push({workspace_id:'dest',tokens:{}}); f.tabs.push({workspace_id:'dest',tab_id:'dest:t1',label:'1',number:1});
      Object.assign(f.panes.find(p=>p.terminal_id===f.context.terminalId),{workspace_id:'dest',tab_id:'dest:t1',pane_id:'dest:p1'});
    }
    return result;
  };
  const before=f.calls.length;
  await assert.rejects(f.card.openDocument(f.store,f.context,f.workspace,f.opened.agent,f.target('plan.md'),DEFAULT_CONFIG),/moved|stale/i);
  assert.equal(f.calls.slice(before).some(args=>args[0]==='tab'&&args[1]==='focus'),false);
});
test('retargeting a document alias during launch never stamps the editor with the new target key', async () => {
  const f=await documentFixture(), base=f.run(f.context.terminalId);
  const card=new WorkspaceClient({env:f.card.env,pluginRoot:f.root,run:async args=>{
    const output=await base(args);
    if(args[0]==='plugin'&&args[2]==='open'&&args.includes('document')) {await unlink(join(f.cwd,'alias.md')); await symlink('second.md',join(f.cwd,'alias.md'));}
    return output;
  }});
  await assert.rejects(card.openDocument(f.store,f.context,f.workspace,f.opened.agent,f.target('alias.md'),DEFAULT_CONFIG),/partial.*canonical|partial.*target/i);
  const unowned=f.panes.at(-1); assert.equal(unowned.tokens.wcd_key,undefined);
  await f.card.openDocument(f.store,f.context,f.workspace,f.opened.agent,f.target('second.md'),DEFAULT_CONFIG);
  assert.notEqual(f.panes.at(-1).terminal_id,unowned.terminal_id);
});
test('metadata permits unrelated tokens up to 32 merged and limits each patch to 16', async () => {
  const f=await fixture(); for(let i=0;i<12;i++) f.workspaces[0].tokens[`other${i}`]='kept';
  const a=await f.joinAgent('term1'); assert.equal(Object.keys(f.workspaces[0].tokens).length,17);
  for(let i=0;i<12;i++) assert.equal(f.workspaces[0].tokens[`other${i}`],'kept');
  const native=()=>structuredClone(f.workspaces[0]);
  await assert.rejects(a.c.stamp('workspace',native(),Object.fromEntries(Array.from({length:17},(_,i)=>[`patch${i}`,'x'])),a.context,DEFAULT_CONFIG),/16.*patch|patch.*16/i);
  await a.c.stamp('workspace',native(),Object.fromEntries(Array.from({length:15},(_,i)=>[`patch${i}`,'x'])),a.context,DEFAULT_CONFIG);
  assert.equal(Object.keys(f.workspaces[0].tokens).length,32);
  await assert.rejects(a.c.stamp('workspace',native(),{overflow:'x'},a.context,DEFAULT_CONFIG),/32/i);
});
test('reservation retry reuses exactly one workspace and completed membership after ready publication fails', async () => {
  const f=await fixture(); const c=f.client('term1'), context=await c.bootstrap(DEFAULT_CONFIG);
  f.setFail(a=>a[1]==='report-metadata' && a.includes('wcw_phase=ready'));
  await assert.rejects(c.joinWorkspace(f.store,context,{sessionId:'s',cwd:f.cwd,title:'title',boundary},DEFAULT_CONFIG), /partial/i);
  const previous=await f.store.readAgent('s'); const id=f.workspaces[0].tokens.wcw_id;
  assert.equal(f.workspaces[0].tokens.wcw_phase,'reserved'); f.setFail(undefined);
  const result=await c.joinWorkspace(f.store,context,{sessionId:'s',cwd:f.cwd,title:'title',boundary},DEFAULT_CONFIG);
  assert.equal(result.workspace.record.id,id); assert.equal(result.agent.record.membershipToken,previous.record.membershipToken);
  assert.equal((await f.store.listWorkspaces()).length,1);
});
test('two agents share one binding but own independent card pairs and isolated projections', async () => {
  const f=await fixture(); const a=await f.joinAgent('term1'); const b=await f.joinAgent('term2');
  assert.equal(a.workspace.record.id,b.workspace.record.id);
  const p=await a.c.openDisplay(f.store,a.context,a.agent,'automatic',DEFAULT_CONFIG);
  const q=await b.c.openDisplay(f.store,b.context,b.agent,'automatic',DEFAULT_CONFIG);
  assert.equal(p.kind,'paired'); assert.equal(q.kind,'paired'); assert.notEqual(p.pair.card.terminal_id,q.pair.card.terminal_id);
  const shared = await f.store.addReferences(agentAuthority(p.agent.record), { documents: [{ checkout:p.agent.record.checkout, path:'plan.md', description:'' }] });
  await f.store.applyPlan(agentAuthority(p.agent.record), { goal: evaluatedGoal('One shared purpose', [{checkout:p.agent.record.checkout,path:'plan.md'}], 'purpose', 'Shared purpose') }, shared.revision);
  await a.c.projectSummaries(f.store,a.context,p.agent,DEFAULT_CONFIG);
  assert.equal(f.panes[0].tokens.wca_status,'active'); assert.equal(f.panes[1].tokens.wca_status,undefined);
  const r=p.agent.record; await f.store.updateOwn({sessionId:r.sessionId,workspaceId:r.workspaceId,membershipToken:r.membershipToken,evidenceGeneration:r.evidenceGeneration},{now:'Only agent one'},p.agent.revision);
  await a.c.projectSummaries(f.store,a.context,p.agent,DEFAULT_CONFIG);
  assert.equal(f.panes[0].tokens.wca_now,'Only agent one'); assert.equal(f.panes[1].tokens.wca_now,undefined);
  const second = await f.store.readAgent('term2');
  await f.store.updateOwn(agentAuthority(second.record), { now:'Only agent two', status:'blocked', statusShort:'Needs review' }, second.revision);
  await Promise.all([a.c.projectSummaries(f.store,a.context,p.agent,DEFAULT_CONFIG), b.c.projectSummaries(f.store,b.context,q.agent,DEFAULT_CONFIG)]);
  assert.equal(f.panes[0].tokens.wca_now,'Only agent one');
  assert.equal(f.panes[1].tokens.wca_now,'Only agent two');
  assert.equal(f.panes[1].tokens.wca_status,'blocked');
  assert.equal(f.panes[1].tokens.wca_status_short,'Needs review');
  assert.equal(f.workspaces[0].tokens.wca_now,undefined);
  assert.equal(f.workspaces[0].tokens.wcw_goal,'One shared purpose');
  assert.equal(f.workspaces[0].tokens.wcw_goal_short,'Shared purpose');
  await a.c.closeDisplay(f.store,a.context,p.agent,DEFAULT_CONFIG);
  assert.ok(f.panes.some(v=>v.terminal_id===q.pair.card.terminal_id)); assert.equal(f.workspaces[0].tokens.wcw_phase,'ready');
});
test('q and failed launch preserve attempts and never automatically respawn; explicit open can recover absent owner', async () => {
  const f=await fixture(); const a=await f.joinAgent('term1');
  const opened=await a.c.openDisplay(f.store,a.context,a.agent,'automatic',DEFAULT_CONFIG);
  f.panes.splice(f.panes.findIndex(p=>p.terminal_id===opened.pair.card.terminal_id),1);
  const suppressed=await a.c.openDisplay(f.store,a.context,opened.agent,'automatic',DEFAULT_CONFIG); assert.equal(suppressed.kind,'suppressed');
  const fresh=await a.c.openDisplay(f.store,a.context,opened.agent,'explicit',DEFAULT_CONFIG); assert.equal(fresh.kind,'paired');
  const g=await fixture(); const b=await g.joinAgent('term1'); g.setFail(a=>a[0]==='plugin'&&a[2]==='open');
  await assert.rejects(b.c.openDisplay(g.store,b.context,b.agent,'automatic',DEFAULT_CONFIG), /partial/i);
  assert.ok((await g.store.readAgent('term1')).record.displayAttempt);
  g.setFail(undefined); assert.equal((await b.c.openDisplay(g.store,b.context,await g.store.readAgent('term1'),'automatic',DEFAULT_CONFIG)).kind,'suppressed');
});
test('failure persisting the attempt blocks all plugin launches and marker writes', async () => {
  const f=await fixture(); const a=await f.joinAgent('term1'); const before=f.calls.length;
  f.store.setDisplayAttempt=async()=>{throw new Error('disk failure');};
  await assert.rejects(a.c.openDisplay(f.store,a.context,a.agent,'automatic',DEFAULT_CONFIG),/disk failure/i);
  assert.equal(f.calls.slice(before).some(args=>args[0]==='plugin'||args[1]==='report-metadata'),false);
});
test('manually changed own checkout with unchanged membership token invalidates painted operations', async () => {
  const f=await fixture(); const a=await f.joinAgent('term1');
  const {stringify}=await import('yaml'); const cwd=join(f.cwd,'subdir'); await mkdir(cwd);
  await writeFile(a.agent.path,stringify({...a.agent.record,cwd}));
  await assert.rejects(a.c.openDisplay(f.store,a.context,a.agent,'automatic',DEFAULT_CONFIG),/stale|membership/i);
  assert.equal(f.panes.length,2);
});
test('partial pair publication fails closed on explicit retry instead of adopting a lone card', async () => {
  const f=await fixture(); const a=await f.joinAgent('term1');
  f.setFail(args=>args[0]==='pane'&&args[1]==='report-metadata'&&args[2]==='w:p1');
  await assert.rejects(a.c.openDisplay(f.store,a.context,a.agent,'automatic',DEFAULT_CONFIG),/partial/i);
  const count=f.panes.length; f.setFail(undefined);
  await assert.rejects(a.c.openDisplay(f.store,a.context,await f.store.readAgent('term1'),'explicit',DEFAULT_CONFIG),/duplicate|incomplete|conflict/i);
  assert.equal(f.panes.length,count);
});
test('card-side close clears only matching pair metadata after its own terminal disappears', async () => {
  const f=await fixture(); const a=await f.joinAgent('term1'); const opened=await a.c.openDisplay(f.store,a.context,a.agent,'automatic',DEFAULT_CONFIG);
  const c=f.client(opened.pair.card.terminal_id); c.env.WORK_CARDS_NONCE=opened.pair.nonce; const context=await c.bootstrap(DEFAULT_CONFIG);
  await c.closeDisplay(f.store,context,opened.agent,DEFAULT_CONFIG);
  assert.equal(f.panes[0].tokens.wca_role,undefined); assert.equal(f.workspaces[0].tokens.wcw_phase,'ready');
});
test('legacy and foreign bindings refuse joining before record creation', async () => {
  for (const tokens of [{wc_card:'legacy'},{wcw_id:'foreign',wcw_store:'other',wcw_token:'x',wcw_nonce:'n',wcw_phase:'ready'}]) {
    const f=await fixture(); f.workspaces[0].tokens=tokens;
    await assert.rejects(f.joinAgent('term1'), /legacy|foreign/i); assert.equal((await f.store.listWorkspaces()).length,0);
  }
});
for (const field of ['sessionId', 'workspaceId', 'membershipToken', 'checkout', 'cwd', 'evidenceGeneration']) test(`a ${field} edit during the paired record read blocks native effects`, async () => {
  const f=await fixture(); const a=await f.joinAgent('term1'); const read=f.store.readWorkspace.bind(f.store); let once=true;
  f.store.readWorkspace=async id=>{
    const result=await read(id);
    if(once) { once=false; await writeFile(a.agent.path,JSON.stringify({...a.agent.record,[field]:field==='cwd'?join(f.cwd,'subdir'):'replacement'})); }
    return result;
  };
  await assert.rejects(a.c.ownership(f.store,a.context,a.agent,DEFAULT_CONFIG),/stale|membership|identity/i);
  assert.equal(f.calls.some(args=>args[0]==='plugin'),false);
});
test('native paired read returns progress published during workspace read without replacing painted identity', async () => {
  const f = await fixture(), a = await f.joinAgent('term1');
  const read = f.store.readWorkspace.bind(f.store); let latest;
  f.store.readWorkspace = async id => {
    const workspace = await read(id);
    latest = await f.store.updateOwn(agentAuthority(a.agent.record), { now: 'Latest progress' }, a.agent.revision);
    return workspace;
  };
  const current = await a.c.currentRecords(f.store, a.agent);
  assert.deepEqual(current.agent, latest);
  assert.notEqual(current.agent.revision, a.agent.revision);
});

test('native paired read retains canonical painted agent path validation', async () => {
  const f = await fixture(), a = await f.joinAgent('term1');
  const foreign = join(f.root, 'foreign-agent.yaml');
  await writeFile(foreign, JSON.stringify(a.agent.record));
  await assert.rejects(a.c.currentRecords(f.store, { ...a.agent, path: foreign }), /foreign agent path/i);
});

test('orphan projection and incomplete legacy pane markers cannot allocate a competing workspace', async () => {
  for (const variant of ['projection','legacy']) {
    const f=await fixture();
    if(variant==='projection') f.workspaces[0].tokens.wcw_goal='surviving projection'; else f.panes[1].tokens.wc_nonce='partial-legacy';
    await assert.rejects(f.joinAgent('term1'),/legacy|incomplete|recovery/i);
    assert.equal((await f.store.listWorkspaces()).length,0);
  }
});
test('binding lifetime nonce invalidates already captured contexts', async () => {
  const f=await fixture(); const a=await f.joinAgent('term1');
  f.workspaces[0].tokens.wcw_nonce='new-lifetime';
  await assert.rejects(a.c.openDisplay(f.store,a.context,a.agent,'automatic',DEFAULT_CONFIG),/lifetime|binding/i);
  assert.equal((await f.store.readAgent('term1')).record.displayAttempt,undefined);
});
test('stale contexts and cancellation block native effects', async () => {
  const f=await fixture(); const a=await f.joinAgent('term1'); f.panes[0].tab_id='w:t2';
  await assert.rejects(a.c.openDisplay(f.store,a.context,a.agent,'automatic',DEFAULT_CONFIG), /moved|stale/i);
  assert.equal((await f.store.readAgent('term1')).record.displayAttempt,undefined);
  const signal=AbortSignal.abort(); await assert.rejects(a.c.joinWorkspace(f.store,a.context,{sessionId:'s',cwd:f.cwd,title:'s',boundary},DEFAULT_CONFIG,signal));
});
test('native cross-workspace terminal movement joins fresh destination without copying references', async () => {
  const f=await fixture(); const a=await f.joinAgent('term1'), r=a.agent.record;
  const auth={sessionId:r.sessionId,workspaceId:r.workspaceId,membershipToken:r.membershipToken,evidenceGeneration:r.evidenceGeneration};
  await f.store.addReferences(auth,{documents:[{checkout:r.checkout,path:'plan.md',description:'source only'}]});
  f.workspaces.push({workspace_id:'dest',tokens:{}}); f.tabs.push({workspace_id:'dest',tab_id:'dest:t1',label:'1',number:1});
  Object.assign(f.panes[0],{workspace_id:'dest',tab_id:'dest:t1',pane_id:'dest:p1'});
  const observed=await a.c.observe(a.context,DEFAULT_CONFIG); assert.equal(observed.caller.terminal_id,'term1');
  const joined=await a.c.joinWorkspace(f.store,observed.context,{sessionId:'term1',cwd:f.cwd,title:'destination',boundary,expected:auth},DEFAULT_CONFIG);
  assert.notEqual(joined.workspace.record.id,a.workspace.record.id); assert.deepEqual(joined.workspace.record.documents,[]);
  assert.notEqual(joined.agent.record.membershipToken,r.membershipToken); assert.equal((await f.store.readWorkspace(r.workspaceId)).record.documents.length,1);
  assert.equal(f.workspaces[0].tokens.wcw_id,r.workspaceId);
});
test('concurrent reservation contenders wait into the same allocation without manual retry', async () => {
  const f=await fixture(); const a=f.client('term1'), b=f.client('term2'); const ca=await a.bootstrap(DEFAULT_CONFIG), cb=await b.bootstrap(DEFAULT_CONFIG);
  const input=s=>({sessionId:s,cwd:f.cwd,title:s,boundary});
  const results=await Promise.allSettled([a.joinWorkspace(f.store,ca,input('a'),DEFAULT_CONFIG),b.joinWorkspace(f.store,cb,input('b'),DEFAULT_CONFIG)]);
  assert.deepEqual(results.map(r=>r.status), ['fulfilled', 'fulfilled']);
  assert.equal(results[0].value.workspace.record.id,results[1].value.workspace.record.id);
  assert.equal((await f.store.listWorkspaces()).length,1);
});
test('discovered join target releases native before durable coordination and rejects subsequent rebinding', async t => {
  for (const change of ['none', 'id', 'nonce', 'phase', 'token']) await t.test(change, async () => {
    const f = await fixture(), a = f.client('term1'), b = f.client('term2');
    const contexts = await Promise.all([a.bootstrap(DEFAULT_CONFIG), b.bootstrap(DEFAULT_CONFIG)]);
    const input = sessionId => ({ sessionId, cwd: f.cwd, title: sessionId, boundary });
    let resumeFirst, firstEntered, secondRead, durableEntered, releaseDurable, holder;
    const firstGate = new Promise(resolve => { resumeFirst = resolve; });
    const firstReady = new Promise(resolve => { firstEntered = resolve; });
    const secondReady = new Promise(resolve => { secondRead = resolve; });
    const durableReady = new Promise(resolve => { durableEntered = resolve; });
    const durableGate = new Promise(resolve => { releaseDurable = resolve; });
    const firstSnapshot = a.snapshot.bind(a), secondSnapshot = b.snapshot.bind(b);
    let firstReads = 0, secondReads = 0;
    a.snapshot = async config => {
      const result = await firstSnapshot(config);
      if (++firstReads === 2) { firstEntered(); await firstGate; }
      return result;
    };
    b.snapshot = async config => {
      const result = await secondSnapshot(config);
      if (++secondReads === 1) secondRead();
      if (secondReads === 2) {
        const key = createHash('sha256').update(await realpath(f.store.directory)).digest('hex');
        const target = f.workspaces[0].tokens.wcw_id;
        holder = withWorkspaceLock(contexts[1].socket, `durable-workspace\0${key}\0${target}`, async () => { durableEntered(); await durableGate; });
        await durableReady;
      }
      return result;
    };
    const first = a.joinWorkspace(f.store, contexts[0], input('a'), DEFAULT_CONFIG);
    await firstReady;
    const second = b.joinWorkspace(f.store, contexts[1], input('b'), DEFAULT_CONFIG);
    const outcome = Promise.allSettled([second]);
    try {
      await secondReady; resumeFirst();
      const joined = await first;
      await durableReady;
      // The waiter must release native before waiting on the held durable target.
      await withWorkspaceLock(contexts[1].socket, contexts[1].workspaceId, async () => {
        assert.equal((await f.store.listAgents()).length, 1);
        if (change === 'id') f.workspaces[0].tokens.wcw_id = 'replacement';
        if (change === 'nonce') f.workspaces[0].tokens.wcw_nonce = 'replacement';
        if (change === 'phase') f.workspaces[0].tokens.wcw_phase = 'reserved';
        if (change === 'token') f.workspaces[0].tokens.wcw_token = 'replacement';
      }, { waitMs: 200 });
      const before = f.calls.length;
      releaseDurable(); await holder;
      const [result] = await outcome;
      if (change === 'none') {
        assert.equal(result.status, 'fulfilled');
        assert.equal(result.value.workspace.record.id, joined.workspace.record.id);
      } else {
        assert.equal(result.status, 'rejected');
        assert.match(result.reason.message, /binding.*changed|binding lifetime/i);
        assert.deepEqual(effects(f.calls.slice(before)), []);
        assert.equal((await f.store.listAgents()).length, 1);
      }
      assert.equal((await f.store.listWorkspaces()).length, 1);
    } finally { resumeFirst(); releaseDurable(); await holder; await first; await outcome; }
  });
});

test('separated pair is positively classified and cleanup never respawns an absent q display', async () => {
  const f=await fixture(); const a=await f.joinAgent('term1'); const opened=await a.c.openDisplay(f.store,a.context,a.agent,'automatic',DEFAULT_CONFIG);
  f.panes[0].tab_id='w:t2'; f.panes[0].pane_id='w:p99';
  const moved=await a.c.observe(a.context,DEFAULT_CONFIG); assert.equal(moved.caller.pane_id,'w:p99');
  assert.equal((await a.c.ownership(f.store,moved.context,opened.agent,DEFAULT_CONFIG)).kind,'separated');
  await a.c.cleanupSeparatedPair(f.store,moved.context,opened.agent,DEFAULT_CONFIG);
  assert.equal((await a.c.openDisplay(f.store,moved.context,opened.agent,'automatic',DEFAULT_CONFIG)).kind,'suppressed');
  await assert.rejects(a.c.cleanupSeparatedPair(f.store,moved.context,opened.agent,DEFAULT_CONFIG),/positively/i);
});
test('Pi membership verification validates ready lifetime, own caller and absent q display', async () => {
  const f=await fixture(), a=await f.joinAgent('term1');
  const empty=await a.c.verifyMembership(f.store,a.context,a.agent,DEFAULT_CONFIG);
  assert.equal(empty.ownership.kind,'empty'); assert.equal(empty.context.bindingNonce,a.binding.nonce);
  const opened=await a.c.openDisplay(f.store,a.context,a.agent,'automatic',DEFAULT_CONFIG);
  f.panes.splice(f.panes.findIndex(p=>p.terminal_id===opened.pair.card.terminal_id),1);
  assert.equal((await a.c.verifyMembership(f.store,a.context,opened.agent,DEFAULT_CONFIG)).ownership.kind,'absent');
  await a.c.releaseDisplay(f.store,a.context,opened.agent,DEFAULT_CONFIG);
  assert.equal(f.panes[0].tokens.wca_agent,undefined);
  assert.ok((await f.store.readAgent('term1')).record.displayAttempt);
  f.workspaces[0].tokens.wcw_phase='reserved';
  await assert.rejects(a.c.verifyMembership(f.store,a.context,opened.agent,DEFAULT_CONFIG),/binding/i);
});
test('replacement refuses paired/q-absent/empty, replaces one fresh separated pair once', async () => {
  const f=await fixture(), a=await f.joinAgent('term1');
  await assert.rejects(a.c.replaceSeparatedDisplay(f.store,a.context,a.agent,DEFAULT_CONFIG),/separated/i);
  const opened=await a.c.openDisplay(f.store,a.context,a.agent,'automatic',DEFAULT_CONFIG);
  await assert.rejects(a.c.replaceSeparatedDisplay(f.store,a.context,opened.agent,DEFAULT_CONFIG),/separated/i);
  f.panes[0].tab_id='w:t2'; const {context}=await a.c.observe(a.context,DEFAULT_CONFIG);
  const replaced=await a.c.replaceSeparatedDisplay(f.store,context,opened.agent,DEFAULT_CONFIG);
  assert.equal(replaced.kind,'paired'); assert.notEqual(replaced.pair.nonce,opened.pair.nonce);
  await assert.rejects(a.c.replaceSeparatedDisplay(f.store,context,replaced.agent,DEFAULT_CONFIG),/separated/i);
  f.panes.splice(f.panes.findIndex(p=>p.terminal_id===replaced.pair.card.terminal_id),1);
  await assert.rejects(a.c.replaceSeparatedDisplay(f.store,context,replaced.agent,DEFAULT_CONFIG),/separated/i);
  assert.equal((await a.c.openDisplay(f.store,context,replaced.agent,'automatic',DEFAULT_CONFIG)).kind,'suppressed');
});
test('replacement cleanup failure, cancellation and authority/native invalidation cannot launch', async () => {
  for (const failure of ['close','stamp','abort','membership','native']) {
    const f=await fixture(), a=await f.joinAgent('term1');
    const opened=await a.c.openDisplay(f.store,a.context,a.agent,'automatic',DEFAULT_CONFIG);
    f.panes[0].tab_id='w:t2'; const {context}=await a.c.observe(a.context,DEFAULT_CONFIG);
    const controller=new AbortController(), base=f.run('term1');
    const client=new WorkspaceClient({env:a.c.env,pluginRoot:f.root,run:async args=>{
      if(failure==='close'&&args[0]==='plugin'&&args[2]==='close') throw new Error('close failed');
      if(failure==='stamp'&&args[1]==='report-metadata') throw new Error('stamp failed');
      const output=await base(args);
      if(args[0]==='plugin'&&args[2]==='close') {
        if(failure==='abort') controller.abort();
        if(failure==='native') f.panes[0].tab_id='w:t1';
        if(failure==='membership') {
          const current=await f.store.readAgent('term1');
          await f.store.resetEvidence(agentAuthority(current.record),boundary,current.revision);
        }
      }
      return output;
    }});
    const before=f.calls.length;
    await assert.rejects(client.replaceSeparatedDisplay(f.store,context,opened.agent,DEFAULT_CONFIG,controller.signal));
    assert.equal(f.calls.slice(before).some(args=>args[0]==='plugin'&&args[2]==='open'),false);
  }
});
test('failed replacement launch records an attempt and never automatically respawns', async () => {
  const f=await fixture(), a=await f.joinAgent('term1');
  const opened=await a.c.openDisplay(f.store,a.context,a.agent,'automatic',DEFAULT_CONFIG);
  f.panes[0].tab_id='w:t2'; const {context}=await a.c.observe(a.context,DEFAULT_CONFIG);
  f.setFail(args=>args[0]==='plugin'&&args[2]==='open');
  await assert.rejects(a.c.replaceSeparatedDisplay(f.store,context,opened.agent,DEFAULT_CONFIG),/partial/i);
  f.setFail(undefined); const current=await f.store.readAgent('term1');
  assert.notEqual(current.record.displayAttempt.nonce,opened.agent.record.displayAttempt.nonce);
  assert.equal((await a.c.openDisplay(f.store,context,current,'automatic',DEFAULT_CONFIG)).kind,'suppressed');
  await assert.rejects(a.c.replaceSeparatedDisplay(f.store,context,current,DEFAULT_CONFIG),/separated/i);
});
test('collision refresh renames only proven automatic editor names and current first panes', async () => {
  const f=await fixture(); const a=await f.joinAgent('term1'); const r=a.agent.record;
  const auth={sessionId:r.sessionId,workspaceId:r.workspaceId,membershipToken:r.membershipToken,evidenceGeneration:r.evidenceGeneration};
  await mkdir(join(f.cwd,'docs')); await writeFile(join(f.cwd,'docs','plan.md'),'plan');
  const target={checkout:r.checkout,path:'docs/plan.md'};
  let workspace=await f.store.addReferences(auth,{documents:[{...target,description:'plan'}]});
  const opened=await a.c.openDisplay(f.store,a.context,a.agent,'automatic',DEFAULT_CONFIG); const card=f.client(opened.pair.card.terminal_id); card.env.WORK_CARDS_NONCE=opened.pair.nonce; const context=await card.bootstrap(DEFAULT_CONFIG);
  await card.openDocument(f.store,context,workspace,opened.agent,target,DEFAULT_CONFIG);
  const editor=f.panes.find(p=>p.tokens.wcd_key); const tab=f.tabs.find(t=>t.tab_id===editor.tab_id); assert.equal(tab.label,'docs/plan.md');
  const second=join(f.root,'other','checkout'); await mkdir(second,{recursive:true});
  workspace=await f.store.registerCheckout(auth,second,workspace.revision);
  await card.refreshDocumentNames(f.store,context,opened.agent,DEFAULT_CONFIG);
  assert.equal(tab.label,resolvedReferences(workspace.record).documents[0].label);
  tab.label='Manual'; await card.refreshDocumentNames(f.store,context,opened.agent,DEFAULT_CONFIG);
  assert.equal(tab.label,'Manual'); assert.equal(editor.tokens.wcd_auto,'manual');
  const initialLabel=tab.label; editor.tokens.wcd_auto='auto';
  const { createHash }=await import('node:crypto'); editor.tokens.wcd_name=createHash('sha256').update(initialLabel).digest('hex');
  const intruder={pane_id:'w:p100',terminal_id:'term100',workspace_id:'w',tab_id:tab.tab_id,tokens:{}}; f.panes.unshift(intruder);
  await card.refreshDocumentNames(f.store,context,opened.agent,DEFAULT_CONFIG); assert.equal(tab.label,initialLabel);
});
test('same document path in separate checkouts opens separate qualified editors and removed painted references cannot open', async () => {
  const f=await fixture(); const a=await f.joinAgent('term1'), r=a.agent.record;
  const auth={sessionId:r.sessionId,workspaceId:r.workspaceId,membershipToken:r.membershipToken,evidenceGeneration:r.evidenceGeneration};
  const second=join(f.root,'second'); await mkdir(second); await writeFile(join(second,'plan.md'),'second');
  let workspace=await f.store.registerCheckout(auth,second,a.workspace.revision);
  const other=Object.entries(workspace.record.checkouts).find(([,c])=>c.directory===second)[0];
  workspace=await f.store.addReferences(auth,{documents:[{checkout:r.checkout,path:'plan.md',description:''},{checkout:other,path:'plan.md',description:''}]});
  const opened=await a.c.openDisplay(f.store,a.context,a.agent,'automatic',DEFAULT_CONFIG); const card=f.client(opened.pair.card.terminal_id); card.env.WORK_CARDS_NONCE=opened.pair.nonce; const context=await card.bootstrap(DEFAULT_CONFIG);
  for (const ref of workspace.record.documents) await card.openDocument(f.store,context,workspace,opened.agent,{checkout:ref.checkout,path:ref.path},DEFAULT_CONFIG);
  const editors=f.panes.filter(p=>p.tokens.wcd_key); assert.equal(editors.length,2); assert.notEqual(editors[0].tokens.wcd_key,editors[1].tokens.wcd_key);
  assert.deepEqual(editors.map(p=>f.tabs.find(t=>t.tab_id===p.tab_id).label).sort(),resolvedReferences(workspace.record).documents.map(d=>d.label).sort());
  const {referenceIdentity}=await import('../dist/records.js'); const target={checkout:r.checkout,path:'plan.md'};
  await f.store.removeReference(auth,{workspaceId:r.workspaceId,revision:workspace.revision,identity:referenceIdentity(target)});
  await assert.rejects(card.openDocument(f.store,context,workspace,opened.agent,target,DEFAULT_CONFIG),/reference/i);
});
test('document opens exact painted identity, reuses verified editor and preserves manual names', async () => {
  const f=await fixture(); const a=await f.joinAgent('term1'); const r=a.agent.record;
  const auth={sessionId:r.sessionId,workspaceId:r.workspaceId,membershipToken:r.membershipToken,evidenceGeneration:r.evidenceGeneration};
  const workspace=await f.store.addReferences(auth,{documents:[{checkout:r.checkout,path:'plan.md',description:'plan'}]});
  const opened=await a.c.openDisplay(f.store,a.context,a.agent,'automatic',DEFAULT_CONFIG);
  const card=f.client(opened.pair.card.terminal_id); card.env.WORK_CARDS_NONCE=opened.pair.nonce; const context=await card.bootstrap(DEFAULT_CONFIG);
  const identity={checkout:r.checkout,path:'plan.md'};
  await card.openDocument(f.store,context,workspace,opened.agent,identity,DEFAULT_CONFIG);
  const editor=f.panes.find(p=>p.tokens.wcd_key); const tab=f.tabs.find(t=>t.tab_id===editor.tab_id); assert.equal(tab.label,'plan.md');
  tab.label='Manual'; const count=f.panes.length;
  await card.openDocument(f.store,context,workspace,opened.agent,identity,DEFAULT_CONFIG);
  assert.equal(f.panes.length,count); assert.equal(tab.label,'Manual');
  await assert.rejects(a.c.openDocument(f.store,a.context,workspace,opened.agent,identity,DEFAULT_CONFIG),/card/i);
  const painted=structuredClone(workspace); painted.record.checkouts[r.checkout].directory=f.root;
  await assert.rejects(card.openDocument(f.store,context,painted,opened.agent,identity,DEFAULT_CONFIG),/checkout|stale/i);
});

async function guardedDocumentFixture(t) {
  const f = await documentFixture();
  const { rm } = await import('node:fs/promises');
  t.after(() => rm(f.root, { recursive: true, force: true }));
  return { ...f, client: f.card, open: () => f.card.openDocument(f.store, f.context, f.workspace, f.opened.agent, f.target('plan.md'), DEFAULT_CONFIG) };
}
const documentMutations = calls => calls.filter(a => ['report-metadata', 'rename', 'focus'].includes(a[1]) || a.slice(0, 3).join(' ') === 'plugin pane open');

test('document matching partial, stale and duplicate editor tuples refuse without effects', async t => {
  for (const mode of ['partial', 'pane', 'terminal', 'tab', 'duplicate']) await t.test(mode, async t => {
    const f = await guardedDocumentFixture(t); await f.open();
    const editor = f.panes.find(p => p.tokens.wcd_key);
    if (mode === 'partial') delete editor.tokens.wcd_tab;
    else if (mode === 'duplicate') {
      f.tabs.push({ tab_id: 'w:t90', workspace_id: 'w', label: '90', number: 90 });
      f.panes.push({ ...editor, pane_id: 'w:p90', terminal_id: 'term90', tab_id: 'w:t90' });
    } else editor.tokens['wcd_' + mode] = 'wrong';
    f.calls.length = 0;
    await assert.rejects(f.open(), /document.*tuple/i);
    assert.deepEqual(documentMutations(f.calls), []);
  });
});

test('document partial launch, metadata, rename and focus failures never close or retry editors', async t => {
  for (const mode of ['open', 'metadata', 'rename', 'focus']) await t.test(mode, async t => {
    const f = await guardedDocumentFixture(t); f.calls.length = 0;
    f.setFail(args => mode === 'open' ? args.slice(0, 3).join(' ') === 'plugin pane open'
      : args[1] === ({ metadata: 'report-metadata', rename: 'rename', focus: 'focus' })[mode]);
    await assert.rejects(f.open(), /partial.*inspect/i);
    assert.equal(f.calls.filter(a => a.slice(0, 3).join(' ') === 'plugin pane open').length, 1);
    assert.equal(f.calls.some(a => a.slice(0, 3).join(' ') === 'plugin pane close'), false);
    if (mode !== 'focus') assert.equal(f.calls.some(a => a[1] === 'focus'), false);
  });
});

test('successful editor metadata acknowledgement without persistence blocks rename and focus', async t => {
  const f = await guardedDocumentFixture(t), run = f.client.run; f.calls.length = 0;
  f.client.run = async args => {
    if (args[0] === 'pane' && args[1] === 'report-metadata' && args.some(value => value.startsWith('wcd_key='))) {
      f.calls.push(args);
      return ''; // Normal report success, but the new editor markers were not persisted.
    }
    return run(args);
  };
  await assert.rejects(f.open(), error => {
    const editor = f.panes.at(-1);
    assert.match(error.message, /Document metadata did not round-trip/);
    for (const identity of [editor.pane_id, editor.terminal_id, editor.tab_id]) assert.ok(error.message.includes(identity), error.message);
    assert.deepEqual(editor.tokens, {});
    return true;
  });
  assert.equal(f.calls.filter(a => a.slice(0, 3).join(' ') === 'plugin pane open').length, 1);
  assert.equal(f.calls.filter(a => a[1] === 'report-metadata').length, 1);
  assert.equal(f.calls.some(a => a[0] === 'tab' && ['rename', 'focus'].includes(a[1])), false);
  assert.equal(f.calls.some(a => a.slice(0, 3).join(' ') === 'plugin pane close'), false);
});

test('document launch rejects returned plugin, entrypoint and reused native identities before stamping', async t => {
  for (const mode of ['plugin', 'entrypoint', 'workspace', 'pane', 'terminal', 'tab']) await t.test(mode, async t => {
    const f = await guardedDocumentFixture(t), run = f.client.run; f.calls.length = 0;
    f.client.run = async args => {
      const raw = await run(args);
      if (args.slice(0, 3).join(' ') !== 'plugin pane open') return raw;
      const response = JSON.parse(raw), info = response.result.plugin_pane;
      if (mode === 'plugin') info.plugin_id = 'other';
      else if (mode === 'entrypoint') info.entrypoint = 'card';
      else info.pane[mode === 'workspace' ? 'workspace_id' : mode === 'pane' ? 'pane_id' : mode === 'terminal' ? 'terminal_id' : 'tab_id']
        = mode === 'workspace' ? 'other' : f.panes[0][mode === 'pane' ? 'pane_id' : mode === 'terminal' ? 'terminal_id' : 'tab_id'];
      return JSON.stringify(response);
    };
    await assert.rejects(f.open(), /partial.*inspect/i);
    const mutations = documentMutations(f.calls);
    assert.equal(mutations.length, 1); assert.deepEqual(mutations[0].slice(0, 3), ['plugin', 'pane', 'open']);
  });
});

test('summaries survive q without native display effects or synthesized shorts; empty display fails closed', async () => {
  const f = await documentFixture();
  f.panes.splice(f.panes.findIndex(p => p.terminal_id === f.context.terminalId), 1);
  let agent = await f.store.readAgent('term1');
  agent = await f.store.updateOwn(agentAuthority(agent.record), { now: 'x'.repeat(100) }, agent.revision);
  const before = f.calls.length;
  await f.a.c.projectSummaries(f.store, f.a.context, agent, DEFAULT_CONFIG);
  assert.equal(f.panes[0].tokens.wca_now, 'x'.repeat(80));
  assert.equal(f.panes[0].tokens.wca_status_short, undefined);
  assert.equal(f.workspaces[0].tokens.wcw_goal_short, undefined);
  assert.equal(effects(f.calls.slice(before)).every(a => a[1] === 'report-metadata'), true);
  await f.a.c.releaseDisplay(f.store, f.a.context, agent, DEFAULT_CONFIG);
  const emptyStart = f.calls.length;
  await assert.rejects(f.a.c.projectSummaries(f.store, f.a.context, agent, DEFAULT_CONFIG), /explicit open establishes projection ownership/i);
  // Even an unrelated unmarked Pi cannot adopt the painted membership.
  const unrelated = f.client('term2'), context = await unrelated.bootstrap(DEFAULT_CONFIG);
  await assert.rejects(unrelated.projectSummaries(f.store, context, agent, DEFAULT_CONFIG), /explicit open establishes projection ownership/i);
  assert.deepEqual(effects(f.calls.slice(emptyStart)), []);
  assert.equal(f.panes[0].tokens.wca_now, undefined);
});

test('short summaries publish independently and cleared fields remove stale text', async () => {
  const f = await documentFixture(), { stringify } = await import('yaml');
  const agent = await f.store.readAgent('term1'), workspace = await f.store.readWorkspace(agent.record.workspaceId);
  await writeFile(agent.path, stringify({ ...agent.record, statusShort: 'Testing界' }));
  await f.store.applyPlan(agentAuthority(agent.record), { goal: evaluatedGoal('Shared purpose in full', [f.target('plan.md')], 'short-purpose', 'Shared purpose') }, workspace.revision);
  await f.a.c.projectSummaries(f.store, f.a.context, agent, DEFAULT_CONFIG);
  assert.equal(f.panes[0].tokens.wca_status_short, 'Testing界');
  assert.equal(f.workspaces[0].tokens.wcw_goal_short, 'Shared purpose');
  await writeFile(agent.path, stringify(agent.record));
  await writeFile(workspace.path, stringify(serializeWorkspaceV1(workspace.record)));
  await f.a.c.projectSummaries(f.store, f.a.context, agent, DEFAULT_CONFIG);
  assert.equal(f.panes[0].tokens.wca_status_short ?? '', '');
  assert.equal(f.workspaces[0].tokens.wcw_goal_short ?? '', '');
});

test('summary publications require owning Pi, not a card or another agent', async () => {
  const f = await documentFixture(), b = await f.joinAgent('term2');
  const before = f.calls.length;
  await assert.rejects(f.card.projectSummaries(f.store, f.context, f.opened.agent, DEFAULT_CONFIG), /owning|Pi|agent/i);
  await assert.rejects(b.c.projectSummaries(f.store, b.context, f.opened.agent, DEFAULT_CONFIG), /foreign|owning|agent/i);
  assert.deepEqual(effects(f.calls.slice(before)), []);
  await assert.rejects(f.a.c.projectCaption(f.store, f.a.context, f.opened.agent, DEFAULT_CONFIG), /paired card/i);
});

test('separated display still permits independent agent summaries and cleanup retires only owned tokens', async () => {
  const f = await documentFixture(), b = await f.joinAgent('term2');
  const other = await b.c.openDisplay(f.store, b.context, b.agent, 'automatic', DEFAULT_CONFIG);
  f.panes.find(p => p.terminal_id === f.context.terminalId).tab_id = 'w:t2';
  f.panes[0].tokens.wca_status_short = 'stale';
  f.workspaces[0].tokens.wcw_goal = 'stale shared';
  await f.a.c.projectSummaries(f.store, f.a.context, f.opened.agent, DEFAULT_CONFIG);
  assert.equal(f.panes[0].tokens.wca_status, 'active');
  assert.equal(f.panes[0].tokens.wca_status_short ?? '', '');
  await b.c.projectSummaries(f.store, b.context, other.agent, DEFAULT_CONFIG);
  f.panes[0].tokens.wca_status_short = 'owned progress';
  const others = structuredClone(f.panes.filter(p => p.tokens.wca_agent !== f.panes[0].tokens.wca_agent));
  f.workspaces[0].tokens.wcw_goal_short = 'shared retained';
  await f.a.c.releaseDisplay(f.store, f.a.context, f.opened.agent, DEFAULT_CONFIG);
  assert.equal(f.panes[0].tokens.wca_status_short, undefined);
  assert.equal(f.workspaces[0].tokens.wcw_goal_short, 'shared retained');
  assert.deepEqual(f.panes.filter(p => others.some(o => o.terminal_id === p.terminal_id)), others);
});

for (const operation of ['open', 'caption', 'summary']) {
  for (const invalid of ['timeout', 'abort', 'move', 'binding', 'membership']) test(`${operation} waiter rechecks ${invalid} without effects`, async () => {
    const f = await documentFixture(), controller = new AbortController(), before = f.calls.length;
    const config = { ...DEFAULT_CONFIG, herdr_timeout_ms: invalid === 'timeout' ? 40 : 1000 };
    const invoke = () => operation === 'open'
      ? f.a.c.openDisplay(f.store, f.a.context, f.opened.agent, 'automatic', config, controller.signal)
      : operation === 'caption'
        ? f.card.projectCaption(f.store, f.context, f.opened.agent, config, controller.signal)
        : f.a.c.projectSummaries(f.store, f.a.context, f.opened.agent, config, controller.signal);
    await heldLock(f, async release => {
      const started = performance.now();
      const rejection = assert.rejects(invoke(), /timed out|abort|moved|stale|membership|binding/i);
      if (invalid === 'timeout') {
        await rejection;
        assert.ok(performance.now() - started < 500);
        return;
      }
      await Promise.all([rejection, delay(60).then(async () => {
        assert.deepEqual(effects(f.calls.slice(before)), []);
        if (invalid === 'abort') {
          controller.abort();
          await rejection; // Cancellation settles while the owner still holds the lock.
          assert.ok(performance.now() - started < 500);
          return;
        }
        if (invalid === 'move') f.panes.find(p => p.terminal_id === (operation === 'caption' ? f.context.terminalId : 'term1')).tab_id = 'w:t2';
        if (invalid === 'binding') f.workspaces[0].tokens.wcw_nonce = 'replacement';
        if (invalid === 'membership') {
          const agent = await f.store.readAgent('term1');
          await f.store.resetEvidence(agentAuthority(agent.record), boundary, agent.revision);
        }
        await release();
      })]);
    });
    assert.deepEqual(effects(f.calls.slice(before)), []);
  });
}

for (const state of ['new', 'reuse', 'q-absent']) test(`display ${state} waits for a transient owner and preserves membership`, async () => {
  const f = await fixture(), a = await f.joinAgent('term1');
  let painted = a.agent, previous;
  if (state !== 'new') {
    previous = await a.c.openDisplay(f.store, a.context, painted, 'automatic', DEFAULT_CONFIG);
    painted = previous.agent;
    if (state === 'q-absent') f.panes.splice(f.panes.findIndex(p => p.terminal_id === previous.pair.card.terminal_id), 1);
  }
  const before = f.calls.length;
  let result;
  await heldLock({ context: a.context }, async release => {
    await Promise.all([
      a.c.openDisplay(f.store, a.context, painted, 'automatic', DEFAULT_CONFIG).then(value => { result = value; }),
      delay(60).then(async () => { assert.deepEqual(effects(f.calls.slice(before)), []); await release(); })
    ]);
  });
  assert.equal(result.kind, state === 'q-absent' ? 'suppressed' : 'paired');
  if (state === 'reuse') assert.equal(result.pair.nonce, previous.pair.nonce);
  const verified = await a.c.verifyMembership(f.store, a.context, result.agent, DEFAULT_CONFIG);
  assert.equal(verified.ownership.kind, state === 'q-absent' ? 'absent' : 'paired');
  assert.equal(f.calls.slice(before).filter(args => args[0] === 'plugin' && args[2] === 'open').length, state === 'new' ? 1 : 0);
});

test('automatic open rereads q absence after waiting and never respawns', async () => {
  const f = await documentFixture(), before = f.calls.length;
  await heldLock(f, async release => {
    await Promise.all([
      f.a.c.openDisplay(f.store, f.a.context, f.opened.agent, 'automatic', DEFAULT_CONFIG).then(result => {
        assert.equal(result.kind, 'suppressed'); assert.equal(result.ownership.kind, 'absent');
      }),
      delay(60).then(async () => {
        f.panes.splice(f.panes.findIndex(p => p.terminal_id === f.context.terminalId), 1);
        await release();
      })
    ]);
  });
  assert.deepEqual(effects(f.calls.slice(before)), []);
  assert.equal((await f.a.c.verifyMembership(f.store, f.a.context, f.opened.agent, DEFAULT_CONFIG)).ownership.kind, 'absent');
});

for (const operation of ['caption', 'summary']) test(`${operation} skips publication if another owner caught up during its wait`, async () => {
  const f = await documentFixture(), before = f.calls.length;
  await heldLock(f, async release => {
    const waiting = operation === 'caption'
      ? f.card.projectCaption(f.store, f.context, f.opened.agent, DEFAULT_CONFIG)
      : f.a.c.projectSummaries(f.store, f.a.context, f.opened.agent, DEFAULT_CONFIG);
    await Promise.all([waiting, delay(60).then(async () => {
      if (operation === 'caption') f.panes.find(p => p.terminal_id === f.context.terminalId).label = f.workspace.record.title;
      else f.panes[0].tokens.wca_status = 'active';
      await release();
    })]);
  });
  assert.deepEqual(effects(f.calls.slice(before)), []);
});

for (const operation of ['caption', 'summary']) test(`${operation} waits and publishes fresh values after transient contention`, async () => {
  const f = await documentFixture(), before = f.calls.length;
  await heldLock(f, async release => {
    const waiting = operation === 'caption'
      ? f.card.projectCaption(f.store, f.context, f.opened.agent, DEFAULT_CONFIG)
      : f.a.c.projectSummaries(f.store, f.a.context, f.opened.agent, DEFAULT_CONFIG);
    await Promise.all([waiting, delay(60).then(async () => {
      assert.deepEqual(effects(f.calls.slice(before)), []);
      const workspace = await f.store.readWorkspace(f.workspace.record.id);
      if (operation === 'caption') await f.store.setWorkspaceTitle(agentAuthority(f.opened.agent.record), 'Title after waiting', workspace.revision);
      else {
        await f.store.applyPlan(agentAuthority(f.opened.agent.record), { goal: evaluatedGoal('Goal after waiting', [f.target('plan.md')], 'waited') }, workspace.revision);
        const agent = await f.store.readAgent('term1');
        await f.store.updateOwn(agentAuthority(agent.record), { now: 'Progress after waiting' }, agent.revision);
      }
      await release();
    })]);
  });
  if (operation === 'caption') assert.equal(f.panes.find(p => p.terminal_id === f.context.terminalId).label, 'Title after waiting');
  else {
    assert.equal(f.panes[0].tokens.wca_now, 'Progress after waiting');
    assert.equal(f.workspaces[0].tokens.wcw_goal, 'Goal after waiting');
  }
  await f.a.c.verifyMembership(f.store, f.a.context, f.opened.agent, DEFAULT_CONFIG);
});

test('summary stamp rereads records before effects and rejects changes during final preparation', async () => {
  const f = await documentFixture(), stamp = f.a.c.stamp.bind(f.a.c); let once = true;
  f.a.c.stamp = async (...args) => {
    if (once) {
      once = false;
      const current = await f.store.readAgent('term1');
      await f.store.resetEvidence(agentAuthority(current.record), boundary, current.revision);
    }
    return stamp(...args);
  };
  const before = f.calls.length;
  await assert.rejects(f.a.c.projectSummaries(f.store, f.a.context, f.opened.agent, DEFAULT_CONFIG), /partial.*stale/i);
  assert.deepEqual(effects(f.calls.slice(before)), []);
});

test('summary workspace failure retains published agent progress without rollback', async () => {
  const f = await documentFixture(), workspace = await f.store.readWorkspace(f.workspace.record.id);
  await f.store.applyPlan(agentAuthority(f.opened.agent.record), { goal: evaluatedGoal('Shared purpose', [f.target('plan.md')], 'goal') }, workspace.revision);
  f.setFail(a => a[0] === 'workspace' && a[1] === 'report-metadata');
  await assert.rejects(f.a.c.projectSummaries(f.store, f.a.context, f.opened.agent, DEFAULT_CONFIG), /partial/i);
  assert.equal(f.panes[0].tokens.wca_status, 'active');
  assert.equal(f.workspaces[0].tokens.wcw_goal, undefined);
  assert.equal(f.calls.some(a => a[0] === 'pane' && a[1] === 'rename'), false);
});

for (const invalid of ['abort', 'move', 'binding', 'evidence', 'attempt']) test(`summary authority rechecked before workspace publication: ${invalid}`, async () => {
  const f = await documentFixture(), controller = new AbortController(), run = f.a.c.run;
  const workspace = await f.store.readWorkspace(f.workspace.record.id);
  await f.store.applyPlan(agentAuthority(f.opened.agent.record), { goal: evaluatedGoal('Shared purpose', [f.target('plan.md')], 'goal') }, workspace.revision);
  f.a.c.run = async args => {
    const result = await run(args);
    if (args[0] === 'pane' && args[1] === 'report-metadata') {
      if (invalid === 'abort') controller.abort();
      if (invalid === 'move') f.panes[0].tab_id = 'w:t2';
      if (invalid === 'binding') f.workspaces[0].tokens.wcw_nonce = 'replacement';
      const agent = await f.store.readAgent('term1');
      if (invalid === 'evidence') await f.store.resetEvidence(agentAuthority(agent.record), boundary, agent.revision);
      if (invalid === 'attempt') await f.store.setDisplayAttempt(agentAuthority(agent.record), { attempt: 'new', nonce: 'new' }, agent.revision);
    }
    return result;
  };
  await assert.rejects(f.a.c.projectSummaries(f.store, f.a.context, f.opened.agent, DEFAULT_CONFIG, controller.signal), /partial/i);
  assert.equal(f.panes[0].tokens.wca_status, 'active');
  assert.equal(f.workspaces[0].tokens.wcw_goal, undefined);
});

test('document environment forwards only required target, command and explicit caller PATH', async t => {
  const f = await guardedDocumentFixture(t); f.calls.length = 0;
  f.client.env.PATH = '/caller bin:/usr/bin:/bin'; f.client.env.SECRET_TEST_VALUE = 'not forwarded'; f.client.env.EDITOR = 'nano -w';
  await f.open();
  const open = f.calls.find(a => a.slice(0, 3).join(' ') === 'plugin pane open');
  const values = open.flatMap((value, index) => value === '--env' ? [open[index + 1]] : []);
  assert.deepEqual(values.map(v => v.split('=')[0]).sort(), ['PATH', 'WORK_CARDS_DOCUMENT_COMMAND', 'WORK_CARDS_DOCUMENT_CWD', 'WORK_CARDS_DOCUMENT_PATH']);
  assert.ok(values.includes('PATH=/caller bin:/usr/bin:/bin'));
  assert.ok(values.includes('WORK_CARDS_DOCUMENT_COMMAND=nano -w'));
  assert.ok(values.includes('WORK_CARDS_DOCUMENT_PATH=' + join(f.cwd, 'plan.md')));
});
