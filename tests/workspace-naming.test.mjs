import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, rm, realpath, writeFile, readFile, symlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createHash } from 'node:crypto';
import { RecordStore, agentAuthority } from '../dist/records.js';
import { WorkspaceClient } from '../dist/herdr/workspace-client.js';
import { NameStore } from '../dist/herdr/name-provenance.js';
import { DEFAULT_CONFIG as config } from '../dist/config.js';

async function fixture(t) {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'native-naming-')));
  t.after(() => rm(root, {recursive:true,force:true}));
  const store = new RecordStore(join(root, 'state')); await mkdir(store.directory);
  let workspace = await store.createWorkspace({id:'durable', creationToken:'creation', title:'Workspace title'});
  let agent = await store.join({workspaceId:'durable', sessionId:'session', membershipToken:'member', title:'Agent title', cwd:root, boundary:{compactionDigest:'c', transcriptLength:0, prefixDigest:'p'}});
  agent = await store.setDisplayAttempt(agentAuthority(agent.record), {attempt:'attempt',nonce:'owner'}, agent.revision);
  const key = createHash('sha256').update(await realpath(store.directory)).digest('hex');
  const identity = createHash('sha256').update(await realpath(agent.path)).digest('hex');
  const workspaceNative = {workspace_id:'w1',label:'checkout',number:1,tokens:{wcw_id:'durable',wcw_store:key,wcw_token:'creation',wcw_nonce:'binding',wcw_phase:'ready'}};
  const tab = {workspace_id:'w1',tab_id:'w1:t1',label:'1',number:1};
  const pane = {workspace_id:'w1',tab_id:tab.tab_id,pane_id:'w1:p1',terminal_id:'terminal',tokens:{wca_agent:identity,wca_nonce:'owner',wca_pi:'terminal',wca_card:'closed',wca_role:'agent'}};
  const snapshot = {workspaces:[workspaceNative],tabs:[tab],panes:[pane],layouts:[{workspace_id:'w1',tab_id:tab.tab_id,panes:[{pane_id:pane.pane_id}]}]};
  const socket = join(root,'native.sock'), context = {socket,terminalId:'terminal',workspaceId:'w1',tabId:tab.tab_id,bindingNonce:'binding'};
  const effects = []; let hook;
  const run = async args => {
    await hook?.(args);
    if (args[0] === 'status') return JSON.stringify({server:{running:true,compatible:true,endpoint_compatible:true,version:'0.9.5',socket}});
    if (args[0] === 'api') return JSON.stringify({result:{type:'session_snapshot',snapshot}});
    if (args[1] === 'rename') {
      effects.push(args); const surface = args[0] === 'workspace' ? workspaceNative : tab;
      surface.label = args[3];
      return JSON.stringify({result:{type:args[0] === 'workspace' ? 'workspace_info' : 'tab_info',[args[0]]:surface}});
    }
    throw new Error('Unexpected native call: '+args.join(' '));
  };
  const client = new WorkspaceClient({env:{HERDR_SOCKET_PATH:socket},run});
  const names = new NameStore(store.directory);
  const scope = kind => names.scope({socket,workspaceId:'w1',bindingNonce:workspaceNative.tokens.wcw_nonce,kind,surfaceId:kind === 'workspace' ? 'w1' : tab.tab_id});
  return {store,client,agent,workspace,context,snapshot,tab,pane,workspaceNative,effects,names,scope,setHook:f=>{hook=f;},project:(intent='automatic',signal,acceptedOwnTitle=agent.record.title)=>client.projectNames(store,context,agent,config,signal,intent,acceptedOwnTitle)};
}

