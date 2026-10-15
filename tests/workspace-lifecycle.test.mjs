import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, realpath, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import { RecordStore } from '../dist/records.js';
import { DEFAULT_CONFIG } from '../dist/config.js';
import { WorkspaceClient } from '../dist/herdr/workspace-client.js';
import { agentAuthority } from '../dist/records.js';
import { WorkspaceLifecycle } from '../dist/pi/workspace-lifecycle.js';
import { projectSharedEvidence } from '../dist/pi/shared-evidence.js';

const evidence = (...texts) => ({ compactionSummary: null, transcript: texts.map(text=>({role:'user',text})) });
function deferred() { let resolve; const promise=new Promise(r=>{resolve=r;}); return {promise,resolve}; }
async function fixture(t, policy='automatic') {
  const root=await realpath(await mkdtemp(join(tmpdir(),'wc-lifecycle-')));
  t.after(()=>rm(root,{recursive:true,force:true}));
  const cwd=join(root,'checkout'); await mkdir(cwd);
  const store=new RecordStore(join(root,'state')), socket=join(root,'h.sock');
  const workspaces=['w','dest'].map(workspace_id=>({workspace_id,tokens:{}}));
  const tabs=['w','dest'].map(workspace_id=>({workspace_id,tab_id:`${workspace_id}:tab`,number:1,label:'1'}));
  const panes=[{pane_id:'w:pi',terminal_id:'pi',workspace_id:'w',tab_id:'w:tab',tokens:{}}];
  const calls=[]; let next=1, hook=async()=>{};
  const run=async args=>{
    calls.push(args); await hook(args);
    const result=(type,fields)=>JSON.stringify({result:{type,...fields}});
    if(args[0]==='status') return JSON.stringify({server:{running:true,compatible:true,endpoint_compatible:true,version:'0.9.4',socket}});
    if(args[0]==='api') return result('session_snapshot',{snapshot:{workspaces,tabs,panes,layouts:tabs.map(tab=>({...tab,panes:panes.filter(p=>p.tab_id===tab.tab_id).map(p=>({pane_id:p.pane_id}))}))}});
    if(args[0]==='pane'&&args[1]==='current') return result('pane_current',{pane:panes.find(p=>p.terminal_id==='pi')});
    if(args[0]==='pane'&&args[1]==='resize') return result('pane_resize',{resize:{pane_id:args[3]}});
    if(args[1]==='report-metadata') {
      const target=(args[0]==='pane'?panes:workspaces).find(p=>(p.pane_id??p.workspace_id)===args[2]);
      for(let i=5;i<args.length;i+=2) { const [key,...values]=args[i+1].split('='); if(args[i]==='--clear-token') delete target.tokens[key]; else target.tokens[key]=values.join('='); }
      return '';
    }
    if(args[0]==='plugin'&&args[1]==='list') return result('plugin_list',{plugins:[]});
    if(args[0]==='plugin'&&args[1]==='link') return result('plugin_linked',{plugin:{plugin_id:'work-cards',enabled:true,plugin_root:root,manifest_path:join(root,'herdr-plugin.toml'),source:{kind:'local'},version:'0.1.0',min_herdr_version:'0.9.3',platforms:['macos','linux'],panes:[{id:'card',title:'Work card',placement:'split',command:['node','dist/herdr/pane.js']},{id:'document',title:'Document',placement:'tab',command:['/bin/sh','bin/open-document.sh']}]}});
    if(args[0]==='plugin'&&args[2]==='open') {
      const owner=panes.find(p=>p.pane_id===args[args.indexOf('--target-pane')+1]);
      const p={...owner,pane_id:`card${next}`,terminal_id:`card${next++}`,tokens:{}}; panes.push(p);
      return result('plugin_pane_opened',{plugin_pane:{plugin_id:'work-cards',entrypoint:'card',pane:p}});
    }
    if(args[0]==='plugin'&&args[2]==='close') { panes.splice(panes.findIndex(p=>p.pane_id===args[3]),1); return result('plugin_pane_closed',{pane_id:args[3]}); }
    throw new Error(`Unexpected ${args.join(' ')}`);
  };
  const client=new WorkspaceClient({env:{HERDR_ENV:'1',HERDR_PANE_ID:'inherited-public-id',HERDR_SOCKET_PATH:socket},pluginRoot:root,run});
  let session={sessionId:'session',cwd,evidence:evidence()}, cancellations=0;
  const options={store,client,config:DEFAULT_CONFIG,policy,current:()=>session,onInvalidate:()=>{cancellations++;}};
  const lifecycle=new WorkspaceLifecycle(options);
  const move=(both=false)=>{for(const p of panes.filter(p=>p.terminal_id==='pi'||both)) {p.workspace_id='dest';p.tab_id='dest:tab';p.pane_id=`dest:${p.terminal_id}`;}};
  const q=()=>{const i=panes.findIndex(p=>p.tokens.wca_role==='card');if(i>=0)panes.splice(i,1);};
  return {store,client,lifecycle,options,panes,workspaces,tabs,run,calls,move,q,cwd,
    get session(){return session;},set session(value){session=value;},get cancellations(){return cancellations;},setHook:f=>{hook=f;},
    launches:()=>calls.filter(a=>a[0]==='plugin'&&a[2]==='open').length,
    fresh:()=>{session={...session,evidence:evidence('Implement the library API')};},
  };
}
async function attached(t,policy='automatic') { const f=await fixture(t,policy); await f.lifecycle.restore(); f.fresh(); if(policy==='manual') await f.lifecycle.open('Library task'); else await f.lifecycle.reconcile(); return f; }

