import assert from 'node:assert/strict';
import test from 'node:test';
import { spawn, spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, realpathSync, renameSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Worker } from 'node:worker_threads';
import { createJiti } from 'jiti';
const jiti=createJiti(import.meta.url);
const {loadNative, NativeController}=await jiti.import('../extensions/stream-checkpoint/native.ts');
const {StreamsChild}=await jiti.import('../extensions/stream-checkpoint/child.ts');
const {default:register}=await jiti.import('../extensions/stream-checkpoint/index.ts');
const {defaults}=await jiti.import('../extensions/stream-checkpoint/settings.ts');
const root=fileURLToPath(new URL('../',import.meta.url));
const binary=join(resolve(root,process.env.CARGO_TARGET_DIR ?? 'target'),'debug','streams');
const plugin=join(root,'integrations/herdr-work-cards');
function temporary(t, dispose=async()=>{}, base=realpathSync(tmpdir())) {
 const directory=mkdtempSync(join(base,'streams-rust-native-'));
 t.after(async()=>{await dispose();rmSync(directory,{recursive:true,force:true});});return directory;
}
function lifetime(t, directory, extra={}) {
 const saved={...process.env};
 for (const key of Object.keys(process.env)) if (/^(HERDR_|CMUX_|STREAMS_|PI_CODING_AGENT_|WORK_CARDS_)/.test(key)) delete process.env[key];
 Object.assign(process.env,{HOME:directory,XDG_CONFIG_HOME:join(directory,'config'),XDG_DATA_HOME:join(directory,'data'),XDG_STATE_HOME:join(directory,'state'),PI_CODING_AGENT_DIR:join(directory,'pi'),HERDR_ENV:'1',HERDR_PANE_ID:'pane',HERDR_TAB_ID:'tab',HERDR_WORKSPACE_ID:'workspace',HERDR_SOCKET_PATH:join(directory,'not-live.sock'),...extra});
 t.after(()=>{for(const key of Object.keys(process.env)) delete process.env[key];Object.assign(process.env,saved);});
}
function piContext(directory,id) {
 const entries=[];let name;
 const ctx={hasUI:true,mode:'tui',cwd:directory,sessionManager:{getSessionId:()=>id,getEntries:()=>entries}};
 const pi={getSessionName:()=>name,setSessionName:value=>{name=value;entries.push({id:randomUUID(),type:'session_info',name:value});},appendEntry:(customType,data)=>entries.push({type:'custom',customType,data})};
 return {pi,ctx,entries,name:()=>name};
}
function fake(t) {
 const adapters=[],children=[],notices=[];
 const directory=temporary(t,async()=>{await Promise.all(adapters.map(adapter=>adapter.dispose()));await Promise.all(children.map(child=>child.stop()));}),id=randomUUID(),f=piContext(directory,id);
 const file=join(directory,'view.json'),log=join(directory,'commands.jsonl'),executable=join(directory,'rust child');
 const state={title:'Saved title',automatic:false,sessionNaming:true};
 const put=()=>{const staged=file+'.new';writeFileSync(staged,JSON.stringify(state));renameSync(staged,file);};put();
 writeFileSync(executable,`#!${process.execPath}\nimport(${JSON.stringify(new URL('./helpers/native-child.mjs',import.meta.url).href)});\n`,{mode:0o700});
 lifetime(t,directory,{STREAMS_BIN_PATH:executable,STREAMS_NATIVE_FIXTURE:file,STREAMS_NATIVE_LOG:log});
 const commands=()=>existsSync(log)?readFileSync(log,'utf8').trim().split('\n').filter(Boolean).map(JSON.parse):[];
 const load=async(options={})=>{const adapter=await loadNative({pi:f.pi,ctx:f.ctx,sessionId:id,cwd:directory,env:{...process.env},assertCurrent:()=>{},warning:text=>notices.push(text),...options});adapters.push(adapter);return adapter;};
 const child=()=>{const value=new StreamsChild({sessionId:id,cwd:directory,env:{...process.env},warning:text=>notices.push(text)});children.push(value);return value;};
 return {...f,directory,id,executable,state,put,commands,load,child,notices};
}
async function until(check) {const deadline=Date.now()+5000;while(!check()&&Date.now()<deadline)await new Promise(resolve=>setTimeout(resolve,5));assert.ok(check(),'child did not reach requested state');}

