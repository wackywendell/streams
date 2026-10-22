import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { PassThrough, Writable } from 'node:stream';
import { createJiti } from 'jiti';
const jiti=createJiti(import.meta.url,{interopDefault:true});
const {StreamsChild}=await jiti.import('../extensions/stream-checkpoint/child.ts');
const {NativeController}=await jiti.import('../extensions/stream-checkpoint/native.ts');
const {default:register}=await jiti.import('../extensions/stream-checkpoint/index.ts');
const {defaults}=await jiti.import('../extensions/stream-checkpoint/settings.ts');

function fixture(t, sessionId='pi-session') {
 const children=[],launches=[],warnings=[];
 const spawn=(binary,argv,options)=>{
  const child=new EventEmitter(),messages=[];
  child.stdout=new PassThrough();child.stderr=new PassThrough();
  child.stdin=new Writable({write(chunk,_encoding,done){const message=JSON.parse(chunk.toString());messages.push(message);child.emit('message',message);done();},final(done){child.ended=true;done();}});
  child.kill=()=>{child.killed=true;queueMicrotask(()=>child.emit('close',null,'SIGTERM'));return true;};
  child.messages=messages;child.emitMessage=value=>child.stdout.write(JSON.stringify(value)+'\n');
  launches.push({binary,argv,options});children.push(child);return child;
 };
 const manager=new StreamsChild({sessionId,cwd:'/tmp',env:{STREAMS_BIN_PATH:process.execPath},warning:text=>warnings.push(text),spawn});
 t.after(async()=>{const stopped=manager.stop();for(const child of children)child.emit('close',0);await stopped;});
 const ready=async()=>{const started=manager.start(),child=children.at(-1);child.emitMessage({type:'ready',protocol:1});await started;return child;};
 return {manager,children,launches,warnings,ready};
}
const tick=()=>new Promise(resolve=>setImmediate(resolve));
function respond(child,id,value={},code=0,notices=[]) {child.emitMessage({type:'response',id,code,value,notices});}