test('lifecycle state requires verified ready membership for active authority in its TypeScript API', () => {
  const filename = fileURLToPath(new URL('../src/workspace-lifecycle-typecheck.ts', import.meta.url));
  const source = `import type { WorkspaceLifecycleState } from './pi/workspace-lifecycle.js';
    import type { Membership, ReadyMembership, WorkspaceClient } from './herdr/workspace-client.js';
    declare const membership: ReadyMembership, unverified: Membership;
    const active: WorkspaceLifecycleState = { kind: 'active', membership };
    const ready: 'ready' = membership.binding.phase;
    const nonce: string = membership.context.bindingNonce;
    declare const verified: Awaited<ReturnType<WorkspaceClient['verifyMembership']>>;
    const result: ReadyMembership = verified;
    // @ts-expect-error Ready membership cannot carry a reserved binding.
    const reservedBinding: ReadyMembership = { ...membership, binding: { ...membership.binding, phase: 'reserved' } };
    // @ts-expect-error Ready membership requires a native binding nonce.
    const missingNonce: ReadyMembership = { ...membership, context: { socket: '', terminalId: '', workspaceId: '', tabId: '' } };
    // @ts-expect-error Active authority cannot omit membership.
    const missing: WorkspaceLifecycleState = { kind: 'active' };
    // @ts-expect-error An agent snapshot alone is not active authority.
    const agentOnly: WorkspaceLifecycleState = { kind: 'active', agent: membership.agent };
    // @ts-expect-error An unverified/reserved membership cannot authorize active state.
    const reserved: WorkspaceLifecycleState = { kind: 'active', membership: unverified };
    // @ts-expect-error Restored state requires its agent snapshot.
    const restored: WorkspaceLifecycleState = { kind: 'restored' };
    // @ts-expect-error Suspended state cannot retain active membership.
    const suspended: WorkspaceLifecycleState = { kind: 'suspended', membership };
    // @ts-expect-error Disposed state cannot retain active membership.
    const disposed: WorkspaceLifecycleState = { kind: 'disposed', membership };`;
  const configPath = fileURLToPath(new URL('../tsconfig.json', import.meta.url));
  const config = ts.readConfigFile(configPath, ts.sys.readFile);
  const { options } = ts.parseJsonConfigFileContent(config.config, ts.sys, fileURLToPath(new URL('..', import.meta.url)));
  const host = ts.createCompilerHost(options), getSourceFile = host.getSourceFile.bind(host);
  host.getSourceFile = (path, ...args) => path === filename ? ts.createSourceFile(path, source, options.target, true) : getSourceFile(path, ...args);
  const diagnostics = ts.getPreEmitDiagnostics(ts.createProgram([filename], { ...options, noEmit: true }, host));
  assert.equal(diagnostics.length, 0, diagnostics.map(d => ts.flattenDiagnosticMessageText(d.messageText, '\n')).join('\n'));
});

test('active state owns the sole membership snapshot and last-known agent is derived', async t => {
  const f = await attached(t);
  assert.equal(f.lifecycle.state.kind, 'active');
  assert.equal(f.lifecycle.state.membership.agent, f.lifecycle.lastKnownAgent);
  assert.equal(Object.hasOwn(f.lifecycle.state, 'agent'), false);
  assert.equal(f.lifecycle.state.membership.binding.phase, 'ready');
  assert.equal(f.lifecycle.state.membership.context.bindingNonce, f.lifecycle.state.membership.binding.nonce);
  const agent = f.lifecycle.lastKnownAgent;
  f.options.onInvalidate = () => assert.equal(f.lifecycle.captureScope(), undefined);
  f.lifecycle.invalidate();
  assert.equal(f.lifecycle.state.kind, 'suspended');
  assert.equal(f.lifecycle.lastKnownAgent, agent);
  assert.equal(f.lifecycle.captureScope(), undefined);
  assert.equal(Object.hasOwn(f.lifecycle.state, 'membership'), false);
  f.lifecycle.dispose();
  assert.equal(f.lifecycle.state.kind, 'disposed');
  assert.equal(f.lifecycle.lastKnownAgent, agent);
  assert.equal(f.lifecycle.captureScope(), undefined);
});