test('Rust routing stays lazy and sends only typed lifecycle and explicit user intent',async t=>{
 const f=fake(t),adapter=await f.load();assert.deepEqual(f.commands(),[]);
 await adapter.start(new AbortController().signal);await until(()=>f.name()==='Saved title');
 assert.deepEqual(f.commands(),[{kind:'session_started'}]);await adapter.open(new AbortController().signal);await adapter.close(new AbortController().signal);
 assert.deepEqual(f.commands(),[{kind:'session_started'},{kind:'open_card'},{kind:'close_card'}]);assert.deepEqual(f.notices,[]);
});
test('catalog UI facts update names without scope adoption or another native request',async t=>{
 const f=fake(t),child=f.child(),adapter=await f.load({child});await adapter.start(new AbortController().signal);await until(()=>f.name()==='Saved title');
 f.state.title='Updated catalog title';f.put();await child.run({kind:'context'});await until(()=>f.name()==='Updated catalog title');
 assert.deepEqual(f.commands(),[{kind:'session_started'},{kind:'context'}]);
 f.state.sessionNaming=false;f.state.title='Disabled';f.put();await child.run({kind:'context'});assert.equal(f.name(),'Updated catalog title');
});
for(const [operation,kind] of [['start','session_started'],['open','open_card'],['close','close_card']])test(`Rust ${operation} checks typed acknowledgements and preserves domain errors`,async t=>{
 const f=fake(t),adapter=await f.load(),signal=new AbortController().signal;
 f.state.invalid=kind;f.put();await assert.rejects(adapter[operation](signal),/acknowledgement/);
 delete f.state.invalid;f.state.fail=kind;f.put();await assert.rejects(adapter[operation](signal),error=>error.kind==='runtime'&&/uncertain native effect/.test(error.message));
 assert.equal(f.commands().length,2);
});
test('cancellation sends a request cancel and shutdown closes stdin without an explicit native close',async t=>{
 const f=fake(t),child=f.child(),adapter=await f.load({child}),controller=new AbortController();f.state.wait='open_card';f.put();
 const opening=adapter.open(controller.signal);await until(()=>f.commands().some(request=>request.kind==='open_card'));
 controller.abort(Error('cancelled effect'));await assert.rejects(opening,/cancelled effect/);await adapter.dispose();await child.stop();
 assert.deepEqual(f.commands(),[{kind:'open_card'}]);assert.deepEqual(f.notices,[]);
});
test('child restart attaches and never repeats session_started or an explicit effect',async t=>{
 const f=fake(t),child=f.child();await child.run({kind:'session_started'});
 const before=f.commands().length;child.process.kill('SIGKILL');await until(()=>f.commands().length>before);
 assert.deepEqual(f.commands().map(request=>request.kind),['session_started','attach']);assert.equal(f.notices.length,1);
 await child.run({kind:'context'});assert.deepEqual(f.commands().map(request=>request.kind),['session_started','attach','context']);
});
test('a retiring child cannot send background warnings into the replacement Pi lifetime',async t=>{
 const f=fake(t),handlers=new Map(),notices=[];f.ctx.ui={notify:message=>notices.push(message),setStatus:()=>{}};
 register({...f.pi,on:(name,handler)=>handlers.set(name,handler),registerCommand:()=>{},registerTool:()=>{}},{settings:{...defaults,reporting:'manual',enrollment:'manual'}});
 await handlers.get('session_start')({},f.ctx);assert.deepEqual(notices,[]);f.state.shutdownWarning='Retired background warning';f.put();
 await handlers.get('session_shutdown')({},f.ctx);assert.deepEqual(notices,[]);
});
test('native defaults to requested Cargo debug binary without explicit selectors',async t=>{
 const f=fake(t),target=join(f.directory,'cargo-target');mkdirSync(join(target,'debug'),{recursive:true});symlinkSync(f.executable,join(target,'debug','streams'));
 delete process.env.STREAMS_BIN_PATH;process.env.CARGO_TARGET_DIR=target;const adapter=await f.load();assert.deepEqual(f.commands(),[]);
 await adapter.start(new AbortController().signal);assert.deepEqual(f.commands(),[{kind:'session_started'}]);
});
test('invalid binary selectors never fall back to another controller',async t=>{
 const f=fake(t);
 for(const value of ['', 'relative',join(f.directory,'absent'),f.directory]) {
  const adapter=await f.load({env:{...process.env,STREAMS_BIN_PATH:value}});
  await assert.rejects(adapter.start(new AbortController().signal),/STREAMS_BIN_PATH/);
 }
 assert.deepEqual(f.commands(),[]);
});
test('native canonicalizes an explicit binary symlink',async t=>{
 const f=fake(t),alias=join(f.directory,'alias');symlinkSync(f.executable,alias);process.env.STREAMS_BIN_PATH=alias;
 await (await f.load()).start(new AbortController().signal);
 assert.deepEqual(f.commands(),[{kind:'session_started'}]);
});
test('Rust launch manifest and packaged launcher require the binary and plain launch inputs',t=>{
 const directory=temporary(t),script=join(plugin,'bin/open-rust-streams-pane.sh'),log=join(directory,'launch.json');
 const executable=join(directory,'explicit rust');
 writeFileSync(executable,`#!${process.execPath}\nrequire('node:fs').writeFileSync(${JSON.stringify(log)},JSON.stringify({args:process.argv.slice(2),context:process.env.STREAMS_CARDS_CONTEXT}));\n`,{mode:0o700});
 const env={PATH:process.env.PATH,STREAMS_BIN_PATH:executable,STREAMS_CARDS_CONTEXT:'plain launch inputs'};
 const run=environment=>spawnSync('/bin/sh',[script],{env:environment,encoding:'utf8',timeout:5000});
 const syntax=spawnSync('/bin/sh',['-n',script]);assert.equal(syntax.status,0);
 for(const key of ['STREAMS_BIN_PATH','STREAMS_CARDS_CONTEXT']) {
  const invalid={...env};delete invalid[key];assert.notEqual(run(invalid).status,0);assert.equal(existsSync(log),false);
 }
 assert.equal(run({...env,STREAMS_BIN_PATH:'relative'}).status,1);
 const result=run(env);assert.equal(result.status,0,result.stderr);assert.deepEqual(JSON.parse(readFileSync(log,'utf8')),{args:['native','pane'],context:'plain launch inputs'});
 const manifest=readFileSync(join(plugin,'streams-rust-herdr-plugin.toml'),'utf8');
 assert.match(manifest,/id = "streams-rust-work-cards"/);assert.match(manifest,/command = \["\/bin\/sh", "bin\/open-rust-streams-pane.sh"\]/);
 for(const file of ['package.json','integrations/herdr-work-cards/package.json']) {
  const files=JSON.parse(readFileSync(join(root,file),'utf8')).files;
  assert.ok(files.some(path=>path.endsWith('streams-rust-herdr-plugin.toml')));
  for(const shared of ['src/pi/session-naming.ts','src/streams-context.ts']) assert.ok(files.includes(file==='package.json'?'integrations/herdr-work-cards/'+shared:shared));
 }
});

