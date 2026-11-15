import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { EventEmitter } from 'node:events';
import { PassThrough, Writable } from 'node:stream';
import { createJiti } from 'jiti';
const jiti=createJiti(import.meta.url,{interopDefault:true});
const {StreamsChild}=await jiti.import('../extensions/stream-checkpoint/child.ts');
const {decodeServerFrame}=await jiti.import('../extensions/stream-checkpoint/child-protocol.ts');
const {default:register}=await jiti.import('../extensions/stream-checkpoint/index.ts');
const {defaults}=await jiti.import('../extensions/stream-checkpoint/settings.ts');
const wire=JSON.parse(readFileSync(new URL('./fixtures/pi-child-v2.json',import.meta.url),'utf8'));
const tick=()=>new Promise(resolve=>setImmediate(resolve));
function respond(child,id,result={},warnings) {child.emitMessage({type:'response',id,outcome:warnings?{status:'saved_with_warnings',result,warnings}:{status:'success',result}});}
function fixture(t, sessionId='pi-session', env={}, piContext) {
 const children=[],launches=[],warnings=[];
 const spawn=(binary,argv,options)=>{
  const child=new EventEmitter(),messages=[];
  child.stdout=new PassThrough();child.stderr=new PassThrough();
  child.stdin=new Writable({write(chunk,_encoding,done){const message=JSON.parse(chunk.toString());messages.push(message);child.emit('message',message);done();},final(done){child.ended=true;done();}});
  child.kill=()=>{child.killed=true;queueMicrotask(()=>child.emit('close',null,'SIGTERM'));return true;};
  child.messages=messages;child.emitMessage=value=>child.stdout.write(JSON.stringify(value)+'\n');
  launches.push({binary,argv,options});children.push(child);return child;
 };
 const manager=new StreamsChild({sessionId,cwd:'/tmp',env:{STREAMS_BIN_PATH:process.execPath,...env},piContext,warning:text=>warnings.push(text),spawn});
 t.after(async()=>{const stopped=manager.stop();for(const child of children)child.emit('close',0);await stopped;});
 const ready=async()=>{
  const started=manager.start(),child=children.at(-1);
  child.on('message',message=>{if(message.request?.kind==='attach')respond(child,message.id,{native:'active'});});
  child.emitMessage({type:'ready',protocol:2});await started;return child;
 };
 return {manager,children,launches,warnings,ready};
}
test('shared Rust v2 server fixtures are the TypeScript decoding contract',()=>{
 for(const frame of wire.server)assert.deepEqual(decodeServerFrame(JSON.stringify(frame)),frame);
});
test('every shared typed request is sent unchanged, without argv or serialized stdin',async t=>{
 const f=fixture(t),child=await f.ready();
 for(const frame of wire.client.filter(frame=>frame.type==='request')) {
  const pending=f.manager.run(frame.request);await tick();
  assert.deepEqual(child.messages.at(-1),{type:'request',id:child.messages.at(-1).id,request:frame.request});
  assert.equal('argv' in child.messages.at(-1),false);assert.equal('input' in child.messages.at(-1),false);
  respond(child,child.messages.at(-1).id);await pending;
 }
 assert.equal(f.launches.length,1);
});
test('failed shutdown warns that native source cleanup may be incomplete',async t=>{
 const f=fixture(t),child=await f.ready(),stopped=f.manager.stop();child.emit('close',2);await stopped;
 assert.deepEqual(f.warnings,['Streams child shutdown failed; native source cleanup may be incomplete.']);assert.equal(f.children.length,1);
});
test('one launch, exact envelope, ready gating and out-of-order typed outcomes',async t=>{
 const f=fixture(t),first=f.manager.run({kind:'context'}),child=f.children[0];
 await tick();assert.deepEqual(child.messages,[]);
 assert.deepEqual(f.launches[0].argv.slice(0,5),['serve','--session-id','pi-session','--sdk-pid',String(process.pid)]);assert.equal(f.launches[0].options.cwd,'/tmp');
 assert.equal(f.launches[0].options.env.STREAMS_BIN_PATH,f.launches[0].binary);
 child.emitMessage({type:'ready',protocol:2});await tick();
 const second=f.manager.run({kind:'apply',request:{captured:true}});await tick();
 respond(child,child.messages[1].id,{receipt:true},['offline','retained']);
 assert.deepEqual(await second,{status:'saved_with_warnings',result:{receipt:true},warnings:['offline','retained']});
 assert.deepEqual(f.warnings,[],'request warnings are not unsolicited notifications');
 respond(child,child.messages[0].id,{context:true});assert.deepEqual(await first,{status:'success',result:{context:true}});
 assert.equal(f.launches.length,1);
});
test('typed intent is captured before readiness and is not retargeted by later caller mutations',async t=>{
 const f=fixture(t),intent={kind:'apply',request:{before:{id:'captured'},inputs:[{record:{id:'dependency'}}]}};
 const pending=f.manager.run(intent);intent.request.before.id='destination';intent.request.inputs[0].record.id='new dependency';
 const child=await f.ready();await tick();assert.deepEqual(child.messages[0].request,{kind:'apply',request:{before:{id:'captured'},inputs:[{record:{id:'dependency'}}]}});
 respond(child,child.messages[0].id);await pending;
});
test('hc request context is captured before readiness and reattach uses the current file',async t=>{
 let file=null;const context=()=>({conversation:{harness:'pi',native_id:'pi-session'},session_file:file});
 const f=fixture(t,'pi-session',{},context),pending=f.manager.run({kind:'session_started'});file='/saved.jsonl';
 const child=await f.ready();await tick();assert.deepEqual(child.messages[0].pi_context,{conversation:{harness:'pi',native_id:'pi-session'},session_file:null});
 respond(child,child.messages[0].id,{native:'active',display:'suppressed'});await pending;child.emit('close',1);
 const replacement=await f.ready();assert.deepEqual(replacement.messages[0].pi_context,context());
});
test('structured domain errors preserve kind and message without a process-style code',async t=>{
 const f=fixture(t),child=await f.ready(),pending=f.manager.run({kind:'open_card'});await tick();
 const outcome={status:'error',error:{kind:'native-context-changed',message:'caller moved'}};
 child.emitMessage({type:'response',id:child.messages.at(-1).id,outcome});assert.deepEqual(await pending,outcome);
 assert.equal(f.launches.length,1);
});
test('abort and deadlines cancel only their request, discard late responses and preserve the child',async t=>{
 const f=fixture(t),child=await f.ready(),controller=new AbortController();
 const request=f.manager.run({kind:'open_card'},{timeout:1000,signal:controller.signal});await tick();
 const id=child.messages[0].id;controller.abort(Error('cancelled by caller'));await assert.rejects(request,/cancelled by caller/);
 assert.deepEqual(child.messages.at(-1),{type:'cancel',id});respond(child,id);assert.equal(child.killed,undefined);
 const timed=f.manager.run({kind:'sync'},{timeout:1});await assert.rejects(timed,/timed out.*uncertain/);
 assert.equal(child.messages.at(-1).type,'cancel');assert.equal(child.killed,undefined);
 const next=f.manager.run({kind:'list_streams'});await tick();respond(child,child.messages.at(-1).id);await next;assert.equal(f.launches.length,1);
});
test('already-aborted signals never spawn; startup cancellation sends no unsent ID',async t=>{
 const f=fixture(t),controller=new AbortController();controller.abort(Error('before start'));
 await assert.rejects(f.manager.run({kind:'list_streams'},{timeout:100,signal:controller.signal}),/before start/);assert.equal(f.launches.length,0);
 const timed=f.manager.run({kind:'list_streams'},{timeout:1});await assert.rejects(timed,/timed out/);assert.deepEqual(f.children[0].messages,[]);
 await f.ready();assert.deepEqual(f.children[0].messages,[]);
});
test('background UI facts and warnings never dispatch another request',async t=>{
 const f=fixture(t),child=await f.ready(),views=[];const unsubscribe=f.manager.subscribe(view=>views.push(view));
 child.emitMessage({type:'view',view:{desired_pi_name:'Saved title'}});child.emitMessage({type:'view',view:{desired_pi_name:null}});
 child.emitMessage({type:'warning',message:'subscription lost'});
 assert.deepEqual(views,[{desired_pi_name:'Saved title'},{desired_pi_name:null}]);assert.deepEqual(f.warnings,['subscription lost']);
 assert.deepEqual(child.messages,[]);assert.equal(f.launches.length,1);unsubscribe();
});
test('restart rejects in-flight work, attaches before new requests and never replays startup or effects',async t=>{
 const f=fixture(t),first=await f.ready(),startup=f.manager.run({kind:'session_started'});await tick();
 respond(first,first.messages[0].id,{native:'active',display:'opened'});await startup;
 const pending=f.manager.run({kind:'open_card'});await tick();first.emit('close',1);await assert.rejects(pending,/exited/);
 const second=f.children[1],next=f.manager.run({kind:'context'});second.emitMessage({type:'ready',protocol:2});await tick();
 assert.deepEqual(second.messages.map(frame=>frame.request),[{kind:'attach'},{kind:'context'}]);
 respond(second,second.messages[1].id,{catalog:true});await next;respond(second,second.messages[0].id,{native:'active'});
 assert.equal(f.launches.length,2);assert.match(f.warnings[0],/Restarting once/);
 second.emit('close',1);assert.match(f.warnings[1],/stopped/);await assert.rejects(f.manager.run({kind:'context'}),/stopped/);assert.equal(f.launches.length,2);
});
for(const outcome of [{status:'error',error:{kind:'runtime',message:'origin not proved'}},{status:'success',result:{}}])test('native attach failure warns but leaves catalog requests available',async t=>{
 const f=fixture(t),first=await f.ready(),startup=f.manager.run({kind:'session_started'});await tick();respond(first,first.messages[0].id,{native:'inactive',reason:'not_enrolled'});await startup;first.emit('close',1);
 const second=f.children[1],next=f.manager.run({kind:'context'});second.emitMessage({type:'ready',protocol:2});await tick();
 second.emitMessage({type:'response',id:second.messages[0].id,outcome});respond(second,second.messages[1].id,{catalog:true});assert.deepEqual(await next,{status:'success',result:{catalog:true}});
 assert.equal(f.launches.length,2);assert.equal(second.killed,undefined);assert.equal(f.warnings.length,2);assert.match(f.warnings[1],/origin not proved|attach acknowledgement/);
});
test('catalog-only restart remains ready-only despite inherited Herdr environment',async t=>{
 const f=fixture(t,'12345678-1234-1234-1234-123456789abc',{HERDR_ENV:'1',HERDR_PANE_ID:'inherited'}),first=await f.ready(),pending=f.manager.run({kind:'context'});await tick();first.emit('close',1);await assert.rejects(pending,/exited/);
 const second=await f.ready();assert.deepEqual(second.messages,[]);const next=f.manager.run({kind:'context'});await tick();assert.deepEqual(second.messages.map(frame=>frame.request),[{kind:'context'}]);respond(second,second.messages[0].id);await next;
});
test('a crash before native startup readiness cannot replay session_started into the replacement',async t=>{
 const f=fixture(t),pending=f.manager.run({kind:'session_started'}),first=f.children[0];first.emit('close',1);await assert.rejects(pending,/exited/);
 const second=await f.ready();assert.deepEqual(second.messages.map(frame=>frame.request),[{kind:'attach'}]);
});
test('native attach timeout cancels only attachment and does not stop catalog transport',async t=>{
 t.mock.timers.enable({apis:['setTimeout']});const f=fixture(t),first=await f.ready(),startup=f.manager.run({kind:'session_started'});await tick();respond(first,first.messages[0].id,{native:'active',display:'suppressed'});await startup;first.emit('close',1);
 const second=f.children[1],ready=f.manager.start();second.emitMessage({type:'ready',protocol:2});await ready;t.mock.timers.tick(120000);
 assert.deepEqual(second.messages.at(-1),{type:'cancel',id:second.messages[0].id});assert.match(f.warnings.at(-1),/attach timed out/);assert.equal(second.killed,undefined);
 const next=f.manager.run({kind:'context'});await tick();respond(second,second.messages.at(-1).id,{catalog:true});await next;
});
for(const message of [{type:'ready',protocol:1},{type:'response',id:1,outcome:{status:'success',result:{}}}])test('unsupported first frame fails without a v1 fallback',async t=>{
 const f=fixture(t),started=f.manager.start();f.children[0].emitMessage(message);await assert.rejects(started,/handshake/);assert.equal(f.children[0].killed,true);assert.equal(f.launches.length,2);await f.ready();
});
for(const message of [
 'not JSON', '{"type":"warning","message":"one","message":"two"}', '{"type":"view","view":{"desired_pi_name":null,"desired_pi_\\u006eame":"duplicate"}}',
 {type:'event',event:'scope',value:null,reason:'old loop'}, {type:'view',view:{}}, {type:'view',view:[]}, {type:'view',view:{desired_pi_name:null,scope:{}}},
 {type:'response',id:0,outcome:{status:'success',result:{}}}, {type:'response',id:1,outcome:{status:'success',result:[]}},
 {type:'response',id:1,outcome:{status:'saved_with_warnings',result:{},warnings:[]}},
 {type:'response',id:1,outcome:{status:'error',error:{kind:'runtime',message:'bad',scope:{}}}},
 {type:'response',id:1,code:0,value:{},notices:[]}, {type:'ready',protocol:2},
])test(`malformed protocol terminates the child: ${JSON.stringify(message)}`,async t=>{
 const f=fixture(t),child=await f.ready();if(typeof message==='string')child.stdout.write(message+'\n');else child.emitMessage(message);
 assert.equal(child.killed,true);assert.equal(f.launches.length,1,'replacement waits for actual close');await tick();assert.equal(f.launches.length,2);await f.ready();
});
test('ready then malformed framing cannot dispatch a stale write into the replacement',async t=>{
 const f=fixture(t),request=f.manager.run({kind:'apply',request:{}});
 f.children[0].stdout.write('{"type":"ready","protocol":2}\nnot-json\n');
 await assert.rejects(request,/lifetime ended/);assert.equal(f.launches.length,2);assert.deepEqual(f.children[1].messages,[]);await f.ready();
});
test('aborting a draining request writes no cancel after EOF and never kills the child',async t=>{
 const f=fixture(t),child=await f.ready(),controller=new AbortController();
 const request=f.manager.run({kind:'list_streams'},{timeout:1000,signal:controller.signal});await tick();
 const stopping=f.manager.stop();controller.abort(Error('cancelled during shutdown'));await assert.rejects(request,/cancelled during shutdown/);await tick();
 assert.equal(child.killed,undefined);assert.deepEqual(child.messages.map(value=>value.type),['request']);child.emit('close',0);await stopping;
});
test('attach warnings are not emitted after shutdown starts, but its accepted reply still drains',async t=>{
 const f=fixture(t),first=await f.ready(),startup=f.manager.run({kind:'session_started'});await tick();respond(first,first.messages[0].id,{native:'active',display:'suppressed'});await startup;first.emit('close',1);
 const second=f.children[1],attached=f.manager.start();second.emitMessage({type:'ready',protocol:2});await tick();
 const stopped=f.manager.stop();respond(second,second.messages[0].id,{native:'active'},['Retired attach warning']);await attached;second.emit('close',0);await stopped;
 assert.equal(f.warnings.length,1);assert.match(f.warnings[0],/Restarting once/);
});
test('ready has a fixed ten-second deadline',async t=>{
 t.mock.timers.enable({apis:['setTimeout']});const f=fixture(t),started=f.manager.start();
 t.mock.timers.tick(9999);assert.equal(f.launches.length,1);t.mock.timers.tick(1);await assert.rejects(started,/handshake timed out/);assert.equal(f.launches.length,2);await f.ready();
});
test('shutdown closes stdin, drains accepted responses and never restarts',async t=>{
 const f=fixture(t),child=await f.ready(),request=f.manager.run({kind:'sync'});await tick();
 const stopped=f.manager.stop();assert.equal(child.ended,true);assert.equal(child.killed,undefined);
 child.emitMessage({type:'warning',message:'Retired background warning'});child.emitMessage({type:'view',view:{desired_pi_name:'Retired name'}});
 respond(child,child.messages[0].id);await request;child.emit('close',0);await stopped;assert.deepEqual(f.warnings,[]);assert.equal(f.launches.length,1);
});
test('Pi lifecycle shares one child, branch events do no native work and switching closes the old pipe',async t=>{
 const handlers=new Map(),tools=new Map(),instances=[];
 const catalog={catalog_source:'catalog',host_id:'host',title_defaults:{stream:'automatic',session:'automatic'},stream:null,session:null,sessions:[]};
 const ctx={cwd:'/tmp',hasUI:false,sessionManager:{getSessionId:()=> 'first'},ui:{setStatus:()=>{},notify:assert.fail}};
 register({on:(name,handler)=>handlers.set(name,handler),registerCommand:()=>{},registerTool:value=>tools.set(value.name,value),appendEntry:()=>{}},
  {settings:{...defaults,reporting:'manual',enrollment:'manual'},child:()=>{const f=fixture(t);instances.push(f);return f.manager;}});
 const connect=async f=>{const child=f.children[0];child.on('message',message=>{if(message.type==='request')respond(child,message.id,message.request.kind==='sync'?{local:'unchanged',sharing:'local-only',domain:null}:catalog);});await f.ready();return child;};
 const starting=handlers.get('session_start')({},ctx);await tick();const first=await connect(instances[0]);await starting;
 for(let i=0;i<20;i++){handlers.get('session_tree')({},ctx);handlers.get('agent_end')({messages:[]},ctx);await tools.get('stream_read').execute('read',{},undefined,undefined,ctx);}
 assert.equal(instances[0].launches.length,1);assert.ok(first.messages.every(frame=>['sync','context'].includes(frame.request.kind)));
 const next={...ctx,sessionManager:{getSessionId:()=> 'next'}},replacing=handlers.get('session_start')({},next);await tick();
 assert.equal(first.ended,true);first.emit('close',0);await tick();const second=await connect(instances[1]);await replacing;
 assert.equal(second.ended,undefined);const final=handlers.get('session_shutdown')({},next);await tick();second.emit('close',0);await final;
});