test('blank, inherited and assistant-only sessions allocate nothing; fresh input retains START boundary', async t=>{
  const f=await fixture(t); f.session={...f.session,evidence:evidence('inherited task')};
  await f.lifecycle.restore(); await f.lifecycle.reconcile();
  f.session={...f.session,evidence:{...f.session.evidence,transcript:[...f.session.evidence.transcript,{role:'assistant',text:'tool result paraphrase'}]}};
  await f.lifecycle.reconcile(); assert.equal(f.calls.length,0); assert.deepEqual(await f.store.listAgents(),[]);
  f.session={...f.session,evidence:{...f.session.evidence,transcript:[...f.session.evidence.transcript,{role:'user',text:'\u001b Fresh task\nplease'}]}};
  await f.lifecycle.reconcile(); const a=await f.store.readAgent('session');
  assert.equal(a.record.boundary.transcriptLength,1); assert.equal(a.record.title,'Fresh task please');
  const shared=projectSharedEvidence(f.session.evidence,a.record.boundary,'continue');
  assert.equal(shared.kind,'suffix'); assert.ok(shared.evidence.transcript.some(m=>m.text.includes('Fresh task')));
});
test('explicit durable selection binds only an unbound native group and reuses no historical suffix', async t => {
  const f = await fixture(t); await f.lifecycle.restore(); f.fresh();
  await f.store.createWorkspace({ id: 'chosen', creationToken: 'chosen-token', title: 'Chosen workspace' });
  await f.lifecycle.open(undefined, 'chosen');
  assert.equal((await f.store.readAgent('session')).record.workspaceId, 'chosen');
  assert.equal(f.workspaces[0].tokens.wcw_id, 'chosen');
  const nonce = f.workspaces[0].tokens.wcw_nonce;
  await f.lifecycle.open(undefined, 'chosen'); assert.equal(f.launches(), 1);
  await f.store.createWorkspace({ id: 'other', creationToken: 'other-token', title: 'Other' });
  await assert.rejects(f.lifecycle.open(undefined, 'other'), /bound|replace|conflict/i);
  assert.equal(f.workspaces[0].tokens.wcw_id, 'chosen');
  f.move(true);
  await assert.rejects(f.lifecycle.open(undefined, 'chosen'), /already|duplicate|live/i);
  for (const key of Object.keys(f.workspaces[0].tokens)) delete f.workspaces[0].tokens[key];
  await f.lifecycle.open(undefined, 'chosen');
  const a = await f.store.readAgent('session');
  assert.notEqual(f.workspaces[1].tokens.wcw_nonce, nonce);
  assert.equal(a.record.boundary.transcriptLength, 1);
});