// Protocol 22: one JSON request line per connection, one reply, then close.
// A worker keeps serving while the main thread blocks in spawnSync.
const herdrServerSource=`const {workerData}=require('node:worker_threads'),fs=require('node:fs'),net=require('node:net');
const {socket,file,log,ready}=workerData,flag=new Int32Array(ready);
function handle(method,params){
 const value=JSON.parse(fs.readFileSync(file,'utf8')),save=()=>{fs.writeFileSync(file+'.server',JSON.stringify(value));fs.renameSync(file+'.server',file);};
 fs.appendFileSync(log,JSON.stringify([method,params])+'\\n');
 if(method==='ping') return {type:'pong',version:'0.9.5',protocol:22};
 if(method==='events.subscribe') return {type:'subscription_started'};
 if(method==='pane.current') return {type:'pane_current',pane:value.pane};
 if(method==='workspace.get') return {type:'workspace_info',workspace:value.snapshot.workspaces[0]};
 if(method==='session.snapshot') {
  const result={type:'session_snapshot',snapshot:structuredClone(value.snapshot)};
  return result;
 }
 if(method==='workspace.report_metadata'||method==='pane.report_metadata') {
  const target=method==='workspace.report_metadata'?value.snapshot.workspaces.find(row=>row.workspace_id===params.workspace_id):value.snapshot.panes.find(row=>row.pane_id===params.pane_id);
  for(const [key,token] of Object.entries(params.tokens)) {if(token===null) delete target.tokens[key]; else target.tokens[key]=token;}
  save();return {type:'ok'};
 }
}
net.createServer(connection=>{let buffer='';connection.on('error',error=>{if(!['EPIPE','ECONNRESET'].includes(error.code)) throw error;});connection.on('data',chunk=>{buffer+=chunk;const end=buffer.indexOf('\\n');if(end<0)return;
 const request=JSON.parse(buffer.slice(0,end)),result=handle(request.method,request.params);
 const reply=JSON.stringify(result?{id:request.id,result}:{id:request.id,error:{code:'forbidden',message:'FORBIDDEN '+request.method}})+'\\n';
 if(request.method==='events.subscribe') connection.write(reply);else connection.end(reply);});})
 .listen(socket,()=>{Atomics.store(flag,0,1);Atomics.notify(flag,0);});`;