test('native default tab adopts, workspace CWD label stays manual; explicit resumes both and no-op has no native effect', async t => {
  const f = await fixture(t);
  assert.deepEqual(await f.project(), {workspace:'manual',tab:'renamed'});
  assert.equal(f.tab.label, 'Agent title'); assert.equal(f.workspaceNative.label,'checkout');
  assert.deepEqual(await f.project('explicit'), {workspace:'renamed',tab:'unchanged'});
  assert.equal((await f.names.read(await f.scope('tab'))).record.mode,'auto');
  const count = f.effects.length;
  assert.deepEqual(await f.project(), {workspace:'unchanged',tab:'unchanged'}); assert.equal(f.effects.length,count);
});

test('unproven custom label pins independently of record titleMode, same-label explicit reclaim resumes tracking', async t => {
  const f = await fixture(t); f.tab.label = 'Agent title';
  assert.equal((await f.project()).tab,'manual');
  assert.equal((await f.project('explicit')).tab,'unchanged');
  f.tab.label = 'User override';
  assert.equal((await f.project()).tab,'manual');
  f.tab.label = 'Agent title'; assert.equal((await f.project()).tab,'manual');
  assert.equal((await f.names.read(await f.scope('tab'))).record.mode,'manual');
});

test('actual first layout pane controls eligibility, without skipping shell/card/editor', async t => {
  for(const kind of ['shell','card','editor']) await t.test(kind,async t => {
    const f = await fixture(t);
    const tokens=kind==='card'?{...f.pane.tokens,wca_role:'card'}:kind==='editor'?{wcd_key:'document'}:{};
    const other = {...f.pane,pane_id:'w1:p2',terminal_id:kind==='card'?'closed':'shell',tokens};
    f.snapshot.panes.push(other); f.snapshot.layouts[0].panes.unshift({pane_id:other.pane_id});
    assert.equal((await f.project('explicit')).tab,'not-first');
    assert.equal(f.tab.label,'1');
    f.snapshot.layouts[0].panes.reverse(); assert.equal((await f.project()).tab,'renamed');
  });
});

test('unmarked ownership and stale binding cannot be reclaimed', async t => {
  for (const kind of ['empty','binding','attempt','missing-nonce']) await t.test(kind,async t => {
    const f = await fixture(t);
    if (kind === 'empty') f.pane.tokens = {};
    if (kind === 'missing-nonce') delete f.context.bindingNonce;
    if (kind === 'binding') f.workspaceNative.tokens.wcw_nonce = 'new';
    if (kind === 'attempt') f.pane.tokens.wca_nonce = 'new';
    await assert.rejects(f.project('explicit'), /ownership|unmarked|binding|nonce/i); assert.equal(f.effects.length,0);
  });
});

test('malformed and symlink provenance suspend naming even for explicit intent', async t => {
  for (const kind of ['malformed','symlink']) await t.test(kind,async t => {
    const f = await fixture(t), scope = await f.scope('workspace');
    await mkdir(join(f.store.directory,'.native-names'));
    const path = f.names.path(scope);
    if (kind === 'malformed') await writeFile(path,'{');
    else { const target=join(f.store.directory,'outside'); await writeFile(target,'{}'); await symlink(target,path); }
    await assert.rejects(f.project('explicit')); assert.equal(f.effects.length,0);
  });
});

test('cancellation and stale record/native changes during preparation prevent effects', async t => {
  for (const change of ['record','move','layout','label','binding','abort']) await t.test(change,async t => {
    const f = await fixture(t), controller = new AbortController();
    // Inject after the real cache read, before native revalidation.
    const original = NameStore.prototype.read; let once=true;
    NameStore.prototype.read = async function(...args) {
      const value = await original.apply(this,args);
      if (once) {
        once=false;
        if (change === 'record') await writeFile(f.agent.path,(await readFile(f.agent.path,'utf8')).replace('Agent title','Changed title'));
        if (change === 'move') f.pane.pane_id='w1:p9';
        if (change === 'layout') f.snapshot.layouts[0].panes=[];
        if (change === 'label') f.workspaceNative.label='Changed label';
        if (change === 'binding') f.workspaceNative.tokens.wcw_nonce='new';
        if (change === 'abort') controller.abort(new Error('cancelled'));
      }
      return value;
    };
    try { await assert.rejects(f.project('explicit',controller.signal)); assert.equal(f.effects.length,0); }
    finally { NameStore.prototype.read = original; }
  });
});