test('blank explicit open seeds an automatic title; reconciliation stays idempotent',async t=>{
  const f=await fixture(t); await f.lifecycle.restore(); await f.lifecycle.open();
  const agent=await f.store.readAgent('session');
  assert.equal(agent.record.title,'Untitled task'); assert.equal(agent.record.titleMode,'auto');
  await f.lifecycle.reconcile(); await f.lifecycle.reconcile();
  assert.equal(f.launches(),1); assert.equal((await f.store.listAgents()).length,1);
  await assert.rejects(f.lifecycle.open(' \t '),/title|dialogue/i);
});
test('explicit unnamed open preserves the startup boundary for continuity checks',async t=>{
  const f=await fixture(t); f.session={...f.session,evidence:evidence('Inherited task with old references')};
  await f.lifecycle.restore(); await f.lifecycle.open();
  const agent=await f.store.readAgent('session');
  assert.equal(agent.record.titleMode,'auto'); assert.equal(agent.record.boundary.transcriptLength,1);
  assert.deepEqual(projectSharedEvidence(f.session.evidence,agent.record.boundary,'continue'),{kind:'suffix',evidence:{transcript:[]}});
});
test('manual startup waits for explicit open; restore can summarize and observed moves remain suspended',async t=>{
  const f=await fixture(t,'manual'); await f.lifecycle.restore(); f.fresh(); await f.lifecycle.reconcile(); assert.equal(f.calls.length,0);
  await f.lifecycle.open('Manual task'); const source=(await f.store.readAgent('session')).record.workspaceId;
  const scope=f.lifecycle.captureScope(); assert.ok(scope);
  f.move(); await f.lifecycle.reconcile(); assert.equal(f.lifecycle.state.kind,'suspended'); assert.equal(f.lifecycle.captureScope(),undefined);
  await assert.rejects(f.lifecycle.assertCurrent(scope)); await f.lifecycle.reconcile();
  assert.equal((await f.store.readAgent('session')).record.workspaceId,source);
  await f.lifecycle.open(); assert.notEqual((await f.store.readAgent('session')).record.workspaceId,source);
});
test('resume and tree reset even matching prefixes; compaction and prefix discontinuity reset generations',async t=>{
  const f=await attached(t); let a=await f.store.readAgent('session');
  for(const change of ['resume','tree','compaction','prefix']) {
    const before=a.record.evidenceGeneration, scope=f.lifecycle.captureScope();
    if(change==='resume') await f.lifecycle.restore();
    if(change==='tree') await f.lifecycle.reset();
    if(change==='compaction') f.session={...f.session,evidence:{...f.session.evidence,compactionSummary:'checkpoint'}};
    if(change==='prefix') f.session={...f.session,evidence:evidence('different branch')};
    await f.lifecycle.reconcile(); a=await f.store.readAgent('session');
    assert.notEqual(a.record.evidenceGeneration,before); assert.equal(a.record.boundary.transcriptLength,f.session.evidence.transcript.length);
    await assert.rejects(f.lifecycle.assertCurrent(scope));
  }
});
test('move keeps own progress and display attempt, never copies source references; both terminals reuse pair',async t=>{
  for(const both of [false,true]) {
    const f=await attached(t); let a=await f.store.readAgent('session'); const source=a.record.workspaceId, oldNonce=a.record.displayAttempt.nonce;
    await f.store.updateOwn(agentAuthority(a.record),{now:'Own progress',remaining:'Own todo',status:'blocked'},a.revision);
    await f.store.addReferences(agentAuthority(a.record),{issues:[{url:'https://example.com/source',description:'source'}]});
    const scope=f.lifecycle.captureScope(); f.move(both); await f.lifecycle.reconcile(); a=await f.store.readAgent('session');
    assert.notEqual(a.record.workspaceId,source); assert.equal(a.record.now,'Own progress'); assert.equal(a.record.remaining,'Own todo'); assert.equal(a.record.status,'blocked');
    assert.deepEqual((await f.store.readWorkspace(a.record.workspaceId)).record.issues,[]);
    assert.equal(a.record.boundary.transcriptLength,1); assert.equal(f.launches(),both?1:2);
    if(both) assert.equal(a.record.displayAttempt.nonce,oldNonce);
    await assert.rejects(f.lifecycle.assertCurrent(scope));
  }
});
test('move samples evidence after awaited live observation, not queued input',async t=>{
  const f=await attached(t); const entered=deferred(), release=deferred(); let once=true;
  f.move(true); f.setHook(async args=>{if(args[0]==='api'&&once){once=false;entered.resolve();await release.promise;}});
  const operation=f.lifecycle.reconcile(); await entered.promise;
  f.session={...f.session,evidence:evidence('Implement the library API','Before move detection')}; release.resolve(); await operation;
  assert.equal((await f.store.readAgent('session')).record.boundary.transcriptLength,2);
});
test('q suppresses reload and move respawn; explicit reopen creates a new attempt',async t=>{
  const f=await attached(t); f.q(); await f.lifecycle.reconcile(); assert.ok(f.lifecycle.captureScope());
  const reloaded=new WorkspaceLifecycle(f.options); await reloaded.restore(); await reloaded.reconcile(); assert.equal(f.launches(),1);
  f.move(); await reloaded.reconcile(); assert.equal(f.launches(),1); assert.ok(reloaded.captureScope());
  await reloaded.open(); assert.equal(f.launches(),2);
});
test('actual UUID switch retires known live or q-closed display; fork inherits no assignment',async t=>{
  for(const closed of [false,true]) {
    const f=await attached(t); let a=await f.store.readAgent('session'); await f.store.updateOwn(agentAuthority(a.record),{now:'parent work'},a.revision);
    if(closed) f.q(); f.session={...f.session,sessionId:'fork'};
    const before=f.launches(), oldScope=f.lifecycle.captureScope(); await f.lifecycle.restore(); await f.lifecycle.reconcile();
    assert.equal(f.launches(),before); assert.equal(f.panes[0].tokens.wca_agent,undefined);
    await assert.rejects(f.store.readAgent('fork'),e=>e.code==='ENOENT');
    f.session={...f.session,evidence:evidence('Implement the library API','New fork task')}; await f.lifecycle.reconcile();
    assert.equal((await f.store.readAgent('fork')).record.now,''); assert.equal((await f.store.readAgent('session')).record.now,'parent work');
    await assert.rejects(f.lifecycle.assertCurrent(oldScope));
  }
});
test('unknown prior owner on a fresh controller fails closed without enumeration or takeover',async t=>{
  const f=await attached(t); f.session={...f.session,sessionId:'unknown-fork'}; const fresh=new WorkspaceLifecycle(f.options); await fresh.restore();
  f.session={...f.session,evidence:evidence('Implement the library API','Fresh unknown task')};
  await assert.rejects(fresh.reconcile(),/owner|marker|foreign/i); assert.equal(f.launches(),1);
  await assert.rejects(f.store.readAgent('unknown-fork'),e=>e.code==='ENOENT');
});
test('slow reconciliation is serialized; queued work and awaited probes are invalidated synchronously',async t=>{
  const f=await attached(t); const entered=deferred(), release=deferred(); let probes=0, max=0, once=true;
  f.setHook(async args=>{if(args[0]==='api'){probes++;max=Math.max(max,probes);if(once){once=false;entered.resolve();await release.promise;}probes--;}});
  const scope=f.lifecycle.captureScope(), first=f.lifecycle.reconcile(); await entered.promise;
  const second=f.lifecycle.reconcile(); const prior=f.cancellations; f.lifecycle.invalidate(); assert.ok(f.cancellations>prior); assert.ok(scope.signal.aborted);
  release.resolve(); await assert.rejects(first); await assert.rejects(second); assert.equal(max,1);
  await f.lifecycle.restore(); await f.lifecycle.reconcile(); assert.ok(f.lifecycle.captureScope());
});
test('summary scope rejects live moves, lost/foreign bindings, and session changes without a restore callback',async t=>{
  for(const change of ['move','missing','binding','session']) {
    const f=await attached(t), scope=f.lifecycle.captureScope();
    if(change==='move') f.move();
    if(change==='missing') f.panes.splice(0,1);
    if(change==='binding') f.workspaces[0].tokens.wcw_token='foreign';
    if(change==='session') f.session={...f.session,sessionId:'new'};
    await assert.rejects(f.lifecycle.assertCurrent(scope)); assert.equal(f.lifecycle.captureScope(),undefined);
  }
});
test('partial launch retains a current attempt snapshot and never auto retries',async t=>{
  const f=await fixture(t); await f.lifecycle.restore(); f.fresh();
  f.setHook(async args=>{if(args[0]==='plugin'&&args[2]==='open')throw new Error('uncertain launch');});
  await assert.rejects(f.lifecycle.reconcile(),/partial/i);
  assert.equal(f.lifecycle.state.kind,'suspended'); assert.equal(f.lifecycle.captureScope(),undefined);
  assert.ok(f.lifecycle.lastKnownAgent.record.displayAttempt); f.setHook(async()=>{});
  await f.lifecycle.reconcile(); await f.lifecycle.reconcile(); assert.equal(f.launches(),1);
  await f.lifecycle.open(); assert.equal(f.launches(),2);
});
test('retirement failure retains scoped target for explicit retry and invalidates before cleanup',async t=>{
  const f=await attached(t), scope=f.lifecycle.captureScope(); f.session={...f.session,sessionId:'fork'};
  f.setHook(async args=>{if(args[0]==='plugin'&&args[2]==='close'){assert.ok(scope.signal.aborted);throw new Error('cleanup failed');}});
  await assert.rejects(f.lifecycle.restore(),/partial/i); assert.equal(f.lifecycle.captureScope(),undefined);
  f.setHook(async()=>{}); await f.lifecycle.open('Fork assignment'); assert.equal(f.launches(),2);
});
test('successful-switch retirement works across controller replacement, retries scoped failure and does not reopen q',async t=>{
  for(const closed of [false,true]) {
    const f=await attached(t), scope=f.lifecycle.captureScope(); if(closed)f.q();
    let fail=true;
    f.setHook(async args=>{if(args[0]==='plugin'&&args[2]==='close'&&fail){assert.ok(scope.signal.aborted);throw new Error('retire close failed');}});
    if(!closed) await assert.rejects(f.lifecycle.retire(),/partial/i);
    fail=false; await f.lifecycle.retire(); assert.equal(f.launches(),1); assert.equal(f.panes[0].tokens.wca_agent,undefined);
    await assert.rejects(f.lifecycle.reconcile(),/restore/i);
    f.session={...f.session,sessionId:'replacement'};
    const next=new WorkspaceLifecycle(f.options); await next.restore(); await next.open('Replacement task');
    assert.equal(f.launches(),2); assert.ok(await f.store.readAgent('session'));
  }
});
test('two lifecycle controllers share a native group but never overwrite another agent progress',async t=>{
  const f=await attached(t);
  f.tabs.push({workspace_id:'w',tab_id:'w:second',number:2,label:'2'});
  const secondPane={pane_id:'w:second-pi',terminal_id:'second-pi',workspace_id:'w',tab_id:'w:second',tokens:{}}; f.panes.push(secondPane);
  const client=new WorkspaceClient({env:f.client.env,pluginRoot:f.client.pluginRoot,run:async args=>args[0]==='pane'&&args[1]==='current'?JSON.stringify({result:{type:'pane_current',pane:secondPane}}):f.run(args)});
  let current={...f.session,sessionId:'second-session',evidence:evidence()};
  const second=new WorkspaceLifecycle({...f.options,client,current:()=>current}); await second.restore();
  current={...current,evidence:evidence('Second task')}; await second.reconcile();
  const a=await f.store.readAgent('session'), b=await f.store.readAgent('second-session');
  assert.equal(a.record.workspaceId,b.record.workspaceId);
  await f.store.updateOwn(agentAuthority(a.record),{now:'First work'},a.revision);
  await f.store.updateOwn(agentAuthority(b.record),{now:'Second work'},b.revision);
  await f.lifecycle.reconcile(); await second.reconcile();
  assert.equal(f.lifecycle.lastKnownAgent.record.now,'First work'); assert.equal(second.lastKnownAgent.record.now,'Second work'); assert.equal(f.launches(),2);
});
test('failed in-Herdr observation is not offline authorization; binding loss cannot silently rebind active group',async t=>{
  const f=await attached(t), id=(await f.store.readAgent('session')).record.workspaceId;
  for(const key of Object.keys(f.workspaces[0].tokens)) delete f.workspaces[0].tokens[key];
  await assert.rejects(f.lifecycle.reconcile(),/binding|lifetime/i);
  assert.equal((await f.store.readAgent('session')).record.workspaceId,id); assert.equal((await f.store.listWorkspaces()).length,1);
  assert.equal(f.lifecycle.captureScope(),undefined); assert.ok(f.lifecycle.lastKnownAgent);
});
test('session change detected by reconciliation cancels the previous scope before refusing work',async t=>{
  const f=await attached(t), scope=f.lifecycle.captureScope(); f.session={...f.session,sessionId:'unrestored'};
  await assert.rejects(f.lifecycle.reconcile(),/restore/i); assert.ok(scope.signal.aborted);
});
test('cancellation during join or display launch prevents remaining effects; restore recovers conservatively',async t=>{
  for(const stage of ['ready','launch']) {
    const f=await fixture(t); await f.lifecycle.restore(); f.fresh(); let once=true;
    f.setHook(async args=>{
      const matches=stage==='ready'?args[1]==='report-metadata'&&args.includes('wcw_phase=ready'):args[0]==='plugin'&&args[2]==='open';
      if(matches&&once){once=false;f.lifecycle.invalidate();}
    });
    await assert.rejects(f.lifecycle.reconcile()); assert.equal(f.lifecycle.captureScope(),undefined);
    f.setHook(async()=>{}); await f.lifecycle.restore();
    if(stage==='launch') {
      // The returned pane has no completed ownership tuple; refuse adoption, never retry launch.
      await f.lifecycle.reconcile(); assert.equal(f.launches(),1);
    } else { await f.lifecycle.reconcile(); assert.equal(f.launches(),1); }
  }
});
test('failed restore cannot reactivate the old evidence generation through a later poll',async t=>{
  const f=await attached(t), before=(await f.store.readAgent('session')).record.evidenceGeneration;
  const reset=f.store.resetEvidence.bind(f.store); let once=true;
  f.store.resetEvidence=async(...args)=>{if(once){once=false;throw new Error('reset failed');}return reset(...args);};
  await assert.rejects(f.lifecycle.restore(),/reset failed/); assert.equal(f.lifecycle.captureScope(),undefined);
  await f.lifecycle.reconcile(); const current=await f.store.readAgent('session');
  assert.notEqual(current.record.evidenceGeneration,before); assert.equal(current.record.boundary.transcriptLength,1);
});
test('bootstrap failure never allocates a guessed destination, and manual resume still authorizes own summaries',async t=>{
  const f=await fixture(t); await f.lifecycle.restore(); f.fresh();
  f.setHook(async args=>{if(args[0]==='pane'&&args[1]==='current')throw new Error('bootstrap unavailable');});
  await assert.rejects(f.lifecycle.reconcile(),/bootstrap unavailable/); assert.equal(f.lifecycle.captureScope(),undefined);
  assert.deepEqual(await f.store.listAgents(),[]); assert.deepEqual(await f.store.listWorkspaces(),[]);
  f.setHook(async()=>{}); await f.lifecycle.reconcile();
  const manual=new WorkspaceLifecycle({...f.options,policy:'manual'}); await manual.restore(); await manual.reconcile();
  const scope=manual.captureScope(); assert.ok(scope); await manual.assertCurrent(scope); assert.equal(f.launches(),1);
});
test('manual same-group moves reverify after evidence reset without automatic display replacement',async t=>{
  const f=await attached(t,'manual');
  f.tabs.push({workspace_id:'w',tab_id:'w:moved-tab',number:2,label:'2'}); f.panes[0].tab_id='w:moved-tab';
  f.session={...f.session,evidence:{...f.session.evidence,compactionSummary:'new checkpoint'}};
  await f.lifecycle.reconcile(); assert.equal(f.lifecycle.state.kind,'active'); assert.ok(f.lifecycle.captureScope()); assert.equal(f.launches(),1);
  await f.lifecycle.open(); assert.ok(f.lifecycle.captureScope()); assert.equal(f.launches(),2);
});
test('matching reserved joins recover through reconcile, explicit open and restore, including manual policy',async t=>{
  for(const policy of ['automatic','manual']) for(const retry of ['reconcile','open','restore']) await t.test(`${policy} ${retry}`,async t=>{
    const f=await fixture(t,policy); await f.lifecycle.restore(); f.fresh();
    f.setHook(async args=>{if(args[1]==='report-metadata'&&args.includes('wcw_phase=ready'))throw new Error('before ready write');});
    await assert.rejects(policy==='manual'?f.lifecycle.open('Explicit task'):f.lifecycle.reconcile(),/partial/i);
    const before=await f.store.readAgent('session'), tokens={...f.workspaces[0].tokens};
    assert.equal(tokens.wcw_phase,'reserved'); assert.equal(before.record.workspaceId,tokens.wcw_id); assert.equal(f.lifecycle.captureScope(),undefined);
    f.setHook(async()=>{});
    if(retry==='restore'){await f.lifecycle.restore();await f.lifecycle.reconcile();}else await f.lifecycle[retry]();
    const after=await f.store.readAgent('session'); assert.equal(f.lifecycle.state.kind,'active');
    assert.equal(after.record.membershipToken,before.record.membershipToken); assert.equal(after.record.workspaceId,before.record.workspaceId);
    assert.equal(f.workspaces[0].tokens.wcw_token,tokens.wcw_token); assert.equal(f.workspaces[0].tokens.wcw_phase,'ready'); assert.equal((await f.store.listWorkspaces()).length,1);
  });
});
test('matching reserved move recovery retains q suppression and rejects foreign reservation metadata',async t=>{
  for(const foreign of [false,true]) await t.test(String(foreign),async t=>{
    const f=await attached(t); f.q(); const attempt=(await f.store.readAgent('session')).record.displayAttempt; f.move();
    f.setHook(async args=>{if(args[1]==='report-metadata'&&args.includes('wcw_phase=ready'))throw new Error('before ready write');});
    await assert.rejects(f.lifecycle.reconcile(),/partial/i); f.setHook(async()=>{});
    if(foreign) {
      f.workspaces[1].tokens.wcw_token='foreign-token';
      await assert.rejects(f.lifecycle.reconcile(),/token|foreign|partial/i); assert.equal(f.workspaces[1].tokens.wcw_phase,'reserved');
    } else {await f.lifecycle.reconcile();assert.equal(f.lifecycle.state.kind,'active');}
    assert.equal(f.launches(),1); assert.equal((await f.store.listWorkspaces()).length,2);
    assert.deepEqual((await f.store.readAgent('session')).record.displayAttempt,attempt);
  });
});
test('manual paired same-workspace tab moves invalidate scope without suspending attached summaries',async t=>{
  const f=await attached(t,'manual'), before=await f.store.readAgent('session'), scope=f.lifecycle.captureScope();
  f.tabs.push({workspace_id:'w',tab_id:'w:paired-move',number:2,label:'2'});
  for(const pane of f.panes){pane.tab_id='w:paired-move';pane.pane_id=`moved:${pane.terminal_id}`;}
  await f.lifecycle.reconcile(); assert.ok(scope.signal.aborted); assert.equal(f.lifecycle.state.kind,'active');
  const current=f.lifecycle.captureScope(); await f.lifecycle.assertCurrent(current);
  assert.equal(current.context.tabId,'w:paired-move'); assert.equal(f.launches(),1);
  assert.equal((await f.store.readAgent('session')).record.membershipToken,before.record.membershipToken);
  f.move(true); await f.lifecycle.reconcile(); assert.equal(f.lifecycle.state.kind,'suspended'); assert.equal(f.lifecycle.captureScope(),undefined);
  assert.equal((await f.store.readAgent('session')).record.workspaceId,before.record.workspaceId);
});
test('retirement refreshes only captured session after a committed move is cancelled during ready publication',async t=>{
  for(const retry of ['retire','restore']) await t.test(retry,async t=>{
    const f=await attached(t), before=await f.store.readAgent('session'); f.move(true);
    f.setHook(async args=>{if(args[1]==='report-metadata'&&args.includes('wcw_phase=ready'))f.lifecycle.invalidate();});
    await assert.rejects(f.lifecycle.reconcile()); const committed=await f.store.readAgent('session');
    assert.notEqual(committed.record.membershipToken,before.record.membershipToken); assert.equal(f.lifecycle.captureScope(),undefined);
    f.session={...f.session,sessionId:'replacement'};
    let fail=true; f.setHook(async args=>{if(args[0]==='plugin'&&args[2]==='close'&&fail){fail=false;throw new Error('retry cleanup');}});
    await assert.rejects(retry==='retire'?f.lifecycle.retire():f.lifecycle.restore(),/retry cleanup/);
    if(retry==='retire'){await f.lifecycle.retire();await f.lifecycle.retire();}
    await f.lifecycle.restore(); assert.equal(f.panes.some(p=>p.tokens.wca_role==='card'),false); assert.equal(f.panes[0].tokens.wca_agent,undefined);
    assert.equal(f.launches(),1); assert.equal((await f.store.readAgent('session')).record.membershipToken,committed.record.membershipToken);
    await assert.rejects(f.store.readAgent('replacement'),e=>e.code==='ENOENT');
  });
});
test('retirement refreshes cancelled evidence/attempt writes but refuses foreign owners and mismatched current nonce',async t=>{
  for(const stage of ['evidence','attempt','foreign','nonce']) await t.test(stage,async t=>{
    const f=await attached(t);
    if(stage==='evidence') {
      const reset=f.store.resetEvidence.bind(f.store);
      f.store.resetEvidence=async(...args)=>{const result=await reset(...args);f.lifecycle.invalidate();return result;};
      await assert.rejects(f.lifecycle.reset());
    } else if(stage==='attempt') {
      f.q(); f.setHook(async args=>{if(args[1]==='report-metadata'&&args.includes('wca_role=agent'))f.lifecycle.invalidate();});
      await assert.rejects(f.lifecycle.open()); f.setHook(async()=>{});
    } else if(stage==='foreign') f.panes[0].tokens.wca_agent='foreign-owner';
    else {const a=await f.store.readAgent('session');await f.store.setDisplayAttempt(agentAuthority(a.record),{attempt:'other',nonce:'other'},a.revision);}
    if(stage==='foreign'||stage==='nonce') {
      await assert.rejects(f.lifecycle.retire(),/foreign|nonce|owner/i); await assert.rejects(f.lifecycle.retire(),/foreign|nonce|owner/i);
      assert.equal(f.panes.some(p=>p.tokens.wca_role==='card'),true);
    } else {await f.lifecycle.retire();assert.equal(f.panes.some(p=>p.tokens.wca_role==='card'),false);}
  });
});
test('an unreadable record suspends authority even when failure recovery cannot reread it',async t=>{
  const f=await attached(t), scope=f.lifecycle.captureScope();
  f.store.readAgent=async()=>{throw new Error('records unavailable');};
  await assert.rejects(f.lifecycle.reconcile(),/records unavailable/);
  assert.ok(scope.signal.aborted); assert.equal(f.lifecycle.captureScope(),undefined); assert.equal(f.lifecycle.state.kind,'suspended');
});
test('unattached and restored disposal never authorizes a scope or drops disposed state', async t => {
  const f = await attached(t);
  for (const restore of [false, true]) {
    const lifecycle = new WorkspaceLifecycle(f.options);
    if (restore) {
      await lifecycle.restore();
      assert.equal(lifecycle.state.kind, 'restored');
      assert.ok(lifecycle.lastKnownAgent);
    }
    const agent = lifecycle.lastKnownAgent;
    lifecycle.dispose();
    assert.equal(lifecycle.state.kind, 'disposed');
    assert.equal(lifecycle.lastKnownAgent, agent);
    assert.equal(lifecycle.captureScope(), undefined);
    await assert.rejects(lifecycle.restore());
    lifecycle.invalidate();
    assert.equal(lifecycle.state.kind, 'disposed');
    assert.equal(lifecycle.captureScope(), undefined);
  }
});

test('disposal cancels work and cannot be reactivated; existing offline record is exposed but not authorized',async t=>{
  const f=await attached(t); const restored=new WorkspaceLifecycle(f.options); await restored.restore();
  assert.equal(restored.state.kind,'restored'); assert.ok(restored.state.agent); assert.equal(restored.captureScope(),undefined);
  const scope=f.lifecycle.captureScope(); f.lifecycle.dispose(); assert.ok(scope.signal.aborted);
  assert.equal(f.lifecycle.state.kind,'disposed'); assert.equal(f.lifecycle.captureScope(),undefined);
  await assert.rejects(f.lifecycle.reconcile()); await assert.rejects(f.lifecycle.restore());
  assert.equal(f.lifecycle.state.kind,'disposed'); assert.equal(f.lifecycle.captureScope(),undefined);
});
