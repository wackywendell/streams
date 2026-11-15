import assert from 'node:assert/strict';
import test from 'node:test';
import {mkdtempSync,readFileSync,rmSync} from 'node:fs';
import {dirname,join} from 'node:path';
import {tmpdir} from 'node:os';
import {fileURLToPath,pathToFileURL} from 'node:url';
import {createJiti} from 'jiti';
import {resolveSdkEntry} from '../extensions/stream-checkpoint/lab-cli.mjs';

if(!process.env.STREAMS_PI_SDK_ENTRY)throw Error('Select an absolute Pi SDK entry with STREAMS_PI_SDK_ENTRY; this test is not part of the hermetic suite');
const entry=resolveSdkEntry(),manifest=JSON.parse(readFileSync(join(dirname(entry),'../package.json'),'utf8'));
assert.equal(manifest.name,'@earendil-works/pi-coding-agent');
assert.equal(manifest.version,'1.5.2','this optional runtime fixture targets the supported Pi 1.5.2 API');
const sdk=await import(pathToFileURL(entry).href);
assert.equal(typeof sdk.SessionManager?.inMemory,'function');assert.equal(typeof sdk.AgentSessionRuntime,'function');
const selected=createJiti(entry,{fsCache:false}),alias={'@earendil-works/pi-coding-agent':entry};
for(const name of ['@earendil-works/pi-ai','@earendil-works/pi-ai/compat','@earendil-works/pi-agent-core','@earendil-works/pi-tui','typebox','typebox/value'])alias[name]=fileURLToPath(selected.esmResolve(name));
const jiti=createJiti(import.meta.url,{alias,interopDefault:true,fsCache:false});
const {default:register}=await jiti.import('../extensions/stream-checkpoint/index.ts');
const {defaults}=await jiti.import('../extensions/stream-checkpoint/settings.ts');

assert.equal(typeof sdk.ExtensionRunner,'function');assert.equal(typeof sdk.createExtensionRuntime,'function');

function fixture(t, beforeApply=async()=>{}) {
 const root=mkdtempSync(join(tmpdir(),'streams-hc-sdk-')),holder={generation:0},children=[],sessions=[],errors=[];
 t.after(async()=>{await holder.current?.retire();rmSync(root,{recursive:true,force:true});});
 const services={cwd:root,agentDir:root,modelRuntime:{async settleCredentialWrites(){}}};
 function session(manager,event={type:'session_start',reason:'startup'}) {
  const handlers=new Map();
  register({on:(name,handler)=>handlers.set(name,[...(handlers.get(name)??[]),handler]),registerCommand(){},registerTool(){},appendEntry(){}},
   {agentDir:root,contextState:holder,settings:{...defaults,reporting:'manual',enrollment:'manual'},child:ctx=>{
    const child={id:ctx.sessionManager.getSessionId(),stops:0,async stop(){this.stops++;},async run(){return {status:'success',result:{local:'unchanged',sharing:'local-only',domain:null}};}};
    children.push(child);return child;
   }});
  const extensionRunner=new sdk.ExtensionRunner([{path:'<streams>',handlers}],sdk.createExtensionRuntime(),root,manager,{});
  extensionRunner.onError(error=>errors.push(error.error));
  const value={sessionManager:manager,sessionFile:manager.getSessionFile(),startEvent:event,extensionRunner,handlers,disposed:false,
   async abort(){},dispose(){this.disposed=true;extensionRunner.invalidate();},
  };
  sessions.push(value);return value;
 }
 const initial=session(sdk.SessionManager.inMemory(root));
 let creations=0;
 const runtime=new sdk.AgentSessionRuntime(initial,services,async input=>{
  const value=session(input.sessionManager,input.sessionStartEvent);await beforeApply(value,++creations);
  return {session:value,services,diagnostics:[]};
 });
 // InteractiveMode binds extensions after AgentSessionRuntime.apply(), not inside createRuntime().
 runtime.setRebindSession(value=>value.extensionRunner.emit(value.startEvent));
 return {holder,children,sessions,errors,runtime,initial};
}
const start=value=>value.extensionRunner.emit(value.startEvent);
const shutdown=value=>value.extensionRunner.emit({type:'session_shutdown'});
function deferred(){let resolve;const promise=new Promise(done=>{resolve=done;});return {promise,resolve};}

test('selected SDK ordinary replacement retires the old integration before binding the new manager',async t=>{
 const {holder,children,errors,runtime,initial}=fixture(t);
 await start(initial);const previous=holder.current;
 await runtime.newSession();
 assert.equal(previous.retired,true);assert.equal(children[0].stops,1);
 assert.notEqual(holder.current.conversation.native_id,previous.conversation.native_id);
 assert.equal(holder.current.conversation.native_id,runtime.session.sessionManager.getSessionId());
 assert.equal(holder.current.session_file,null);assert.equal(children.length,2);
 assert.throws(()=>initial.extensionRunner.createContext().sessionManager,/stale/);
 await shutdown(runtime.session);assert.equal(children[1].stops,1);assert.deepEqual(errors,[]);
});

test('selected SDK out-of-order creation binds the applied runtime and fences the still-valid displaced runner',async t=>{
 const entered=deferred(),release=deferred();
 const {holder,children,errors,runtime,initial}=fixture(t,async(value,number)=>{
  if(number===1){entered.resolve(value);await release.promise;}
 });
 await start(initial);
 const first=runtime.newSession();
 try {
  const waiting=await entered.promise;
  await runtime.newSession();
  const displaced=runtime.session,displacedId=displaced.sessionManager.getSessionId();
  assert.equal(holder.current.conversation.native_id,displacedId);
  release.resolve();await first;
  assert.equal(runtime.session,waiting);
  assert.equal(holder.current.conversation.native_id,waiting.sessionManager.getSessionId());
  assert.equal(displaced.disposed,false,'SDK completion order did not dispose the displaced runtime');
  assert.equal(displaced.extensionRunner.createContext().sessionManager.getSessionId(),displacedId);
  assert.equal(children.find(child=>child.id===displacedId).stops,1,'process ownership retired the displaced helper anyway');
  await assert.rejects(()=>displaced.handlers.get('session_start')[0](displaced.startEvent,displaced.extensionRunner.createContext()),/Session changed/);
  assert.equal(holder.current.conversation.native_id,runtime.session.sessionManager.getSessionId());
  assert.equal(children.length,3);
  await shutdown(runtime.session);assert.ok(children.every(child=>child.stops===1));assert.deepEqual(errors,[]);
 } finally {release.resolve();await first;}
});