function herdrServer(t,data) {
 const ready=new SharedArrayBuffer(4),worker=new Worker(herdrServerSource,{eval:true,workerData:{...data,ready}});
 t.after(()=>worker.terminate());
 assert.equal(Atomics.wait(new Int32Array(ready),0,0,5000),'ok','fake Herdr did not listen');
}
function real(t,{enrolled=true}={}) {
 assert.ok(existsSync(binary),`Build the requested Rust CLI first: ${binary}`);
 const adapters=[],children=[];
 // Keep the named-session socket below the Unix socket path-length limit.
 const directory=temporary(t,async()=>{await Promise.all(adapters.map(adapter=>adapter.dispose()));await Promise.all(children.map(child=>child.stop()));},'/tmp'),id=randomUUID(),socket=join(directory,'config/herdr/sessions/fixture/herdr.sock'),fixture=join(directory,'native.json'),log=join(directory,'native-commands.jsonl'),executable=join(directory,'herdr'),spawned=join(directory,'herdr-spawned.jsonl'),f=piContext(directory,id);
 mkdirSync(dirname(socket),{recursive:true});
 lifetime(t,directory,{HERDR_BIN_PATH:executable,HERDR_SESSION:'fixture',HERDR_SOCKET_PATH:socket,STREAMS_BIN_PATH:binary});
 const load=async input=>{const adapter=await loadNative(input);adapters.push(adapter);return adapter;};
 const child=(options={})=>{const value=new StreamsChild({sessionId:id,cwd:directory,env:{...process.env},...options});children.push(value);return value;};
 const pane={pane_id:'pane',terminal_id:'terminal',workspace_id:'workspace',tab_id:'tab',cwd:directory,agent:'pi',agent_session:{agent:'pi',kind:'id',value:id},tokens:{}};
 const native={pane,socket,snapshot:{workspaces:[{workspace_id:'workspace',label:'Fixture',number:1,tokens:{}}],tabs:[{workspace_id:'workspace',tab_id:'tab',label:'1',number:1}],panes:[pane],layouts:[{workspace_id:'workspace',tab_id:'tab',panes:[{pane_id:'pane'}]}],agents:[pane]}};
 const put=()=>{writeFileSync(fixture+'.parent',JSON.stringify(native));renameSync(fixture+'.parent',fixture);};put();
 // The CLI must never start Herdr; the tripwire records any attempt.
 writeFileSync(executable,`#!${process.execPath}\nrequire('node:fs').appendFileSync(${JSON.stringify(spawned)},JSON.stringify(process.argv.slice(2))+'\\n');process.exit(91);\n`,{mode:0o700});
 herdrServer(t,{socket,file:fixture,log});
 const invoke=(...args)=>{
  const result=spawnSync(binary,[...args,'--json'],{env:process.env,cwd:directory,encoding:'utf8',timeout:120000});assert.ifError(result.error);
  let data;try{data=JSON.parse(result.stdout);}catch{assert.fail(result.stdout+result.stderr);}return {code:result.status,data};
 };
 const cli=(...args)=>{const result=invoke(...args);assert.equal(result.code,0,JSON.stringify(result.data));return result.data;};
 cli('init','--sync','none');cli('config','set','frontend.mode','none');
 const stream=cli('create','Effort').stream;
 const session=enrolled?cli('session','attach',stream.id,'--harness','pi','--session-id',id,'--title','Real Rust lane').session:null;
 cli('config','set','frontend.mode','herdr');
 const config=join(directory,'cards.json');writeFileSync(config,JSON.stringify({workspace_startup:'manual',session_naming:true,workspace_naming:false,tab_naming:false}));process.env.WORK_CARDS_CONFIG_PATH=config;
 const catalog=cli('config','get','catalog.root'),hash=value=>createHash('sha256').update(value).digest('hex'),key=(...parts)=>hash('['+parts.map(JSON.stringify).join(', ')+']');
 const host=session?.host_id??JSON.parse(readFileSync(join(directory,'config/streams/host.json'),'utf8')).id;
 const state=join(directory,'state/streams/catalogs',hash(realpathSync(catalog)),key(host));
 const context=cli('agent','context','--harness','pi','--session-id',id);
 native.snapshot.workspaces[0].tokens={strm_catalog:hash(JSON.stringify(context.catalog_source)),strm_host:host,strm_stream:stream.id};put();
 const transcript=join(directory,'pi/sessions',id+'.jsonl');mkdirSync(dirname(transcript),{recursive:true});
 writeFileSync(transcript,JSON.stringify({type:'session',version:3,id,cwd:directory,timestamp:new Date(0).toISOString()})+'\n');
 const commands=()=>{assert.equal(existsSync(spawned),false,'the Rust CLI started a Herdr process');return existsSync(log)?readFileSync(log,'utf8').trim().split('\n').filter(Boolean).map(JSON.parse):[];};
 return {...f,directory,id,stream,state,native,put,invoke,cli,load,child,commands};
}
test('one-shot native operations subscribe before their single snapshot without a scope round trip',t=>{
 const f=real(t);writeFileSync(join(f.directory,'native-commands.jsonl'),'');
 for(const operation of ['observe','check','project']) {
  const start=f.commands().length;f.cli('native','--session-id',f.id,operation);const counts={};
  for(const [method] of f.commands().slice(start))counts[method]=(counts[method]??0)+1;
  assert.equal(counts.ping,1,operation);assert.equal(counts['events.subscribe'],1,operation);assert.equal(counts['session.snapshot'],1,operation);
  const methods=f.commands().slice(start).map(([method])=>method);assert.ok(methods.indexOf('events.subscribe')<methods.indexOf('session.snapshot'));
 }
});
test('Pi reuses one real v2 child; catalog UI facts never trigger a TypeScript native control loop',async t=>{
 const f=real(t),notices=[],requests=[];let spawns=0;
 const child=f.child({warning:text=>notices.push(text),spawn:(...args)=>{spawns++;return spawn(...args);}});
 const run=child.run;child.run=(request,...rest)=>{requests.push(structuredClone(request));return run(request,...rest);};
 const controller=new NativeController({pi:f.pi,ctx:f.ctx,child,factory:f.load,warning:text=>notices.push(text)});
 t.after(async()=>{controller.stop();await controller.idle();});
 await controller.start();await until(()=>f.name()==='Real Rust lane');assert.deepEqual(requests,[{kind:'session_started'}]);
 const outcome=await child.run({kind:'context'});assert.equal(outcome.status,'success');const catalog=outcome.result;
 const intent={version:2,operation_id:'child-title',catalog_source:catalog.catalog_source,actor:'user',kind:'session',before:catalog.session,changes:{title:'Child title'},title_default:catalog.title_defaults.session,inputs:[]};
 const saved=await child.run({kind:'apply',request:intent});assert.notEqual(saved.status,'error',JSON.stringify(saved));
 await until(()=>f.name()==='Child title');assert.deepEqual(requests.map(request=>request.kind),['session_started','context','apply']);assert.equal(spawns,1);
 assert.ok(f.commands().some(([method])=>method.endsWith('report_metadata')));assert.deepEqual(notices,[]);
});
test('real v2 attach does not need a prior Pi scope and has no automatic-open outcome',async t=>{
 const f=real(t),child=f.child({warning:assert.fail});
 const before=f.commands().length;await child.start();assert.equal(f.commands().length,before,'ready alone must not observe or project');
 const attached=await child.run({kind:'attach'});assert.equal(attached.status,'success');assert.deepEqual(attached.result,{native:'active'});
});
test('real Rust owner observation is read-only and wrong session/caller refuses before metadata effects',t=>{
 const f=real(t),before=readFileSync(join(f.directory,'native.json'),'utf8');
 f.cli('native','--session-id',f.id,'observe');
 assert.equal(readFileSync(join(f.directory,'native.json'),'utf8'),before);assert.equal(existsSync(join(f.state,'native-cards')),false);
 const wrongSession=f.invoke('native','--session-id',randomUUID(),'project');assert.notEqual(wrongSession.code,0);assert.match(wrongSession.data.error.message,/session|conversation|caller|origin/i);
 f.native.pane.agent_session={agent:'pi',kind:'id',value:randomUUID()};f.put();
 const wrongCaller=f.invoke('native','--session-id',f.id,'project');assert.notEqual(wrongCaller.code,0);assert.match(wrongCaller.data.error.message,/session|conversation|caller|origin/i);
 assert.ok(f.commands().every(([method])=>!method.endsWith('report_metadata')));
});
test('launched-card readers reject a socket or namespace that contradicts the pinned endpoint',t=>{
 const f=real(t),observed=f.cli('native','--session-id',f.id,'observe');
 const keys=['HOME','XDG_CONFIG_HOME','XDG_DATA_HOME','XDG_STATE_HOME','PI_CODING_AGENT_DIR','PATH','HERDR_SOCKET_PATH','HERDR_SESSION'];
 const reader=Object.fromEntries(keys.filter(key=>process.env[key]!==undefined).map(key=>[key,process.env[key]]));
 const card={...f.native.pane,pane_id:'card',terminal_id:'card-terminal',agent:null,agent_session:null};
 f.native.snapshot.panes.push(card);f.native.snapshot.layouts[0].panes.push({pane_id:'card'});f.put();
 process.env.HERDR_PANE_ID='card';
 for(const change of [{HERDR_SOCKET_PATH:join(f.directory,'different.sock')},{HERDR_SESSION:'different'}]) {
  process.env.STREAMS_CARDS_CONTEXT=JSON.stringify({scope:observed.scope,owner:observed.owner,
   reader:{environment:{...reader,...change},cwd:f.directory},lock_directory:join(f.directory,'card-locks'),plugin_root:plugin});
  const result=f.invoke('native','--session-id',f.id,'check');
  assert.notEqual(result.code,0);assert.match(result.data.error.message,/reader endpoint changed/);
 }
 assert.ok(f.commands().every(([method])=>['ping','events.subscribe','session.snapshot'].includes(method)));
});
test('retired layout capture refuses without inspecting or mutating native resources',t=>{
 const f=real(t),before=readFileSync(join(f.directory,'native.json'),'utf8');
 const refused=f.invoke('native','--session-id',f.id,'capture');
 assert.notEqual(refused.code,0);assert.match(refused.data.error.message,/unrecognized subcommand 'capture'/);
 assert.equal(readFileSync(join(f.directory,'native.json'),'utf8'),before);assert.deepEqual(f.commands(),[]);
 for(const name of ['arrangements','associations','placements']) assert.equal(existsSync(join(f.state,name)),false);
});
test('Pi lifecycle preserves its named Herdr endpoint without saved native resources',async t=>{
 const f=real(t);
 assert.equal(process.env.HERDR_SESSION,'fixture');
 const notices=[],controller=new NativeController({pi:f.pi,ctx:f.ctx,factory:f.load,warning:text=>notices.push(text)});t.after(async()=>{controller.stop();await controller.idle();});
 controller.start();await controller.idle();assert.deepEqual(notices,[]);
 assert.equal(f.name(),'Real Rust lane');assert.ok(f.commands().some(([method])=>method.endsWith('report_metadata')));
 const allowed=['ping','events.subscribe','pane.current','workspace.get','session.snapshot','workspace.report_metadata','pane.report_metadata'];
 assert.ok(f.commands().every(([method])=>allowed.includes(method)));
 const snapshot=JSON.parse(readFileSync(join(f.directory,'native.json'),'utf8')).snapshot;
 assert.equal(snapshot.workspaces[0].tokens.strm_stream,f.stream.id);
 assert.equal(snapshot.workspaces[0].tokens.strm_binding,undefined);
 for(const name of ['arrangements','associations','placements']) assert.equal(existsSync(join(f.state,name)),false);
 controller.stop();await controller.idle();delete process.env.HERDR_SOCKET_PATH;
 const observed=f.cli('native','--session-id',f.id,'observe');
 // Without an explicit socket, the named session resolves to the same endpoint.
 assert.equal(observed.owner.terminal_id,'terminal');assert.equal(observed.scope.streamId,f.stream.id);
 const file=join(f.directory,'native.json'),live=JSON.parse(readFileSync(file,'utf8')),wrongSession=randomUUID();
 for(const row of [live.pane,...live.snapshot.panes,...live.snapshot.agents])row.agent_session={agent:'pi',kind:'id',value:wrongSession};
 writeFileSync(file,JSON.stringify(live));
 const before=readFileSync(file,'utf8');
 const refused=f.invoke('native','--session-id',f.id,'project');
 assert.notEqual(refused.code,0);assert.match(refused.data.error.message,/session|conversation|caller|origin/i);
 assert.equal(readFileSync(file,'utf8'),before);
 f.commands(); // Assert zero Herdr CLI spawns throughout the lifecycle.
});