test('one launch, exact launch envelope, ready gating and out-of-order CLI-compatible results',async t=>{
 const f=fixture(t),first=f.manager.run(['agent','context','--json']),child=f.children[0];
 await tick();assert.deepEqual(child.messages,[]);
 assert.deepEqual(f.launches[0].argv.slice(0,3),['serve','--session-id','pi-session']);assert.equal(f.launches[0].options.cwd,'/tmp');
 assert.equal(f.launches[0].options.env.STREAMS_RUST_BIN,f.launches[0].binary);
 child.emitMessage({type:'ready',protocol:1});await tick();
 const second=f.manager.run(['agent','apply','--json'],JSON.stringify({save:true}));await tick();
 assert.equal(f.launches.length,1);assert.deepEqual(child.messages[1].input,{save:true});assert.ok(!('input' in child.messages[0]));
 respond(child,child.messages[1].id,{receipt:true},2,['offline','retained']);
 assert.deepEqual(await second,{code:2,stdout:'{"receipt":true}',stderr:'offline\nretained'});
 respond(child,child.messages[0].id,{context:true});assert.equal((await first).code,0);
});
test('abort and deadlines cancel only their request, discard late responses and preserve the child',async t=>{
 const f=fixture(t),child=await f.ready(),controller=new AbortController();
 const request=f.manager.run(['workspace','capture','id','--json'],undefined,{timeout:1000,signal:controller.signal});await tick();
 const id=child.messages[0].id;controller.abort(Error('cancelled by caller'));await assert.rejects(request,/cancelled by caller/);
 assert.deepEqual(child.messages.at(-1),{type:'cancel',id});respond(child,id);assert.equal(child.killed,undefined);
 const timed=f.manager.run(['sync'],undefined,{timeout:1});await assert.rejects(timed,/timed out.*uncertain/);
 assert.equal(child.messages.at(-1).type,'cancel');assert.equal(child.killed,undefined);
 const next=f.manager.run(['list','--json']);await tick();respond(child,child.messages.at(-1).id);await next;assert.equal(f.launches.length,1);
});
test('already-aborted signals never start a process; startup cancellation sends no unsent id',async t=>{
 const f=fixture(t),controller=new AbortController();controller.abort(Error('before start'));
 await assert.rejects(f.manager.run(['list'],undefined,{timeout:100,signal:controller.signal}),/before start/);assert.equal(f.launches.length,0);
 const timed=f.manager.run(['list'],undefined,{timeout:1});await assert.rejects(timed,/timed out/);assert.deepEqual(f.children[0].messages,[]);
 await f.ready();assert.deepEqual(f.children[0].messages,[]);
});
test('scope and warning events are delivered without requests or spawns',async t=>{
 const f=fixture(t),child=await f.ready();let count=0;const unsubscribe=f.manager.subscribe(()=>count++);
 const waiting=f.manager.waitForScope();child.emitMessage({type:'event',event:'scope',value:null,reason:'not enrolled'});
 assert.deepEqual(await waiting,{value:null,reason:'not enrolled'});
 child.emitMessage({type:'event',event:'scope',value:null,reason:'workspace unbound'});child.emitMessage({type:'event',event:'warning',message:'subscription lost'});
 assert.equal(count,2);assert.equal(f.manager.revision,2);assert.deepEqual(f.warnings,['subscription lost']);assert.equal(f.launches.length,1);unsubscribe();
});
test('unexpected exit rejects in-flight work, restarts once and stops after the second exit',async t=>{
 const f=fixture(t),child=await f.ready(),pending=f.manager.run(['list']);await tick();
 child.emit('close',0);await assert.rejects(pending,/exited/);assert.equal(f.launches.length,2);assert.match(f.warnings[0],/Restarting once/);
 const restarted=await f.ready();restarted.emit('close',1);assert.equal(f.launches.length,2);assert.match(f.warnings[1],/stopped/);
 await assert.rejects(f.manager.run(['list']),/stopped/);assert.equal(f.launches.length,2);
});
for(const message of [{type:'ready',protocol:2},{type:'response',id:1,code:0,value:{},notices:[]}])test('invalid first message is a failed start',async t=>{
 const f=fixture(t),started=f.manager.start();f.children[0].emitMessage(message);await assert.rejects(started,/handshake/);assert.equal(f.children[0].killed,true);assert.equal(f.launches.length,2);
 await f.ready();
});
for(const message of ['not JSON',{type:'event',event:'unknown'},{type:'event',event:'scope',value:null},{type:'event',event:'scope',value:{}},{type:'response',id:1,code:0,value:{}},{type:'ready',protocol:1}])test(`malformed protocol crashes the child: ${JSON.stringify(message)}`,async t=>{
 const f=fixture(t),child=await f.ready();if(typeof message==='string')child.stdout.write(message+'\n');else child.emitMessage(message);
 assert.equal(child.killed,true);assert.equal(f.launches.length,2);await f.ready();
});
test('ready followed by malformed framing cannot dispatch a write into the unready replacement',async t=>{
 const f=fixture(t),request=f.manager.run(['agent','apply'],'{}');
 f.children[0].stdout.write('{"type":"ready","protocol":1}\nnot-json\n');
 await assert.rejects(request,/lifetime ended/);assert.equal(f.launches.length,2);assert.deepEqual(f.children[1].messages,[]);await f.ready();
});
test('aborting a draining request does not write after stdin EOF or kill the child',async t=>{
 const f=fixture(t),child=await f.ready(),controller=new AbortController();
 const request=f.manager.run(['list'],undefined,{timeout:1000,signal:controller.signal});await tick();
 const stopping=f.manager.stop();controller.abort(Error('cancelled during shutdown'));await assert.rejects(request,/cancelled during shutdown/);await tick();
 assert.equal(child.killed,undefined);assert.deepEqual(child.messages.map(value=>value.type),['request']);child.emit('close',0);await stopping;
});
test('ready has a fixed ten-second deadline',async t=>{
 t.mock.timers.enable({apis:['setTimeout']});const f=fixture(t),started=f.manager.start();
 t.mock.timers.tick(9999);assert.equal(f.launches.length,1);t.mock.timers.tick(1);await assert.rejects(started,/handshake timed out/);assert.equal(f.launches.length,2);await f.ready();
});
test('shutdown closes stdin, drains accepted responses and never restarts',async t=>{
 const f=fixture(t),child=await f.ready(),request=f.manager.run(['sync']);await tick();
 const stopped=f.manager.stop();assert.equal(child.ended,true);assert.equal(child.killed,undefined);
 respond(child,child.messages[0].id);await request;child.emit('close',0);await stopped;assert.deepEqual(f.warnings,[]);assert.equal(f.launches.length,1);
});

