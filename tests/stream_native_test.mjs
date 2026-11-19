import assert from 'node:assert/strict';
import test from 'node:test';
import { randomUUID } from 'node:crypto';
import { existsSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { SessionManager } from '@earendil-works/pi-coding-agent';
import { createJiti } from 'jiti';
import { withPatchSchemas } from './helpers/patch-schemas.mjs';
const jiti=createJiti(import.meta.url);
const {NativeController}=await jiti.import('../extensions/stream-checkpoint/native.ts');
const {loadRustNative}=await jiti.import('../extensions/stream-checkpoint/native-rust.ts');
const {default:register}=await jiti.import('../extensions/stream-checkpoint/index.ts');
const {defaults}=await jiti.import('../extensions/stream-checkpoint/settings.ts');
const turn=()=>new Promise(resolve=>setImmediate(resolve));
async function until(check) {for(let i=0;i<100&&!check();i++)await turn();assert.ok(check());}
function environment(t,changes={}) {
 const values={HERDR_ENV:'1',HERDR_PANE_ID:'pane',HERDR_TAB_ID:'tab',HERDR_WORKSPACE_ID:'workspace',HERDR_SOCKET_PATH:'/not-live.sock',HC_TERMINAL_ID:undefined,HC_SOCKET:undefined,HC_SESSION:undefined,CMUX_WORKSPACE_ID:undefined,...changes};
 const previous=Object.fromEntries(Object.keys(values).map(key=>[key,process.env[key]]));
 for(const [key,value] of Object.entries(values)) {if(value===undefined)delete process.env[key];else process.env[key]=value;}
 t.after(()=>{for(const [key,value] of Object.entries(previous)) {if(value===undefined)delete process.env[key];else process.env[key]=value;}});
}
function fixture(t,options={}) {
 const id=options.manager?.getSessionId()??randomUUID(),requests=[],notices=[],listeners=new Set(),entries=[];
 const messages=options.empty?[]:[{role:'user',content:'Build work cards',timestamp:0}];
 let name,disposed=0,loads=0,loaded;
 const ctx={hasUI:true,mode:'tui',cwd:'/repo',sessionManager:options.manager??{getSessionId:()=>id,getLeafId:()=> 'leaf',getBranch:()=>[{id:'leaf'}],buildSessionProjection:()=>({messages})},ui:{notify:text=>notices.push(text),setStatus:()=>{},select:async()=> 'Close'}};
 const pi={getSessionName:()=>name,setSessionName:value=>{name=value;entries.push({value});},appendEntry:()=>{}};
 const child={subscribe:listener=>{listeners.add(listener);return()=>listeners.delete(listener);},stop:async()=>{disposed++;},run:async(request,runOptions)=>{
  requests.push(structuredClone(request));
  if(options.run)return options.run(request,runOptions);
  return {status:'success',result:request.kind==='session_started'?{native:'active',display:'suppressed'}:request.kind==='close_card'?{closed:true}:{display:'existing'}};
 }};
 const factory=async input=>{loads++;loaded=input;return loadRustNative(input);};
 const controller=new NativeController({pi,ctx,child,factory,warning:text=>notices.push(text)});
 t.after(async()=>{controller.stop();await controller.idle();});
 return {id,ctx,pi,child,controller,requests,notices,entries,messages,factory,loads:()=>loads,loaded:()=>loaded,name:()=>name,disposed:()=>disposed,view:desired_pi_name=>{for(const listener of listeners)listener({desired_pi_name});}};
}
test('empty Pi metadata does not start native work; the first persisted message starts it once',async t=>{
 environment(t);const directory=mkdtempSync(join(tmpdir(),'streams-empty-pi-'));t.after(()=>rmSync(directory,{recursive:true,force:true}));
 const manager=SessionManager.create(directory,directory),f=fixture(t,{manager});
 manager.appendCustomEntry('startup-metadata',{ready:true});
 assert.equal(existsSync(manager.getSessionFile()),false);
 assert.equal(f.controller.start(),undefined);assert.equal(f.controller.start(),undefined);
 assert.deepEqual(f.requests,[]);assert.equal(f.loads(),0);assert.deepEqual(f.notices,[]);
 await assert.rejects(f.controller.ready(),/Start a conversation/);assert.deepEqual(f.requests,[]);
 manager.appendMessage({role:'user',content:'Build work cards',timestamp:0});
 assert.equal(existsSync(manager.getSessionFile()),true);
 await f.controller.ready();await f.controller.start();assert.deepEqual(f.requests,[{kind:'session_started'}]);
});
test('hc starts an empty interactive conversation and accepts a typed lifecycle report',async t=>{
 environment(t,{HERDR_ENV:undefined,HERDR_PANE_ID:undefined,HC_TERMINAL_ID:'t1'});
 const f=fixture(t,{empty:true,run:async()=>({status:'success',result:{backend:'hc',report:{native:{kind:'inactive',reason:'unavailable'},projection:{companion:null,desired_pi_name:null,warnings:['Display unavailable']}}}})});
 await f.controller.start();assert.deepEqual(f.requests,[{kind:'session_started'}]);assert.deepEqual(f.notices,['Display unavailable']);
});
test('hc startup failure leaves catalog preparation available without retrying native startup',async t=>{
 environment(t,{HERDR_ENV:undefined,HERDR_PANE_ID:undefined,HC_TERMINAL_ID:'t1'});
 const f=fixture(t,{run:async()=>({status:'error',error:{kind:'runtime',message:'Registration unavailable'}})});
 await assert.rejects(f.controller.start(),/Registration unavailable/);await f.controller.prepareCatalog();await f.controller.prepareCatalog();
 assert.deepEqual(f.requests,[{kind:'session_started'}]);await assert.rejects(f.controller.open(),/Registration unavailable/);
});
for(const role of ['compactionSummary','branchSummary'])test(`resumed ${role} history starts native work`,async t=>{
 environment(t);const f=fixture(t,{empty:true});f.messages.push({role,summary:'Earlier work',timestamp:0});
 await f.controller.start();assert.deepEqual(f.requests,[{kind:'session_started'}]);
});
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
test('startup failure rejects every dependent action without retrying or sending explicit effects',async t=>{
 environment(t);const f=fixture(t,{run:async()=>({status:'error',error:{kind:'native-context-changed',message:'Origin not proved'}})});
 await assert.rejects(f.controller.start(),error=>error.kind==='native-context-changed');
 await assert.rejects(f.controller.close(),error=>error.kind==='native-context-changed');await turn();assert.deepEqual(f.requests.map(value=>value.kind),['session_started']);
});
test('failed startup before adapter assignment reports a lifecycle error rather than dereferencing undefined',async t=>{
 environment(t);const f=fixture(t);process.env.HERDR_PANE_ID='changed';await assert.rejects(f.controller.start(),/environment changed/);process.env.HERDR_PANE_ID='pane';
 await assert.rejects(f.controller.open(),/environment changed/);assert.deepEqual(f.requests,[]);
});
test('an explicit open rejects automatic-only suppression instead of reporting a pane open',async t=>{
 environment(t);const f=fixture(t,{run:async()=>({status:'success',result:{native:'active',display:'suppressed'}})});
 await f.controller.start();await assert.rejects(f.controller.open(),/display acknowledgement/);assert.equal(f.requests.length,2);
});
test('deferred startup reads the current context and empty close has action-neutral wording',async t=>{
 environment(t);const f=fixture(t,{empty:true});let current=f.ctx;
 const controller=new NativeController({pi:f.pi,ctx:f.ctx,current:()=>current,child:f.child,factory:f.factory,warning:assert.fail});t.after(async()=>{controller.stop();await controller.idle();});
 await assert.rejects(controller.close(),error=>/Start a conversation/.test(error.message)&&!error.message.includes('opening'));
 current={...f.ctx,sessionManager:{...f.ctx.sessionManager,buildSessionProjection:()=>({messages:[{role:'user',content:'New work',timestamp:0}]})}};
 await controller.start();assert.deepEqual(f.requests,[{kind:'session_started'}]);
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
 const starting=f.controller.start();await until(()=>!!release);f.controller.stop();assert.equal(signal.aborted,true);release();await assert.rejects(starting,/aborted/);await f.controller.idle();assert.deepEqual(f.notices,[]);assert.deepEqual(f.requests,[{kind:'session_started'}]);
});
function paneFixture(t,options={}) {
 const handlers=new Map(),commands=new Map(),tools=new Map(),steps=[],inferences=[],enrollments=[];
 const catalog=withPatchSchemas({catalog_source:'source-a',host_id:'host',stream:null,session:null,title_defaults:{stream:'automatic',session:'automatic'}});
 let preference='inherit',unreadable=options.unreadable??null;
 const f=fixture(t,{empty:options.empty,run:async request=>{
  if(options.realClient && ['context','sync','enroll','apply'].includes(request.kind))return {status:'success',result:await cli[request.kind](request.request)};
  steps.push(request.kind);
  if(request.kind==='discard_unreadable_card'){const stream=unreadable;unreadable=null;return {status:'success',result:{session:null,stream}};}
  if(request.kind==='session_started'){
   if(unreadable)return {status:'error',error:{kind:'runtime',message:unreadable}};
   if(f.messages.length)await options.beforeStart?.();
   if(options.workspaceStream)catalog.stream=structuredClone(options.workspaceStream);
   return {status:'success',result:catalog.session?{native:'active',display:'suppressed'}:{native:'inactive',reason:'not_enrolled'}};
  }
  preference=request.kind==='close_card'?'manual':'automatic';
  if(!catalog.session)return {status:'error',error:{kind:'runtime',message:'Card preference saved; native card effects require an enrolled Session'}};
  return {status:'success',result:request.kind==='close_card'?{closed:true}:{display:'opened'}};
 }});
 const attach=()=>{
  catalog.stream={id:'effort',title:'Work',title_mode:'automatic',goal:'Goal',goal_mode:'automatic',goal_short:null,owner:{host_id:'host'},revision:1,updated_at_ms:0,repositories:{},references:[]};
  catalog.session={id:f.id,host_id:'host',revision:1,updated_at_ms:0,membership_epoch:0,stream_id:'effort',harness:'pi',harness_session_id:f.id,title:'Contribution',title_mode:'automatic',status:'active',recap:'',state:'',state_short:null,remaining:'',checkout:null};
 };
 if(options.attached)attach();
 const cli={
  context:async()=>structuredClone(catalog),
  sync:async()=>({local:'unchanged',sharing:'local-only',domain:null}),
  enroll:async request=>{
   steps.push('enroll');enrollments.push(structuredClone(request));
   if(options.rejectEnrollment)return {saves:[{target:'session:native',save:{local:'unchanged',sharing:'local-only',domain:{outcome:'conflict'}}}]};
   attach();return {stream:catalog.stream,session:catalog.session,saves:[{target:'session:native',save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}}}]};
  },
  apply:async request=>{
   steps.push(`apply-${request.kind}`);Object.assign(catalog[request.kind],request.changes);
   return {save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}}};
  },
 };
 register({...f.pi,on:(key,value)=>handlers.set(key,value),registerCommand:(key,value)=>commands.set(key,value),registerTool:tool=>tools.set(tool.name,tool)},
  {...(options.realClient?{child:()=>f.child}:{cli}),native:input=>f.factory({...input,child:f.child}),settings:{...defaults,...options.settings},infer:async(ctx,request,signal)=>{
   inferences.push(request.type);if(options.infer)await options.infer(ctx,request,signal);
   return request.type==='enroll'?{meaningful:true,stream:{title:'Work'},session:{title:'Contribution'}}:{changes:request.type==='session'?{state:'Reported'}:{goal:'Refined'}};
  }});
 t.after(()=>handlers.get('session_shutdown')({},f.ctx));
 return {...f,handlers,tools,steps,inferences,enrollments,catalog,preference:()=>preference,start:()=>handlers.get('session_start')({},f.ctx),run:(args='pane')=>commands.get('streams').handler(args,f.ctx)};
}
test('empty startup and explicit reports do no native or model work',async t=>{
 environment(t);const f=paneFixture(t,{empty:true});await f.start();await f.run('report');
 await until(()=>f.notices.length>0);
 assert.deepEqual(f.steps,[]);assert.deepEqual(f.inferences,[]);assert.equal(f.preference(),'inherit');
 assert.match(f.notices.at(-1),/Start a conversation/);
 await f.run();assert.deepEqual(f.steps,[]);assert.deepEqual(f.inferences,[]);assert.match(f.notices.at(-1),/Start a conversation/);
});
test('first report waits for native startup and captures the workspace Stream before enrollment',async t=>{
 environment(t);let release;
 const stream={id:'effort',title:'Work',title_mode:'automatic',goal:'Goal',goal_mode:'automatic',goal_short:null,owner:{host_id:'host'},revision:1,updated_at_ms:0,repositories:{},references:[]};
 const f=paneFixture(t,{empty:true,workspaceStream:stream,beforeStart:()=>new Promise(resolve=>release=resolve),infer:async(_ctx,request)=>{
  if(request.type==='enroll')assert.deepEqual(request.catalog.stream,stream);
 }});
 await f.start();assert.deepEqual(f.steps,[]);
 f.messages.push({role:'user',content:'Build work cards',timestamp:0});
 f.handlers.get('agent_end')({messages:[{role:'assistant',stopReason:'stop'}]},f.ctx);
 await until(()=>!!release);assert.deepEqual(f.inferences,[]);
 release();await until(()=>f.steps.includes('apply-stream'));
 assert.deepEqual(f.steps,['session_started','enroll','apply-session','apply-stream']);
 assert.equal(f.enrollments.length,1);assert.equal(f.enrollments[0].stream_id,'effort');assert.equal(Object.hasOwn(f.enrollments[0],'stream'),false);
 assert.deepEqual(f.notices,[]);
 await f.run();assert.equal(f.steps.filter(step=>step==='session_started').length,1);
});
for(const operation of ['read','select','create','attach'])test(`first ${operation} initializes native context even with reporting and enrollment manual`,async t=>{
 environment(t);
 const stream={id:'effort',title:'Work',title_mode:'automatic',goal:'Goal',goal_mode:'automatic',goal_short:null,owner:{host_id:'host'},revision:1,updated_at_ms:0,repositories:{},references:[]};
 const f=paneFixture(t,{empty:true,realClient:true,workspaceStream:stream,settings:{reporting:'manual',enrollment:'manual'}});
 f.ctx.ui.editor=async(_title,initial)=>initial||'Contribution';f.ctx.ui.confirm=async()=>true;
 await f.start();assert.deepEqual(f.steps,[]);
 if(operation==='attach') {await f.run('attach');assert.match(f.notices.at(-1),/Start a conversation/);assert.deepEqual(f.enrollments,[]);}
 f.messages.push({role:'user',content:'Build work cards',timestamp:0});
 if(operation==='read') {
  const value=await f.tools.get('stream_read').execute('read',{},undefined,undefined,f.ctx);assert.deepEqual(value.details.stream,stream);
 } else if(operation==='attach')await f.run('attach');
 else if(operation==='select')await f.tools.get('select_stream').execute('select',{stream_id:'effort',reason:'Continue'},undefined,undefined,f.ctx);
 else await assert.rejects(f.tools.get('create_stream').execute('create',{title:'Conflicting',reason:'Continue'},undefined,undefined,f.ctx),/already associated/);
 assert.equal(f.steps.filter(step=>step==='session_started').length,1);assert.deepEqual(f.inferences,[]);
 if(['select','attach'].includes(operation)) {assert.equal(f.enrollments.length,1);assert.equal(f.enrollments[0].stream_id,'effort');}
 else assert.deepEqual(f.enrollments,[]);
});
test('failed deferred startup blocks report inference and manual reads until reload',async t=>{
 environment(t);const f=paneFixture(t,{empty:true,realClient:true,beforeStart:()=>{throw Error('Origin not proved');}});
 await f.start();f.messages.push({role:'user',content:'Build work cards',timestamp:0});
 await f.run('report');await until(()=>f.notices.some(text=>text.includes('Origin not proved')));
 await assert.rejects(f.tools.get('stream_read').execute('read',{},undefined,undefined,f.ctx),/Origin not proved/);
 assert.deepEqual(f.inferences,[]);assert.deepEqual(f.enrollments,[]);assert.deepEqual(f.steps,['session_started']);
});
test('a reset discards the unreadable card that failed native startup, then starts again and rebuilds it',async t=>{
 environment(t);const f=paneFixture(t,{attached:true,realClient:true,unreadable:'Stream record hosts/host/streams/effort.json is unreadable: unknown field `archived`'});
 await f.start();await until(()=>f.notices.some(text=>text.includes('unknown field `archived`')));
 await f.run('report');await until(()=>f.notices.filter(text=>text.includes('unknown field `archived`')).length===2);
 assert.deepEqual(f.inferences,[],'a report still waits for a native startup that failed');
 await f.run('reset');await until(()=>f.steps.includes('apply-stream'));
 assert.deepEqual(f.steps.filter(step=>step!=='lifecycle'),['session_started','discard_unreadable_card','session_started','apply-session','apply-stream']);
 assert.ok(f.notices.some(text=>text.endsWith('unknown field `archived`. Reset discarded it.')),f.notices.join('\n'));
});
test('a tree change during manual read startup prevents its context request',async t=>{
 environment(t);let release;const f=paneFixture(t,{empty:true,realClient:true,beforeStart:()=>new Promise(resolve=>release=resolve)});
 await f.start();f.messages.push({role:'user',content:'Build work cards',timestamp:0});
 const reading=f.tools.get('stream_read').execute('read',{},undefined,undefined,f.ctx);await until(()=>!!release);
 f.handlers.get('session_tree')({},f.ctx);release();await assert.rejects(reading,/Session changed/);
 assert.deepEqual(f.requests.map(request=>request.kind),['sync','session_started']);
});
test('a tree change during deferred startup prevents reporting from the old branch',async t=>{
 environment(t);let release;const f=paneFixture(t,{empty:true,beforeStart:()=>new Promise(resolve=>release=resolve)});
 await f.start();f.messages.push({role:'user',content:'Build work cards',timestamp:0});
 await f.run('report');await until(()=>!!release);f.handlers.get('session_tree')({},f.ctx);release();
 await turn();await turn();assert.deepEqual(f.inferences,[]);assert.deepEqual(f.steps,['session_started']);
});
for(const reporting of ['automatic','manual'])test(`unenrolled pane reports then opens with ${reporting} background reporting`,async t=>{
 environment(t);const f=paneFixture(t,{settings:{reporting}});await f.start();
 assert.equal(f.catalog.session,null);assert.equal(f.preference(),'inherit');assert.deepEqual(f.inferences,[]);
 await f.run();
 assert.deepEqual(f.steps,['session_started','enroll','apply-session','apply-stream','open_card']);
 assert.deepEqual(f.inferences,['enroll','session','stream']);assert.equal(f.preference(),'automatic');assert.equal(f.catalog.session.state,'Reported');
 assert.ok(f.notices.every(text=>!text.includes('native action failed')&&!text.includes('require an enrolled')));
});
test('manual enrollment explains attach without inference, native actions or preference changes',async t=>{
 environment(t);const f=paneFixture(t,{settings:{enrollment:'manual'}});await f.start();await f.run();
 assert.deepEqual(f.inferences,[]);assert.deepEqual(f.steps,['session_started']);assert.equal(f.preference(),'inherit');assert.equal(f.catalog.session,null);
 assert.match(f.notices.at(-1),/manual.*\/streams attach/);
});
test('already enrolled pane opens without inference even with both settings manual',async t=>{
 environment(t);const f=paneFixture(t,{attached:true,settings:{reporting:'manual',enrollment:'manual'}});await f.start();await f.run();
 assert.deepEqual(f.inferences,[]);assert.deepEqual(f.steps,['session_started','open_card']);assert.equal(f.preference(),'automatic');
});
for(const failure of ['model','save'])test(`pane ${failure} failure does not send open or save its preference`,async t=>{
 environment(t);const f=paneFixture(t,{rejectEnrollment:failure==='save',infer:async()=>{if(failure==='model')throw Error('Model unavailable');}});await f.start();await f.run();
 assert.deepEqual(f.inferences,['enroll']);assert.deepEqual(f.steps,failure==='save'?['session_started','enroll']:['session_started']);
 assert.equal(f.preference(),'inherit');assert.equal(f.catalog.session,null);
 assert.match(f.notices.at(-1),failure==='model'?/Model unavailable/:/did not enroll.*\/streams attach/);
});
test('pane enrollment is fenced when the session tree changes during inference',async t=>{
 environment(t);let f;f=paneFixture(t,{infer:async(_ctx,_request,signal)=>{f.handlers.get('session_tree')({},f.ctx);assert.equal(signal.aborted,true);}});
 await f.start();await f.run();assert.deepEqual(f.steps,['session_started']);assert.equal(f.preference(),'inherit');assert.deepEqual(f.notices,[]);
});
test('outside Herdr pane never runs enrollment',async t=>{
 environment(t,{HERDR_ENV:undefined});const f=paneFixture(t);await f.start();await f.run();
 assert.deepEqual(f.inferences,[]);assert.deepEqual(f.steps,[]);assert.equal(f.preference(),'inherit');assert.match(f.notices.at(-1),/interactive/);
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
