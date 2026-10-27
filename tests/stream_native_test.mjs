import assert from 'node:assert/strict';
import test from 'node:test';
import { randomUUID } from 'node:crypto';
import { createJiti } from 'jiti';
const jiti=createJiti(import.meta.url);
const {NativeController}=await jiti.import('../extensions/stream-checkpoint/native.ts');
const {loadRustNative}=await jiti.import('../extensions/stream-checkpoint/native-rust.ts');
const {default:register}=await jiti.import('../extensions/stream-checkpoint/index.ts');
const {defaults}=await jiti.import('../extensions/stream-checkpoint/settings.ts');
const turn=()=>new Promise(resolve=>setImmediate(resolve));
async function until(check) {for(let i=0;i<100&&!check();i++)await turn();assert.ok(check());}
function environment(t,changes={}) {
 const values={HERDR_ENV:'1',HERDR_PANE_ID:'pane',HERDR_TAB_ID:'tab',HERDR_WORKSPACE_ID:'workspace',HERDR_SOCKET_PATH:'/not-live.sock',...changes};
 const previous=Object.fromEntries(Object.keys(values).map(key=>[key,process.env[key]]));
 for(const [key,value] of Object.entries(values)) {if(value===undefined)delete process.env[key];else process.env[key]=value;}
 t.after(()=>{for(const [key,value] of Object.entries(previous)) {if(value===undefined)delete process.env[key];else process.env[key]=value;}});
}
function fixture(t,options={}) {
 const id=randomUUID(),requests=[],notices=[],listeners=new Set(),entries=[];
 let name,disposed=0,loads=0,loaded;
 const ctx={hasUI:true,mode:'tui',cwd:'/repo',sessionManager:{getSessionId:()=>id,getLeafId:()=> 'leaf',getBranch:()=>[{id:'leaf'}],buildSessionProjection:()=>({messages:[]})},ui:{notify:text=>notices.push(text),setStatus:()=>{},select:async()=> 'Close'}};
 const pi={getSessionName:()=>name,setSessionName:value=>{name=value;entries.push({value});},appendEntry:()=>{}};
 const child={subscribe:listener=>{listeners.add(listener);return()=>listeners.delete(listener);},stop:async()=>{disposed++;},run:async(request,runOptions)=>{
  requests.push(structuredClone(request));
  if(options.run)return options.run(request,runOptions);
  return {status:'success',result:request.kind==='session_started'?{native:'active',display:'suppressed'}:request.kind==='close_card'?{closed:true}:{display:'existing'}};
 }};
 const factory=async input=>{loads++;loaded=input;return loadRustNative(input);};
 const controller=new NativeController({pi,ctx,child,factory,warning:text=>notices.push(text)});
 t.after(async()=>{controller.stop();await controller.idle();});
 return {id,ctx,pi,child,controller,requests,notices,entries,factory,loads:()=>loads,loaded:()=>loaded,name:()=>name,disposed:()=>disposed,view:desired_pi_name=>{for(const listener of listeners)listener({desired_pi_name});}};
}
test('construction is inert; startup sends only session_started and explicit actions send one typed intent each',async t=>{
 environment(t);const f=fixture(t);await turn();assert.deepEqual(f.requests,[]);assert.equal(f.loads(),0);
 await f.controller.start();await f.controller.start();await f.controller.idle();
 assert.deepEqual(f.requests,[{kind:'session_started'}]);assert.equal(f.loads(),1);
 assert.equal(f.loaded().sessionId,f.id);assert.equal(f.loaded().cwd,'/repo');assert.notEqual(f.loaded().env,process.env);
 await f.controller.open();await f.controller.close();assert.deepEqual(f.requests.map(request=>request.kind),['session_started','open_card','close_card']);
});
for(const [label,env,ctx] of [
 ['headless',{}, {hasUI:false}],['rpc',{}, {mode:'rpc'}],['outside Herdr',{HERDR_ENV:undefined},{}],['no exact pane',{HERDR_PANE_ID:undefined},{}],
])test(`${label} never loads UI native dependencies`,async t=>{
 environment(t,env);const f=fixture(t);Object.assign(f.ctx,ctx);await f.controller.start();assert.equal(f.loads(),0);await assert.rejects(f.controller.open(),/interactive/);assert.deepEqual(f.requests,[]);
});
test('helper identities never trigger session startup',async t=>{
 environment(t);const f=fixture(t);f.ctx.sessionManager.getSessionId=()=> 'helper';
 const controller=new NativeController({pi:f.pi,ctx:f.ctx,child:f.child,factory:f.factory,warning:assert.fail});t.after(()=>controller.stop());
 await controller.start();assert.equal(f.loads(),0);
});
for(const reason of ['outside_herdr','not_enrolled'])test(`inactive ${reason} lifecycle is expected, without error-text matching`,async t=>{
 environment(t);const f=fixture(t,{run:async request=>request.kind==='session_started'?{status:'success',result:{native:'inactive',reason}}:{status:'error',error:{kind:'inactive',message:'No active native owner'}}});
 await f.controller.start();assert.deepEqual(f.notices,[]);await assert.rejects(f.controller.open(),error=>error.kind==='inactive');assert.deepEqual(f.requests.map(value=>value.kind),['session_started','open_card']);
});
test('startup failures warn; explicit errors preserve their kind and are never retried',async t=>{
 environment(t);const f=fixture(t,{run:async()=>({status:'error',error:{kind:'native-context-changed',message:'Origin not proved'}})});
 await f.controller.start();assert.match(f.notices[0],/Origin not proved/);
 await assert.rejects(f.controller.close(),error=>error.kind==='native-context-changed');await turn();assert.deepEqual(f.requests.map(value=>value.kind),['session_started','close_card']);
});
test('failed startup before adapter assignment reports a lifecycle error rather than dereferencing undefined',async t=>{
 environment(t);const f=fixture(t);process.env.HERDR_PANE_ID='changed';await f.controller.start();process.env.HERDR_PANE_ID='pane';
 await assert.rejects(f.controller.open(),/startup did not initialize/);assert.deepEqual(f.requests,[]);
});
test('an explicit open rejects automatic-only suppression instead of reporting a pane open',async t=>{
 environment(t);const f=fixture(t,{run:async()=>({status:'success',result:{native:'active',display:'suppressed'}})});
 await f.controller.start();await assert.rejects(f.controller.open(),/display acknowledgement/);assert.equal(f.requests.length,2);
});
test('the child, not TypeScript, serializes explicit actions',async t=>{
 environment(t);let release;const f=fixture(t,{run:async request=>{
  if(request.kind==='session_started')return {status:'success',result:{native:'active',display:'suppressed'}};
  if(request.kind==='open_card')await new Promise(resolve=>release=resolve);
  return {status:'success',result:request.kind==='open_card'?{display:'opened'}:{closed:true}};
 }});
 await f.controller.start();const opening=f.controller.open();await until(()=>!!release);
 await f.controller.close();assert.deepEqual(f.requests.map(value=>value.kind),['session_started','open_card','close_card']);release();await opening;
});
test('desired names use the checked setter without origin tracking; null and identical names are inert',async t=>{
 environment(t);const f=fixture(t);await f.controller.start();
 f.view('Saved title');f.view('Saved title');f.view(null);assert.equal(f.name(),'Saved title');assert.deepEqual(f.entries,[{value:'Saved title'}]);
 f.pi.setSessionName('User edit');f.view('Catalog wins');assert.equal(f.name(),'Catalog wins');assert.deepEqual(f.notices,[]);
 assert.deepEqual(f.requests,[{kind:'session_started'}]);f.controller.stop();await f.controller.idle();f.view('Retired');assert.equal(f.name(),'Catalog wins');assert.equal(f.disposed(),0,'shared transport is retired by index, not the UI adapter');
});
test('name application errors are UI warnings, not protocol failures or native refresh triggers',async t=>{
 environment(t);const f=fixture(t);await f.controller.start();f.view(' ');assert.match(f.notices[0],/nonempty/);assert.equal(f.requests.length,1);
});
for(const change of ['pane','endpoint','cwd','session','cargo-target'])test(`changed ${change} fences UI facts and explicit commands`,async t=>{
 environment(t);const f=fixture(t);await f.controller.start();
 if(change==='pane')process.env.HERDR_PANE_ID='other';
 if(change==='endpoint')process.env.HERDR_SOCKET_PATH='/other';
 if(change==='cwd')f.ctx.cwd='/other';
 if(change==='session')f.ctx.sessionManager.getSessionId=()=>randomUUID();
 if(change==='cargo-target')environment(t,{CARGO_TARGET_DIR:'/other-target'});
 f.view('Stale title');assert.equal(f.name(),undefined);await assert.rejects(f.controller.open(),/session or environment changed/);assert.equal(f.requests.length,1);
});
test('shutdown cancels a pending native request but never sends close or final capture',async t=>{
 environment(t);let signal,release;const f=fixture(t,{run:async(_request,options)=>{signal=options.signal;await new Promise(resolve=>release=resolve);return {status:'success',result:{native:'active',display:'opened'}};}});
 const starting=f.controller.start();await until(()=>!!release);f.controller.stop();assert.equal(signal.aborted,true);release();await starting;await f.controller.idle();assert.deepEqual(f.notices,[]);assert.deepEqual(f.requests,[{kind:'session_started'}]);
});
test('index branch events and already-bound attach do not request projection, capture or reclaim',async t=>{
 environment(t);const f=fixture(t);
 // The test factory uses the same child while the index is given a fake catalog client.
 const original=f.factory;const adapter=await original({pi:f.pi,ctx:f.ctx,sessionId:f.id,cwd:f.ctx.cwd,env:{...process.env},child:f.child,assertCurrent:()=>{},warning:assert.fail});
 const handlers=new Map(),commands=new Map();
 register({...f.pi,on:(key,value)=>handlers.set(key,value),registerCommand:(key,value)=>commands.set(key,value),registerTool:()=>{}},{cli:{context:async()=>({session:{stream_id:'effort'}}),sync:async()=>({local:'unchanged',sharing:'local-only',domain:null})},native:async()=>adapter,settings:{...defaults,reporting:'manual',enrollment:'manual'}});
 await handlers.get('session_start')({},f.ctx);for(let i=0;i<20;i++)handlers.get('session_tree')({},f.ctx);
 await commands.get('streams').handler('attach',f.ctx);assert.deepEqual(f.requests,[{kind:'session_started'}]);
 await handlers.get('session_shutdown')({},f.ctx);assert.deepEqual(f.requests,[{kind:'session_started'}]);
});
test('index sends session_started before startup sync and still syncs after a native lifecycle error',async t=>{
 environment(t);let release;const f=fixture(t,{run:async()=>new Promise(resolve=>release=resolve)}),handlers=new Map(),syncs=[];
 register({...f.pi,on:(name,handler)=>handlers.set(name,handler),registerCommand:()=>{},registerTool:()=>{}},{cli:{sync:async()=>{syncs.push('sync');return {local:'unchanged',sharing:'local-only',domain:null};}},native:input=>f.factory({...input,child:f.child}),settings:{...defaults,reporting:'manual',enrollment:'manual'}});
 const starting=handlers.get('session_start')({},f.ctx);await until(()=>!!release);assert.deepEqual(f.requests,[{kind:'session_started'}]);assert.deepEqual(syncs,[]);
 release({status:'error',error:{kind:'runtime',message:'Origin not proved'}});await starting;assert.deepEqual(syncs,['sync']);assert.match(f.notices[0],/Origin not proved/);
 await handlers.get('session_shutdown')({},f.ctx);assert.deepEqual(syncs,['sync','sync']);
});
test('native actions retain accepted warning outcomes and return display results without confirmation reads',async t=>{
 environment(t);const f=fixture(t,{run:async request=>({status:request.kind==='session_started'?'success':'saved_with_warnings',result:request.kind==='session_started'?{native:'active',display:'suppressed'}:{display:'opened'},...(request.kind==='session_started'?{}:{warnings:['Title saved; projection deferred']})})});
 await f.controller.start();assert.deepEqual(await f.controller.open(),{display:'opened'});assert.deepEqual(f.notices,['Title saved; projection deferred']);assert.equal(f.requests.length,2);
});