function nativeFixture(t) {
 const env={...process.env};Object.assign(process.env,{HERDR_ENV:'1',HERDR_PANE_ID:'pane',HERDR_SOCKET_PATH:'/socket'});
 t.after(()=>{for(const key of Object.keys(process.env))delete process.env[key];Object.assign(process.env,env);});
 const id='12345678-1234-1234-1234-123456789abc',f=fixture(t,id),entries=[];
 const observation={version:1,scope:{catalogSource:'catalog',hostId:'host',streamId:'stream',sessionId:'session',nativeSessionId:id},
  native:{pane_id:'pane',tab_id:'tab',workspace_id:'workspace'},context:{socket:'/socket',paneId:'pane',terminalId:'terminal',tabId:'tab',workspaceId:'workspace',binding:null},
  session:{title:'Initial title',title_mode:'automatic'},config:{automatic:true,sessionNaming:false}};
 const ctx={cwd:'/tmp',hasUI:true,mode:'tui',sessionManager:{getSessionId:()=>id,getEntries:()=>entries},ui:{notify:assert.fail}};
 const pi={appendEntry:(customType,data)=>entries.push({type:'custom',customType,data}),getSessionName:()=>undefined};
 const controller=new NativeController({pi,ctx,child:f.manager,warning:text=>f.warnings.push(text)});
 t.after(async()=>{controller.stop();await controller.idle();});
 const wire=child=>{
  child.on('message',message=>{if(message.type!=='request')return;
   const command=message.argv.find(arg=>['open','close','project','capture'].includes(arg));
   if(f.fail===command){f.fail=undefined;f.beforeFailure?.();respond(child,message.id,{error:{kind:'native-context-changed',message:'owner moved'}},1);return;}
   const expected=JSON.parse(message.argv[message.argv.indexOf('--expect')+1]);
   const native={workspace_id:expected.context.workspaceId,owner_pane_id:expected.context.paneId,owner_terminal_id:expected.context.terminalId,session_id:expected.scope.sessionId,native_session_id:id,binding:expected.context.binding};
   respond(child,message.id,command==='open'?{display:'opened',native}:command==='close'?{closed:true}:command==='project'?{names:{workspace:'unchanged',tab:'unchanged'},native}:{captured:true},f.code??0);
  });
 };
 const scope=child=>child.emitMessage({type:'event',event:'scope',value:structuredClone(observation)});
 return Object.assign(f,{controller,observation,wire,scope});
}
test('native events adopt valid moves, retry user actions only after scope, and spawn no per-event processes',async t=>{
 const f=nativeFixture(t),child=await f.ready();f.wire(child);f.scope(child);f.controller.start();await f.controller.idle();
 const commands=()=>child.messages.filter(message=>message.type==='request').map(message=>message.argv);
 assert.equal(commands().filter(argv=>argv.includes('open')).length,1);assert.ok(commands().every(argv=>!argv.includes('observe')));
 f.observation.context.tabId=f.observation.native.tab_id='new-tab';f.observation.context.workspaceId=f.observation.native.workspace_id='new-workspace';
 f.observation.context.paneId=f.observation.native.pane_id='new-pane';f.observation.context.socket='/moved-socket';
 f.observation.scope.streamId='new-stream';f.observation.session.title='Moved title';f.scope(child);await f.controller.idle();
 const project=commands().findLast(argv=>argv.includes('project'));assert.equal(JSON.parse(project[project.indexOf('--expect')+1]).context.workspaceId,'new-workspace');
 f.fail='close';let settled=false;const closing=f.controller.close().then(()=>{settled=true;});await f.controller.idle();
 assert.equal(settled,false);const count=commands().length;f.controller.update();await f.controller.idle();assert.equal(commands().length,count);
 f.observation.session.title='Next scope';f.scope(child);await closing;await f.controller.idle();assert.equal(commands().filter(argv=>argv.includes('close')).length,2);
 f.fail='project';f.observation.session.title='Automatic race';f.scope(child);await f.controller.idle();const dropped=commands().length;
 await tick();assert.equal(commands().length,dropped);assert.equal(f.launches.length,1);assert.equal(commands().filter(argv=>argv.includes('open')).length,1);
});
for(const timing of ['before-failure','after-failure'])test(`scope retries preserve open then close order: ${timing}`,async t=>{
 const f=nativeFixture(t),child=await f.ready();f.wire(child);f.scope(child);f.controller.start();await f.controller.idle();
 const before=child.messages.length;f.fail='open';
 const move=()=>{f.observation.session.title='Moved';f.scope(child);};if(timing==='before-failure')f.beforeFailure=move;
 const opening=f.controller.open(),closing=f.controller.close();
 if(timing==='after-failure'){await f.controller.idle();move();}
 await Promise.all([opening,closing]);await f.controller.idle();
 assert.deepEqual(child.messages.slice(before).filter(message=>message.argv?.includes('open')||message.argv?.includes('close')).map(message=>message.argv.includes('open')?'open':'close'),['open','open','close']);
});
test('cached unbound startup recovers on its first subscribed enrollment scope',async t=>{
 const f=nativeFixture(t),child=await f.ready();f.wire(child);child.emitMessage({type:'event',event:'scope',value:null,reason:'unbound'});
 f.controller.start();await f.controller.idle();assert.deepEqual(child.messages,[]);
 f.scope(child);await f.controller.idle();assert.ok(child.messages.some(message=>message.argv?.includes('open')));
});
test('a restarted child updates native scope but never reopens the card',async t=>{
 const f=nativeFixture(t),first=await f.ready();f.wire(first);f.scope(first);f.controller.start();await f.controller.idle();
 await f.controller.close();first.emit('close',1);const second=await f.ready();f.wire(second);f.scope(second);await f.controller.idle();
 assert.equal(f.launches.length,2);assert.equal(second.messages.some(message=>message.argv?.includes('open')),false);
 assert.ok(second.messages.some(message=>message.argv?.includes('project')));
});
test('enabling automatic cards after startup cannot create a new automatic-open trigger',async t=>{
 const f=nativeFixture(t),child=await f.ready();f.wire(child);f.observation.config.automatic=false;f.scope(child);f.controller.start();await f.controller.idle();
 f.observation.config.automatic=true;f.scope(child);await f.controller.idle();assert.equal(child.messages.some(message=>message.argv?.includes('open')),false);
});
test('native acknowledgements still require code zero, unlike accepted catalog save receipts',async t=>{
 const f=nativeFixture(t),child=await f.ready();f.wire(child);f.scope(child);f.controller.start();await f.controller.idle();
 f.code=2;await assert.rejects(f.controller.close(),/failed \(2\)/);
});
test('a second crash rejects user actions deferred for the next scope',async t=>{
 const f=nativeFixture(t),first=await f.ready();f.wire(first);f.scope(first);f.controller.start();await f.controller.idle();
 first.emit('close',1);const second=await f.ready();f.wire(second);f.scope(second);await f.controller.idle();
 f.fail='close';const closing=f.controller.close();const rejected=assert.rejects(closing,/stopped/);await f.controller.idle();second.emit('close',1);await rejected;
});
test('a crash before the initial scope still projects the restarted scope without automatic open',async t=>{
 const f=nativeFixture(t),first=await f.ready();f.wire(first);f.controller.start();await tick();
 first.emit('close',1);const second=await f.ready();f.wire(second);f.scope(second);await f.controller.idle();
 assert.ok(second.messages.some(message=>message.argv?.includes('project')));assert.equal(second.messages.some(message=>message.argv?.includes('open')),false);
});
test('Pi handlers share one child, session switches close stdin, and stale shutdowns cannot stop a replacement',async t=>{
 const handlers=new Map(),tools=new Map(),instances=[];
 const catalog={catalog_source:'catalog',host_id:'host',stream:null,session:null,sessions:[],authority:null};
 const ctx={cwd:'/tmp',hasUI:false,sessionManager:{getSessionId:()=> 'first'},ui:{setStatus:()=>{},notify:assert.fail}};
 register({on:(name,handler)=>handlers.set(name,handler),registerCommand:()=>{},registerTool:value=>tools.set(value.name,value),appendEntry:()=>{}},
  {settings:{...defaults,reporting:'manual',enrollment:'manual'},child:()=>{const f=fixture(t);instances.push(f);return f.manager;}});
 const wire=async f=>{const child=f.children[0];child.on('message',message=>{if(message.type==='request')respond(child,message.id,message.argv[0]==='sync'?{local:'unchanged',sharing:'local-only',domain:null}:catalog);});await f.ready();return child;};
 const starting=handlers.get('session_start')({},ctx),first=await wire(instances[0]);await starting;
 for(let i=0;i<20;i++){handlers.get('session_tree')({},ctx);handlers.get('agent_end')({messages:[]},ctx);await tools.get('stream_read').execute('read',{},undefined,undefined,ctx);}
 assert.equal(instances[0].launches.length,1);
 let held;first.removeAllListeners('message');first.on('message',message=>{held=message;});
 const shutting=handlers.get('session_shutdown')({},ctx);await tick();
 const next={...ctx,sessionManager:{getSessionId:()=> 'next'}};const replacing=handlers.get('session_start')({},next);await tick();
 assert.equal(first.ended,true);first.emit('close',0);await tick();const second=await wire(instances[1]);await replacing;await shutting;
 assert.equal(second.ended,undefined);assert.equal(instances[1].launches.length,1);assert.equal(held.argv[0],'sync');
 const final=handlers.get('session_shutdown')({},next);await tick();second.emit('close',0);await final;
});