test('wrong or late native acknowledgement never publishes automatic provenance', async t => {
  for (const change of ['wrong-id','wrong-label','late-label','late-owner','late-record','abort']) await t.test(change,async t => {
    const f = await fixture(t), controller = new AbortController(), run=f.client.run;
    f.client.run = async args => {
      const output=await run(args);
      if (args[1] === 'rename') {
        const value=JSON.parse(output);
        if (change === 'wrong-id') value.result.workspace.workspace_id='foreign';
        if (change === 'wrong-label') value.result.workspace.label='foreign';
        if (change === 'late-label') f.workspaceNative.label='User rename';
        if (change === 'late-owner') f.pane.tokens.wca_nonce='changed';
        if (change === 'late-record') await f.store.setOwnTitle(agentAuthority(f.agent.record),'Changed title',f.agent.revision);
        if (change === 'abort') controller.abort(new Error('cancelled'));
        return JSON.stringify(value);
      }
      return output;
    };
    await assert.rejects(f.project('explicit',controller.signal));
    assert.equal(await f.names.read(await f.scope('workspace')),null);
  });
});

test('manual record titles still project through automatic provenance, while native overrides stay pinned', async t => {
  const f=await fixture(t); await f.project('explicit');
  const authority=agentAuthority(f.agent.record);
  await f.store.setOwnTitle(authority,'Manual source title',f.agent.revision);
  const workspace=await f.store.readWorkspace('durable');
  await f.store.setWorkspaceTitle(authority,'Manual workspace source',workspace.revision);
  assert.deepEqual(await f.project('automatic',undefined,'Manual source title'),{workspace:'renamed',tab:'renamed'});
  assert.equal(f.tab.label,'Manual source title'); assert.equal(f.workspaceNative.label,'Manual workspace source');
  f.tab.label='Native manual'; assert.equal((await f.project('automatic',undefined,'Manual source title')).tab,'manual');
});

test('binding recreation has a new surface history and cannot import former automatic labels', async t => {
  const f=await fixture(t); await f.project('explicit');
  f.workspaceNative.tokens.wcw_nonce='replacement'; f.context.bindingNonce='replacement';
  assert.deepEqual(await f.project(),{workspace:'manual',tab:'manual'});
  assert.equal((await f.names.read(await f.scope('tab'))).record.owner,null);
});

test('tab defaults use current snapshot position, not stable native number or arbitrary numeric names', async t => {
  for (const label of ['1','3','', ' 1 ']) await t.test(JSON.stringify(label),async t => {
    const f=await fixture(t); f.tab.number=3; f.tab.label=label;
    assert.equal((await f.project()).tab,label==='1'?'renamed':'manual');
  });
});

test('tab wrong acknowledgements retain rename as unproven and do not publish automatic history', async t => {
  for (const change of ['tab_id','workspace_id','number','label','type']) await t.test(change,async t => {
    const f=await fixture(t), run=f.client.run;
    f.client.run=async args=>{
      const output=await run(args);
      if(args[0]!=='tab'||args[1]!=='rename') return output;
      const response=JSON.parse(output);
      if(change==='type') response.result.type='wrong';
      else response.result.tab[change]=change==='number'?99:'wrong';
      return JSON.stringify(response);
    };
    await assert.rejects(f.project()); assert.equal(await f.names.read(await f.scope('tab')),null);
  });
});

test('option-like native title rejects before rename without clearing or publishing provenance', async t => {
  const f=await fixture(t);
  const workspace=await f.store.readWorkspace('durable');
  await f.store.setWorkspaceTitle(agentAuthority(f.agent.record),'--clear',workspace.revision);
  await assert.rejects(f.project('explicit'),/option prefix/); assert.equal(f.effects.length,0);
  assert.equal(await f.names.read(await f.scope('workspace')),null);
});

