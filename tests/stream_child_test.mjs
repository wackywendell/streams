import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { PassThrough, Writable } from 'node:stream';
import { createJiti } from 'jiti';
const jiti=createJiti(import.meta.url,{interopDefault:true});
const {StreamsChild}=await jiti.import('../extensions/stream-checkpoint/child.ts');

function fixture(t) {
 const children=[],launches=[],warnings=[];
 const spawn=(binary,argv,options)=>{
  const child=new EventEmitter(),messages=[];
  child.stdout=new PassThrough();child.stderr=new PassThrough();
  child.stdin=new Writable({write(chunk,_encoding,done){messages.push(JSON.parse(chunk.toString()));done();},final(done){child.ended=true;done();}});
  child.kill=()=>{child.killed=true;queueMicrotask(()=>child.emit('close',null,'SIGTERM'));return true;};
  child.messages=messages;child.emitMessage=value=>child.stdout.write(JSON.stringify(value)+'\n');
  launches.push({binary,argv,options});children.push(child);return child;
 };
 const manager=new StreamsChild({sessionId:'pi-session',cwd:'/tmp',env:{STREAMS_BIN_PATH:process.execPath},warning:text=>warnings.push(text),spawn});
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
 child.emitMessage({type:'event',event:'scope',value:{moved:true}});child.emitMessage({type:'event',event:'warning',message:'subscription lost'});
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
for(const message of ['not JSON',{type:'event',event:'unknown'},{type:'event',event:'scope',value:null},{type:'response',id:1,code:0,value:{}},{type:'ready',protocol:1}])test(`malformed protocol crashes the child: ${JSON.stringify(message)}`,async t=>{
 const f=fixture(t),child=await f.ready();if(typeof message==='string')child.stdout.write(message+'\n');else child.emitMessage(message);
 assert.equal(child.killed,true);assert.equal(f.launches.length,2);await f.ready();
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
