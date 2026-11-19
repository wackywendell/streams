import assert from 'node:assert/strict';
import test from 'node:test';
import {EventEmitter} from 'node:events';
import {PassThrough, Writable} from 'node:stream';
import {createJiti} from 'jiti';
const jiti=createJiti(import.meta.url,{interopDefault:true});
const {default:register}=await jiti.import('../extensions/stream-checkpoint/index.ts');
const {StreamsChild}=await jiti.import('../extensions/stream-checkpoint/child.ts');
const {defaults}=await jiti.import('../extensions/stream-checkpoint/settings.ts');
const {checkoutSource}=await jiti.import('../extensions/stream-checkpoint/streams-bin.ts');
const turn=()=>new Promise(resolve=>setImmediate(resolve));
const deferred=()=>{let resolve;const promise=new Promise(yes=>{resolve=yes;});return {promise,resolve};};
function fixture(contextState={generation:0}) {
 const handlers=new Map(),tools=new Map(),children=[],notices=[];
 let id='first',file=null;
 const ctx={cwd:'/tmp',hasUI:false,sessionManager:{getSessionId:()=>id,getSessionFile:()=>file},ui:{setStatus(){},notify:text=>notices.push(text)}};
 register({on:(name,handler)=>handlers.set(name,handler),registerCommand(){},registerTool:tool=>tools.set(tool.name,tool),appendEntry(){}},
  {...(contextState?{contextState}:{}),settings:{...defaults,reporting:'manual',enrollment:'manual'},child:()=>{
   const session=id,child={session,stops:0,requests:[],stop(){this.stops++;return this.closing?.promise??Promise.resolve();},async run(request){
    this.requests.push(request);
    if(this.reply)await this.reply.promise;
    return {status:'success',result:request.kind==='sync'?{local:'unchanged',sharing:'local-only',domain:null}:{catalog_source:session,host_id:'host',stream:null,session:null,title_defaults:{stream:'automatic',session:'automatic'},patch_schemas:{}}};
   }};
   children.push(child);return child;
  }});
 return {ctx,handlers,children,contextState,notices,select:(next,path=null)=>{id=next;file=path;},
  start:()=>handlers.get('session_start')({},ctx),stop:()=>handlers.get('session_shutdown')({},ctx),
  read:()=>tools.get('stream_read').execute('read',{},undefined,undefined,ctx)};
}
test('first request initializes state; same-ID persistence retains its generation/helper',async()=>{
 const f=fixture();assert.equal(f.children.length,0);
 assert.equal((await f.read()).details.catalog_source,'first');
 const first=f.contextState.current;assert.equal(first.conversation.native_id,'first');assert.equal(first.session_file,null);
 f.select('first','/saved/first.jsonl');await f.read();
 assert.equal(f.contextState.current,first);assert.equal(first.session_file,'/saved/first.jsonl');assert.equal(f.children.length,1);
 await f.stop();assert.equal(f.children[0].stops,1);
});
for(const operation of ['new','fork','resume','startup-resume'])test(`${operation} adopts the observed ID and waits for the old helper to close`,async()=>{
 const f=fixture();await f.start();const first=f.contextState.current,child=f.children[0];child.closing=deferred();
 f.select(operation);const reading=f.read();await turn();
 assert.notEqual(f.contextState.current.generation,first.generation);assert.equal(first.retired,true);
 assert.equal(child.stops,1);assert.equal(f.children.length,1,'replacement must not start before old close');
 child.closing.resolve();assert.equal((await reading).details.catalog_source,operation);assert.equal(f.children.length,2);
 await f.stop();
});
test('overlapping starts discard the intermediate state instead of creating an orphan helper',async()=>{
 const f=fixture();await f.start();f.children[0].closing=deferred();
 f.select('middle');const middle=f.start();const abandoned=f.contextState.current;
 f.select('last');const last=f.start();await turn();
 assert.equal(abandoned.retired,true);assert.equal(f.children.length,1);
 f.children[0].closing.resolve();await Promise.all([middle,last]);
 assert.deepEqual(f.children.map(child=>child.session),['first','last']);
 assert.equal((await f.read()).details.catalog_source,'last');await f.stop();
});
test('switching away and back creates a distinct generation and rejects an obsolete result',async()=>{
 const f=fixture();await f.start();const original=f.contextState.current;
 f.children[0].reply=deferred();const reading=f.read(),rejected=assert.rejects(reading,/Session changed/);await turn();
 f.select('other');await f.start();f.select('first');await f.start();
 assert.ok(f.contextState.current.generation>original.generation);
 f.children[0].reply.resolve();await rejected;assert.equal((await f.read()).details.catalog_source,'first');await f.stop();
});
test('reload with a shared process holder fences old extension callbacks and shutdown',async()=>{
 const holder={generation:0},old=fixture(holder);await old.start();const previous=holder.current;
 const next=fixture(holder);await next.start();assert.ok(holder.current.generation>previous.generation);
 assert.equal(old.children[0].stops,1);
 await assert.rejects(old.read(),/retired|Session changed/);await old.stop();
 assert.equal(next.children[0].stops,0);assert.equal((await next.read()).details.catalog_source,'first');await next.stop();
});
test('independently injected registrations do not share state implicitly',async()=>{
 const first=fixture(null),second=fixture(null);await first.start();await second.start();
 await first.stop();assert.equal(second.children[0].stops,0);await second.stop();
});
test('retired same-ID callbacks cannot resurrect a stopped state; a new ID can initialize lazily',async()=>{
 const f=fixture();await f.start();await f.stop();
 await assert.rejects(f.read(),/retired/);assert.equal(f.children.length,1);
 f.select('resumed');assert.equal((await f.read()).details.catalog_source,'resumed');await f.stop();
});
test('shutdown retires its captured helper in finally when catalog sync fails',async()=>{
 const f=fixture();await f.start();f.children[0].run=async()=>{throw Error('sync failed');};
 await f.stop();assert.equal(f.children[0].stops,1);assert.ok(f.notices.some(text=>text.includes('sync failed')));
});
function processFixture() {
 const children=[],warnings=[];
 const manager=new StreamsChild({sessionId:'first',cwd:'/tmp',env:{STREAMS_BIN_PATH:process.execPath},warning:text=>warnings.push(text),spawn:()=>{
  const child=new EventEmitter();child.stdout=new PassThrough();child.stderr=new PassThrough();
  child.stdin=new Writable({write(_chunk,_encoding,done){done();}});child.kill=()=>true;children.push(child);return child;
 }});
 return {manager,children,warnings};
}
test('a failed helper remains retained until actual close, before replacement',async()=>{
 const f=processFixture(),started=f.manager.start(),ready=JSON.stringify({type:'ready',protocol:2,source:checkoutSource()});
 f.children[0].stdout.write(ready+'\nnot json\n');await started;await turn();
 assert.equal(f.children.length,1,'kill is not close');
 f.children[0].emit('close',1);await turn();assert.equal(f.children.length,2);
 f.children[1].stdout.write(ready+'\n');await f.manager.start();
 const stopped=f.manager.stop();f.children[1].emit('close',0);await stopped;
});
test('stop waits for a dying helper and suppresses its scheduled replacement',async()=>{
 const f=processFixture(),started=f.manager.start();
 f.children[0].stdout.write(JSON.stringify({type:'ready',protocol:2,source:checkoutSource()})+'\nnot json\n');await started;await turn();
 let done=false;const stopped=f.manager.stop().then(()=>{done=true;});await turn();
 assert.equal(done,false);assert.equal(f.children.length,1);
 f.children[0].emit('close',1);await stopped;await turn();assert.equal(f.children.length,1);
});