test('trusted workspace updates independently of unready own title; an accepted source resumes default and claimed tabs', async t => {
  for(const kind of ['default','claimed']) await t.test(kind,async t => {
    const f=await fixture(t);
    if(kind==='default') {
      const shell={...f.pane,pane_id:'w1:p2',terminal_id:'shell',tokens:{}};
      f.snapshot.panes.push(shell); f.snapshot.layouts[0].panes.unshift({pane_id:shell.pane_id});
      assert.equal((await f.project('explicit',undefined,null)).tab,'not-first');
      f.snapshot.panes.pop(); f.snapshot.layouts[0].panes.shift();
    } else await f.project('explicit',undefined,null);
    const before=f.tab.label;
    const own=await f.store.setOwnTitle(agentAuthority(f.agent.record),'Untitled task',f.agent.revision);
    const workspace=await f.store.readWorkspace('durable');
    await f.store.setWorkspaceTitle(agentAuthority(own.record),'Changed workspace',workspace.revision);
    assert.deepEqual(await f.project('automatic',undefined,null),{workspace:'renamed',tab:'not-ready'});
    assert.equal(f.tab.label,before);
    assert.equal((await f.project()).tab,'not-ready'); // Painted source is not current readiness proof.
    const accepted=await f.store.setOwnTitle(agentAuthority(own.record),'Accepted own title',own.revision);
    assert.equal((await f.project('automatic',undefined,accepted.record.title)).tab,'renamed');
    assert.equal(f.tab.label,'Accepted own title');
    assert.equal((await f.project('explicit',undefined,null)).tab,'unchanged');
  });
});

test('actual-first outcome takes precedence over own-title readiness', async t => {
  const f=await fixture(t), shell={...f.pane,pane_id:'w1:p2',terminal_id:'shell',tokens:{}};
  f.snapshot.panes.push(shell); f.snapshot.layouts[0].panes.unshift({pane_id:shell.pane_id});
  assert.equal((await f.project('automatic',undefined,null)).tab,'not-first');
});

test('own-title readiness is rechecked after cache awaits and after acknowledgement', async t => {
  for(const stage of ['prepare','acknowledgement']) await t.test(stage,async t => {
    const f=await fixture(t); await f.project('explicit');
    const initial=await f.store.setOwnTitle(agentAuthority(f.agent.record),'Accepted own title',f.agent.revision);
    const original=NameStore.prototype.read, run=f.client.run; let once=true;
    const change=async()=>{if(once){once=false;await f.store.setOwnTitle(agentAuthority(initial.record),'New unaccepted source',initial.revision);}};
    if(stage==='prepare') NameStore.prototype.read=async function(scope) {const value=await original.call(this,scope);if(scope.kind==='tab') await change();return value;};
    else f.client.run=async args=>{const value=await run(args);if(args[0]==='tab'&&args[1]==='rename') await change();return value;};
    const count=f.effects.length;
    try {await assert.rejects(f.project('automatic',undefined,initial.record.title),/partial|changed/);}
    finally {NameStore.prototype.read=original;}
    assert.equal(f.effects.length,count+(stage==='prepare'?0:1));
    assert.equal((await f.names.read(await f.scope('tab'))).record.lastApplied,'Agent title');
    assert.equal((await f.project('automatic',undefined,initial.record.title)).tab,'not-ready');
  });
});

test('cache publication failure retains native progress and retries do not adopt unproven names', async t => {
  const f=await fixture(t), original=NameStore.prototype.write;
  NameStore.prototype.write=async()=>{throw new Error('cache unavailable');};
  try { await assert.rejects(f.project('explicit'),/partial|cache unavailable/); }
  finally {NameStore.prototype.write=original;}
  assert.equal(f.workspaceNative.label,'Workspace title');
  assert.equal((await f.project()).workspace,'manual');
});
