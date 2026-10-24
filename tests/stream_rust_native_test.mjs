import assert from 'node:assert/strict';
import test from 'node:test';
import { spawn, spawnSync } from 'node:child_process';
import { once } from 'node:events';
import { createInterface } from 'node:readline';
import { createHash, randomUUID } from 'node:crypto';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Worker } from 'node:worker_threads';
import { createJiti } from 'jiti';
const jiti=createJiti(import.meta.url);
const {loadNative, NativeController}=await jiti.import('../extensions/stream-checkpoint/native.ts');
const {StreamsChild}=await jiti.import('../extensions/stream-checkpoint/child.ts');
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
 const adapters=[],children=[];
 const directory=temporary(t,async()=>{await Promise.all(adapters.map(adapter=>adapter.dispose()));await Promise.all(children.map(child=>child.stop()));}),id=randomUUID(),f=piContext(directory,id);
 const file=join(directory,'observation.json'),log=join(directory,'commands.jsonl'),executable=join(directory,'rust child');
 const observation={version:1,scope:{catalogSource:'catalog',hostId:'host',streamId:'stream',sessionId:'session',nativeSessionId:id},native:{pane_id:'pane',tab_id:'tab',workspace_id:'workspace'},context:{socket:join(directory,'not-live.sock'),paneId:'pane',terminalId:'terminal',tabId:'tab',workspaceId:'workspace',binding:null},session:{title:'Saved title',title_mode:'automatic'},config:{automatic:false,sessionNaming:true}};
 const put=value=>writeFileSync(file,JSON.stringify(value));put(observation);
 writeFileSync(executable,`#!${process.execPath}\nimport(${JSON.stringify(new URL('./helpers/native-child.mjs',import.meta.url).href)});\n`,{mode:0o700});
 lifetime(t,directory,{STREAMS_BIN_PATH:executable,STREAMS_NATIVE_FIXTURE:file,STREAMS_NATIVE_LOG:log});
 const commands=()=>existsSync(log)?readFileSync(log,'utf8').trim().split('\n').filter(Boolean).map(JSON.parse):[];
 const load=async(options={})=>{const adapter=await loadNative({pi:f.pi,ctx:f.ctx,sessionId:id,cwd:directory,env:{...process.env},...options});adapters.push(adapter);return adapter;};
 const child=()=>{const value=new StreamsChild({sessionId:id,cwd:directory,env:{...process.env},warning:assert.fail});children.push(value);return value;};
 return {...f,directory,id,executable,observation,put,commands,load,child};
}
function command(args) {return args.find(arg=>['observe','open','close','project','capture'].includes(arg));}
async function observe(child, id) {
 const revision=child.revision;
 let unsubscribe;
 const changed=new Promise((resolve,reject)=>{
  const timer=setTimeout(()=>{unsubscribe();reject(Error('child did not emit a new scope'));},5000);
  unsubscribe=child.subscribe(()=>{if(child.revision>revision){clearTimeout(timer);unsubscribe();resolve();}});
 });
 const result=await child.run(['native','observe','--session-id',id,'--json']);
 assert.equal(result.code,0,result.stdout);await changed;
}
async function until(check) {const deadline=Date.now()+5000;while(!check()&&Date.now()<deadline) await new Promise(resolve=>setTimeout(resolve,5));assert.ok(check(),'child did not reach requested command');}

test('Rust routing stays lazy and routes the Pi lane without TypeScript native effects or Python capture',async t=>{
 const f=fake(t),notices=[];
 const adapter=await f.load();assert.deepEqual(f.commands(),[]);
 const controller=new NativeController({pi:f.pi,ctx:f.ctx,factory:async()=>adapter,capture:assert.fail,warning:text=>notices.push(text)});
 t.after(async()=>{controller.stop();await controller.idle();});controller.start();await controller.idle();
 assert.deepEqual(f.commands().map(command).filter(value=>value!=='observe'),['project','capture']);
 assert.equal(command(f.commands()[0]),'observe');
 assert.equal(f.name(),'Saved title');assert.deepEqual(notices,[]);
 assert.equal(adapter.automatic,false);assert.equal(adapter.sessionNaming,true);
 for(const args of f.commands().filter(args=>command(args)!=='observe')) {
  assert.deepEqual(JSON.parse(args[args.indexOf('--expect')+1]),{version:1,scope:f.observation.scope,context:f.observation.context});
  assert.equal(args[args.indexOf('--plugin-root')+1],plugin);assert.equal(args[args.indexOf('--session-id')+1],f.id);
 }
 await controller.open();await controller.close();
 assert.equal(f.commands().filter(args=>command(args)==='open').length,1);
 assert.equal(f.commands().map(command).filter(value=>value!=='observe').at(-1),'close');
 for(const [target,reclaim] of [['all','all'],['session','tab'],['stream','workspace']]) {
  await controller.reclaimNames(target);
  const args=f.commands().findLast(args=>command(args)==='project');assert.equal(args[args.indexOf('--reclaim')+1],reclaim);
 }
 assert.equal(f.commands().filter(args=>command(args)==='open').length,1);
});
test('Rust adapter caches scope and adopts moved endpoint, terminal and binding on child events',async t=>{
 const f=fake(t),child=f.child(),adapter=await f.load({child});await adapter.read();
 const context=await adapter.bootstrap(),before=f.commands().length;
 await adapter.read();assert.equal(f.commands().length,before);
 await assert.rejects(adapter.open({...context,terminalId:'reused'},new AbortController().signal,'explicit'),/context changed/);
 f.observation.context={...f.observation.context,socket:join(f.directory,'moved.sock'),terminalId:'replacement',binding:'replacement',tabId:'moved-tab',workspaceId:'moved-workspace'};
 f.observation.native={pane_id:'pane',tab_id:'moved-tab',workspace_id:'moved-workspace'};
 f.observation.scope={...f.observation.scope,streamId:'moved-stream',sessionId:'moved-session'};f.put(f.observation);
 assert.equal((await adapter.read()).scope.streamId,'stream');
 await observe(child,f.id);
 assert.deepEqual((await adapter.read()).scope,f.observation.scope);
 await assert.rejects(adapter.open(context,new AbortController().signal,'explicit'),/context changed/);
 const moved=await adapter.bootstrap();assert.deepEqual(moved,f.observation.context);
 await adapter.project(moved,new AbortController().signal);
 const args=f.commands().findLast(args=>command(args)==='project');
 assert.deepEqual(JSON.parse(args[args.indexOf('--expect')+1]),{version:1,scope:f.observation.scope,context:f.observation.context});
});
for(const operation of ['open','project','close']) test(`Rust ${operation} rejects malformed acknowledgements and preserves actionable failure`,async t=>{
 const f=fake(t),adapter=await f.load();await adapter.read();const context=await adapter.bootstrap(),signal=new AbortController().signal;
 const invoke=()=>adapter[operation](context,signal,'explicit');
 f.observation.invalid=operation;f.put(f.observation);await assert.rejects(invoke(),/acknowledgement/);
 delete f.observation.invalid;f.observation.fail=operation;f.put(f.observation);
 await assert.rejects(invoke(),/uncertain native effect; pending retained/);
 assert.equal(f.commands().filter(args=>command(args)===operation).length,2);
});
test('an effect scope mismatch waits for a new child scope and then recovers',async t=>{
 const f=fake(t),child=f.child(),adapter=await f.load({child});await adapter.read();const context=await adapter.bootstrap();
 f.observation.fail='project';f.observation.failureKind='native-context-changed';f.put(f.observation);
 await assert.rejects(adapter.project(context,new AbortController().signal),error=>error.kind==='native-context-changed');
 const before=f.commands().filter(args=>command(args)!=='observe').length;
 await assert.rejects(adapter.read(),/scope changed/);
 await assert.rejects(adapter.project(context,new AbortController().signal),/context changed/);
 assert.equal(f.commands().filter(args=>command(args)!=='observe').length,before);
 delete f.observation.fail;f.observation.context.binding='recovered-binding';f.put(f.observation);
 await observe(child,f.id);
 assert.deepEqual((await adapter.read()).scope,f.observation.scope);
 await adapter.project(await adapter.bootstrap(),new AbortController().signal);
 assert.equal(f.commands().filter(args=>command(args)==='project').length,2);
});
test('an acknowledged first binding is required by the very next effect subprocess',async t=>{
 const f=fake(t),adapter=await f.load();await adapter.read();const context=await adapter.bootstrap();
 f.observation.ackBinding='created-binding';f.put(f.observation);
 await adapter.open(context,new AbortController().signal,'explicit');
 f.observation.context.binding='created-binding';f.put(f.observation);
 await adapter.project(context,new AbortController().signal);
 const args=f.commands().findLast(args=>command(args)==='project');assert.equal(JSON.parse(args[args.indexOf('--expect')+1]).context.binding,'created-binding');
});
for(const message of ['native projection requires an enrolled Stream and Session','requested Pi session differs from the native pane session','native projection requires a ready workspace association for this Stream']) test(`unready Rust startup remains quiet: ${message}`,async t=>{
 const f=fake(t);f.observation.fail='observe';f.observation.failMessage=message;f.put(f.observation);
 const notices=[],controller=new NativeController({pi:f.pi,ctx:f.ctx,factory:()=>f.load(),warning:text=>notices.push(text)});t.after(async()=>{controller.stop();await controller.idle();});
 controller.start();await controller.idle();assert.deepEqual(notices,[]);await assert.rejects(controller.open(),error=>error.message.includes(message));
 assert.ok(f.commands().every(args=>command(args)==='observe'));
});
test('Rust native cancellation drains the subprocess before shutdown and never invokes close',async t=>{
 const f=fake(t);f.observation.config.automatic=true;f.observation.wait='open';f.put(f.observation);
 const notices=[],controller=new NativeController({pi:f.pi,ctx:f.ctx,factory:()=>f.load(),warning:text=>notices.push(text)});t.after(async()=>{controller.stop();await controller.idle();});
 controller.start();await until(()=>f.commands().some(args=>command(args)==='open'));
 controller.stop();await controller.idle();assert.deepEqual(notices,[]);
 assert.equal(f.commands().some(args=>command(args)==='close'||command(args)==='capture'),false);
});
test('native defaults to the requested Cargo debug Rust CLI without either explicit selector',async t=>{
 const f=fake(t),target=join(f.directory,'cargo-target');mkdirSync(join(target,'debug'),{recursive:true});
 symlinkSync(f.executable,join(target,'debug','streams'));
 delete process.env.STREAMS_BIN_PATH;process.env.CARGO_TARGET_DIR=target;
 const adapter=await f.load();assert.deepEqual(f.commands(),[]);
 await adapter.read();assert.deepEqual(f.commands().map(command),['observe']);
});
test('an explicit invalid binary never falls back to the Cargo binary or legacy coordinator',async t=>{
 const f=fake(t);
 for(const selector of ['STREAMS_BIN_PATH','STREAMS_RUST_BIN']) {
  for(const value of ['', 'relative',join(f.directory,'absent'),f.directory]) {
   const env={...process.env};delete env.STREAMS_BIN_PATH;delete env.STREAMS_RUST_BIN;env[selector]=value;
   const adapter=await f.load({env});await assert.rejects(adapter.read(),new RegExp(selector));
  }
 }
 assert.deepEqual(f.commands(),[]);
});
test('native rejects conflicting reporting and compatibility selectors before any subprocess',async t=>{
 const f=fake(t),other=join(f.directory,'other-cli');symlinkSync(process.execPath,other);
 process.env.STREAMS_RUST_BIN=other;
 const adapter=await f.load();await assert.rejects(adapter.read(),/STREAMS_BIN_PATH and STREAMS_RUST_BIN select different/);
 assert.deepEqual(f.commands(),[]);
});
test('native accepts the compatibility alias and canonically matching selectors',async t=>{
 const f=fake(t),alias=join(f.directory,'alias');symlinkSync(f.executable,alias);
 delete process.env.STREAMS_BIN_PATH;process.env.STREAMS_RUST_BIN=alias;
 await (await f.load()).read();process.env.STREAMS_BIN_PATH=f.executable;
 await (await f.load()).read();assert.deepEqual(f.commands().map(command),['observe','observe']);
});
test('Rust launch manifest and packaged launcher match the coordinator and require the protected envelope',t=>{
 const directory=temporary(t),script=join(plugin,'bin/open-rust-streams-pane.sh'),log=join(directory,'launch.json');
 const executable=join(directory,'explicit rust');
 writeFileSync(executable,`#!${process.execPath}\nrequire('node:fs').writeFileSync(${JSON.stringify(log)},JSON.stringify({args:process.argv.slice(2),scope:process.env.STREAMS_CARDS_SCOPE}));\n`,{mode:0o700});
 const fields=['SCOPE','OWNER','NONCE','OWNER_TERMINAL','READ_ENV','READ_CWD','STATE_DIR','PLUGIN_ROOT'];
 const env={PATH:process.env.PATH,STREAMS_RUST_BIN:executable,...Object.fromEntries(fields.map(field=>['STREAMS_CARDS_'+field,'protected']))};
 const run=environment=>spawnSync('/bin/sh',[script],{env:environment,encoding:'utf8',timeout:5000});
 const syntax=spawnSync('/bin/sh',['-n',script]);assert.equal(syntax.status,0);
 for(const key of ['STREAMS_RUST_BIN',...fields.map(field=>'STREAMS_CARDS_'+field)]) {
  const invalid={...env};delete invalid[key];assert.notEqual(run(invalid).status,0);assert.equal(existsSync(log),false);
 }
 assert.equal(run({...env,STREAMS_RUST_BIN:'relative'}).status,1);
 const result=run(env);assert.equal(result.status,0,result.stderr);assert.deepEqual(JSON.parse(readFileSync(log,'utf8')),{args:['native','pane'],scope:'protected'});
 const manifest=readFileSync(join(plugin,'streams-rust-herdr-plugin.toml'),'utf8');
 assert.match(manifest,/id = "streams-rust-work-cards"/);assert.match(manifest,/command = \["\/bin\/sh", "bin\/open-rust-streams-pane.sh"\]/);
 for(const file of ['package.json','integrations/herdr-work-cards/package.json']) {
  const files=JSON.parse(readFileSync(join(root,file),'utf8')).files;
  assert.ok(files.some(path=>path.endsWith('streams-rust-herdr-plugin.toml')));
  for(const shared of ['src/herdr/subprocess.ts','src/pi/session-naming.ts','src/streams-context.ts']) assert.ok(files.includes(file==='package.json'?'integrations/herdr-work-cards/'+shared:shared));
 }
});

// Protocol 22: one JSON request line per connection, one reply, then close.
// A worker keeps serving while the main thread blocks in spawnSync.
const herdrServerSource=`const {workerData}=require('node:worker_threads'),fs=require('node:fs'),net=require('node:net');
const {socket,file,log,ready}=workerData,flag=new Int32Array(ready);
function handle(method,params){
 const value=JSON.parse(fs.readFileSync(file,'utf8')),save=()=>fs.writeFileSync(file,JSON.stringify(value));
 fs.appendFileSync(log,JSON.stringify([method,params])+'\\n');
 if(method==='ping') return {type:'pong',version:'0.9.5',protocol:22};
 if(method==='events.subscribe') return {type:'subscription_started'};
 if(method==='pane.current') return {type:'pane_current',pane:value.pane};
 if(method==='workspace.get') return {type:'workspace_info',workspace:value.snapshot.workspaces[0]};
 if(method==='session.snapshot') {
  const result={type:'session_snapshot',snapshot:structuredClone(value.snapshot)};
  if(value.replaceOwnerAfterSnapshot&&--value.replaceOwnerAfterSnapshot===0){for(const row of [value.pane,value.snapshot.panes[0],value.snapshot.agents[0]]) row.terminal_id='reused-terminal';}
  if(value.replaceOwnerAfterSnapshot!==undefined) save();
  return result;
 }
 if(method==='workspace.report_metadata'||method==='pane.report_metadata') {
  const target=method==='workspace.report_metadata'?value.snapshot.workspaces.find(row=>row.workspace_id===params.workspace_id):value.snapshot.panes.find(row=>row.pane_id===params.pane_id);
  for(const [key,token] of Object.entries(params.tokens)) {if(token===null) delete target.tokens[key]; else target.tokens[key]=token;}
  save();return {type:'ok'};
 }
}
net.createServer(connection=>{let buffer='';connection.on('data',chunk=>{buffer+=chunk;const end=buffer.indexOf('\\n');if(end<0)return;
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
 const adapters=[];
 // Keep the named-session socket below the Unix socket path-length limit.
 const directory=temporary(t,()=>Promise.all(adapters.map(adapter=>adapter.dispose())),'/tmp'),id=randomUUID(),socket=join(directory,'config/herdr/sessions/fixture/herdr.sock'),fixture=join(directory,'native.json'),log=join(directory,'native-commands.jsonl'),executable=join(directory,'herdr'),spawned=join(directory,'herdr-spawned.jsonl'),f=piContext(directory,id);
 mkdirSync(dirname(socket),{recursive:true});
 lifetime(t,directory,{HERDR_BIN_PATH:executable,HERDR_SESSION:'fixture',HERDR_SOCKET_PATH:socket,STREAMS_BIN_PATH:binary});
 const load=async input=>{const adapter=await loadNative(input);adapters.push(adapter);return adapter;};
 const pane={pane_id:'pane',terminal_id:'terminal',workspace_id:'workspace',tab_id:'tab',cwd:directory,agent:'pi',agent_session:{agent:'pi',kind:'id',value:id},tokens:{}};
 const native={pane,socket,snapshot:{workspaces:[{workspace_id:'workspace',label:'Fixture',number:1,tokens:{}}],tabs:[{workspace_id:'workspace',tab_id:'tab',label:'1',number:1}],panes:[pane],layouts:[{workspace_id:'workspace',tab_id:'tab',panes:[{pane_id:'pane'}]}],agents:[pane]}};
 const put=()=>writeFileSync(fixture,JSON.stringify(native));put();
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
 const legacyState=join(dirname(socket),'streams-native');
 cli('native','migrate-evidence','--legacy-state-dir',legacyState);
 const catalog=cli('config','get','catalog.root'),hash=value=>createHash('sha256').update(value).digest('hex'),key=(...parts)=>hash('['+parts.map(JSON.stringify).join(', ')+']');
 const host=session?.host_id??JSON.parse(readFileSync(join(directory,'config/streams/host.json'),'utf8')).id;
 const state=join(directory,'state/streams/catalogs',hash(realpathSync(catalog)),key(host));
 const association=join(state,'associations',key('herdr',socket,'fixture'),key('workspace')+'.json');
 mkdirSync(dirname(association),{recursive:true,mode:0o700});writeFileSync(association,JSON.stringify({version:1,workspace_id:'workspace',stream_id:stream.id,session_id:null,command:null,ready:true}),{mode:0o600});
 const transcript=join(directory,'pi/sessions',id+'.jsonl');mkdirSync(dirname(transcript),{recursive:true});
 writeFileSync(transcript,JSON.stringify({type:'session',version:3,id,cwd:directory,timestamp:new Date(0).toISOString()})+'\n');
 const commands=()=>{assert.equal(existsSync(spawned),false,'the Rust CLI started a Herdr process');return existsSync(log)?readFileSync(log,'utf8').trim().split('\n').filter(Boolean).map(JSON.parse):[];};
 return {...f,directory,id,stream,state,legacyState,native,put,invoke,cli,load,commands};
}
for(const pinned of [false,true]) test(`native operation Herdr request budgets${pinned?' with --expect':''}`,t=>{
 const f=real(t);writeFileSync(join(f.directory,'native-commands.jsonl'),'');
 for(const operation of ['observe','check','project','capture']) {
  const observed=pinned?f.cli('native','--session-id',f.id,'observe'):null;
  const expect=observed?['--expect',JSON.stringify({version:1,scope:observed.scope,context:observed.context})]:[];
  const start=f.commands().length;
  f.cli('native','--session-id',f.id,...expect,operation);
  const counts={};
  for(const [method] of f.commands().slice(start)) counts[method]=(counts[method]??0)+1;
  // One protocol check per process, then one socket request per observation
  // or effect; no Herdr CLI process (commands() trips on any spawn).
  const expected={observe:{ping:1,'session.snapshot':1},check:{ping:1,'session.snapshot':1},
   project:{ping:1,'session.snapshot':7,'workspace.report_metadata':1,'pane.report_metadata':1},
   capture:{ping:1,'session.snapshot':4}}[operation];
  assert.deepEqual(counts,expected,operation);
 }
});
test('native evidence reset works without a catalog, host identity or caller Session',t=>{
 const directory=temporary(t,async()=>{},'/tmp'),socket=join(directory,'config/herdr/sessions/fixture/herdr.sock');
 const file=join(directory,'native.json'),log=join(directory,'native-commands.jsonl'),legacy=join(directory,'known-v1');
 mkdirSync(dirname(socket),{recursive:true});
 writeFileSync(file,JSON.stringify({snapshot:{workspaces:[],tabs:[],panes:[],layouts:[],agents:[]}}));
 lifetime(t,directory,{HERDR_SOCKET_PATH:socket,HERDR_ENV:'0',HERDR_PANE_ID:'not-a-caller'});
 herdrServer(t,{socket,file,log});
 const result=spawnSync(binary,['native','migrate-evidence','--legacy-state-dir',legacy,'--json'],{env:process.env,cwd:directory,encoding:'utf8',timeout:120000});
 assert.ifError(result.error);assert.equal(result.status,0,result.stdout+result.stderr);
 assert.equal(JSON.parse(result.stdout).evidence_reset.deleted_evidence,0);
 assert.deepEqual(JSON.parse(readFileSync(join(directory,'state/streams/native/format.json'),'utf8')),{version:2});
 assert.equal(existsSync(join(directory,'config/streams/host.json')),false);
 assert.equal(existsSync(join(directory,'state/streams/catalogs')),false);
 assert.equal(existsSync(join(directory,'data/streams')),false);
 assert.equal(existsSync(legacy),false);
 assert.deepEqual(readFileSync(log,'utf8').trim().split('\n').map(line=>JSON.parse(line)[0]),['ping','session.snapshot']);
});

test('native evidence reset is explicit and does not need an enrolled Session',t=>{
 const f=real(t,{enrolled:false}),root=join(f.directory,'state/streams/native'),marker=join(root,'format.json');
 rmSync(marker);mkdirSync(f.legacyState,{recursive:true,mode:0o700});
 const key='ab'.repeat(32),path=join(f.legacyState,key+'.json');
 writeFileSync(path,JSON.stringify({version:1,key,nonce:'uncertain',phase:'pending'}),{mode:0o600});
 const before=f.commands().length;
 const scoped=f.invoke('native','--session-id',f.id,'migrate-evidence','--legacy-state-dir',f.legacyState);
 assert.notEqual(scoped.code,0);assert.match(scoped.data.error.message,/not Session-scoped/);
 assert.equal(f.commands().length,before);
 const reset=f.cli('native','migrate-evidence','--legacy-state-dir',f.legacyState);
 assert.equal(reset.version,2);assert.equal(reset.evidence_reset.deleted_evidence,1);
 assert.deepEqual(reset.evidence_reset.closed_cards,[]);
 assert.equal(existsSync(path),false);assert.deepEqual(JSON.parse(readFileSync(marker,'utf8')),{version:2});
 assert.deepEqual(f.commands().slice(before).map(([method])=>method),['ping','session.snapshot']);
 const again=f.commands().length;
 assert.equal(f.cli('native','migrate-evidence','--legacy-state-dir',f.legacyState).evidence_reset.already_completed,true);
 assert.deepEqual(f.commands().slice(again).map(([method])=>method),['ping']);
});

test('real protocol-1 child emits native scope changes, deduplicates observations and preserves stale native fences', {timeout:300000}, async t=>{
 const f=real(t,{enrolled:false}),beforeStart=f.commands().length;
 const child=spawn(binary,['serve','--session-id',f.id,'--plugin-root',plugin],{env:{...process.env},cwd:f.directory,stdio:['pipe','pipe','pipe']});
 const closed=once(child,'close'),frames=[],waiting=[];let stderr='';
 child.stderr.setEncoding('utf8').on('data',chunk=>{stderr+=chunk;});
 createInterface({input:child.stdout}).on('line',line=>{
  const value=JSON.parse(line),resolve=waiting.shift();if(resolve)resolve(value);else frames.push(value);
 });
 const next=()=>frames.length?Promise.resolve(frames.shift()):new Promise(resolve=>waiting.push(resolve));
 const request=(id,argv,input)=>child.stdin.write(JSON.stringify({type:'request',id,argv,...(input===undefined?{}:{input})})+'\n');
 t.after(async()=>{child.kill('SIGKILL');await closed;});
 assert.deepEqual(await next(),{type:'ready',protocol:1});
 const unready=await next();assert.equal(unready.event,'scope');assert.equal(unready.value,null);assert.equal(typeof unready.reason,'string');
 const unreadyCost=f.commands().length-beforeStart,beforeUnready=f.commands().length;
 request(12,['native','observe']);const unavailable=await next();assert.equal(unavailable.id,12);assert.equal(unavailable.code,1);
 assert.equal(unavailable.value.error.message,unready.reason);
 assert.equal(unreadyCost,3,'startup must ping, subscribe, then observe once');
 assert.equal(f.commands().length-beforeUnready,1,'a later observation reuses the pinned client');
 const unenrolled=f.cli('agent','context','--harness','pi','--session-id',f.id,'--stream-id',f.stream.id);
 request(10,['agent','enroll'],{operation_id:'child-enroll',catalog_source:unenrolled.catalog_source,harness:'pi',harness_session_id:f.id,
  actor:'user',cwd:f.directory,stream_id:f.stream.id,session:{title:'Real Rust lane'}});
 const readyScope=await next();assert.equal(readyScope.event,'scope');assert.equal(readyScope.value.scope.nativeSessionId,f.id);
 const enrolled=await next();assert.equal(enrolled.id,10);assert.equal(enrolled.code,0,JSON.stringify(enrolled));
 const beforeCli=f.commands().length;
 assert.deepEqual(readyScope.value,f.cli('native','--session-id',f.id,'observe'));
 const observeCost=f.commands().length-beforeCli;
 const context=f.cli('agent','context','--harness','pi','--session-id',f.id);
 const input={operation_id:'child-title',catalog_source:context.catalog_source,actor:'user',kind:'session',before:context.session,
  changes:{title:'Child title'},ownership_token:context.authority.token,inputs:[{kind:'stream',record:context.stream}]};
 request(1,['agent','apply','--json'],input);
 const renamed=await next();assert.equal(renamed.event,'scope');assert.equal(renamed.value.session.title,'Child title');
 let reply=await next();assert.equal(reply.type,'response');assert.equal(reply.id,1);assert.equal(reply.code,0,JSON.stringify(reply));assert.deepEqual(reply.notices,[]);
 // One snapshot, without repeating the CLI's protocol check.
 const beforeObserve=f.commands().length;
 request(2,['native','--session-id',f.id,'observe','--json']);
 reply=await next();assert.equal(reply.id,2);assert.equal(reply.code,0);assert.deepEqual(reply.value,renamed.value);
 assert.equal(f.commands().length-beforeObserve,observeCost-1);
 // Neither rejected saves nor unchanged observations should emit scope events.
 request(3,['agent','apply'],null);reply=await next();assert.equal(reply.id,3);assert.equal(reply.code,1);
 request(11,['agent','enroll'],null);reply=await next();assert.equal(reply.id,11);assert.equal(reply.code,1);
 f.native.pane.tab_id='moved-tab';f.native.snapshot.tabs[0].tab_id='moved-tab';f.native.snapshot.layouts[0].tab_id='moved-tab';f.put();
 request(4,['native','project','--session-id',f.id,'--expect',JSON.stringify({version:1,scope:renamed.value.scope,context:renamed.value.context})]);
 const movedFrames=[await next(),await next()],failed=movedFrames.find(frame=>frame.type==='response'),moved=movedFrames.find(frame=>frame.event==='scope');
 assert.equal(failed.id,4);assert.equal(failed.code,1);assert.equal(failed.value.error.kind,'native-context-changed',JSON.stringify(failed));
 assert.ok(moved.value,JSON.stringify(moved));assert.equal(moved.value.context.tabId,'moved-tab');assert.equal(moved.value.scope.sessionId,renamed.value.scope.sessionId);
 // Adopting the new expectation must permit effects despite inherited tab env.
 request(5,['native','project','--expect',JSON.stringify({version:1,scope:moved.value.scope,context:moved.value.context})]);
 const bound=await next();assert.equal(bound.event,'scope');assert.equal(typeof bound.value.context.binding,'string');
 reply=await next();assert.equal(reply.id,5);assert.equal(reply.code,0,JSON.stringify(reply));
 request(6,['workspace','capture',f.stream.id]);reply=await next();assert.equal(reply.id,6);assert.equal(reply.code,0,JSON.stringify(reply));
 assert.equal(reply.value.stream_id,f.stream.id);assert.ok(reply.value.arrangement.tabs.length);
 request(7,['sync','--json']);reply=await next();assert.equal(reply.id,7);assert.equal(reply.code,0,JSON.stringify(reply));
 const beforeFailures=f.commands().length;
 request(8,['agent','apply'],null);reply=await next();assert.equal(reply.id,8);assert.equal(reply.code,1);
 request(9,['agent','enroll'],null);reply=await next();assert.equal(reply.id,9);assert.equal(reply.code,1);
 child.stdin.end();assert.equal((await closed)[0],0,stderr);assert.deepEqual(frames,[]);assert.equal(stderr,'');
 assert.equal(f.commands().length,beforeFailures);
});
test('Pi native updates reuse one real child with bounded socket work and no Herdr CLI spawns',async t=>{
 const f=real(t),notices=[],requests=[];let spawns=0;
 const child=new StreamsChild({sessionId:f.id,cwd:f.directory,env:{...process.env},warning:text=>notices.push(text),
  spawn:(...args)=>{spawns++;return spawn(...args);}});
 const run=child.run;child.run=(argv,...rest)=>{requests.push(argv);return run(argv,...rest);};
 const controller=new NativeController({pi:f.pi,ctx:f.ctx,factory:input=>f.load({...input,child}),capture:assert.fail,warning:text=>notices.push(text)});
 t.after(async()=>{controller.stop();await controller.idle();await child.stop();});
 controller.start();await controller.idle();
 assert.deepEqual(requests.map(command),['project','capture'],'the first binding echo scheduled a duplicate startup pass');
 for(let i=0;i<4;i++) {
  const before=f.commands().length,requestCount=requests.length;
  controller.update();await controller.idle();
  assert.deepEqual(requests.slice(requestCount).map(command),['project','capture']);
  // CLI budgets: project 10, capture 5; serve may recompute scope (2) after each.
  assert.ok(f.commands().length-before<=19,'Pi update exceeded the declared socket request budget');
  assert.equal(spawns,1,'a Pi update started another Streams process');
 }
 assert.deepEqual(notices,[]);
});
test('an identical child scope echo schedules zero project or capture calls',async t=>{
 const f=fake(t),child=f.child();f.observation.echoScope=true;f.put(f.observation);
 const controller=new NativeController({pi:f.pi,ctx:f.ctx,factory:()=>f.load({child}),warning:assert.fail});
 t.after(async()=>{controller.stop();await controller.idle();});
 controller.start();await controller.idle();
 const before=f.commands().filter(args=>['project','capture'].includes(command(args))).length;
 await child.run(['native','observe','--session-id',f.id]);await controller.idle();
 assert.equal(f.commands().filter(args=>['project','capture'].includes(command(args))).length,before);
});
test('real Rust owner observation is read-only and rejects stale context before metadata effects',t=>{
 const f=real(t),before=readFileSync(join(f.directory,'native.json'),'utf8');
 const observed=f.cli('native','--session-id',f.id,'observe');
 assert.equal(observed.version,1);assert.equal(observed.scope.nativeSessionId,f.id);assert.equal(observed.context.terminalId,'terminal');assert.equal(observed.context.binding,null);
 assert.deepEqual(observed.config,{automatic:false,sessionNaming:true});
 assert.equal(readFileSync(join(f.directory,'native.json'),'utf8'),before);assert.equal(existsSync(join(f.state,'native-cards')),false);
 const expected={version:1,scope:observed.scope,context:observed.context};
 f.native.pane.terminal_id='replacement';f.native.snapshot.panes[0].terminal_id='replacement';f.put();
 const result=f.invoke('native','--session-id',f.id,'--expect',JSON.stringify(expected),'project');
 assert.notEqual(result.code,0);assert.match(result.data.error.message,/changed/);assert.equal(result.data.error.kind,'native-context-changed');
 assert.ok(f.commands().every(([method])=>!method.endsWith('report_metadata')));
});
test('launched-card readers reject a socket or namespace that contradicts the pinned endpoint',t=>{
 const f=real(t),observed=f.cli('native','--session-id',f.id,'observe');
 const keys=['HOME','XDG_CONFIG_HOME','XDG_DATA_HOME','XDG_STATE_HOME','PI_CODING_AGENT_DIR','PATH','HERDR_SOCKET_PATH','HERDR_SESSION'];
 const reader=Object.fromEntries(keys.filter(key=>process.env[key]!==undefined).map(key=>[key,process.env[key]]));
 const card={...f.native.pane,pane_id:'card',terminal_id:'card-terminal',agent:null,agent_session:null};
 f.native.snapshot.panes.push(card);f.native.snapshot.layouts[0].panes.push({pane_id:'card'});f.put();
 Object.assign(process.env,{HERDR_PANE_ID:'card',STREAMS_CARDS_SCOPE:JSON.stringify(observed.scope),
  STREAMS_CARDS_OWNER:JSON.stringify({pane_id:'pane',tab_id:'tab',workspace_id:'workspace'}),
  STREAMS_CARDS_NONCE:'nonce',STREAMS_CARDS_OWNER_TERMINAL:'terminal',STREAMS_CARDS_READ_CWD:f.directory,
  STREAMS_CARDS_STATE_DIR:join(f.directory,'native-state'),STREAMS_CARDS_PLUGIN_ROOT:plugin});
 for(const change of [{HERDR_SOCKET_PATH:join(f.directory,'different.sock')},{HERDR_SESSION:'different'}]) {
  process.env.STREAMS_CARDS_READ_ENV=JSON.stringify({...reader,...change});
  const result=f.invoke('native','--session-id',f.id,'check');
  assert.notEqual(result.code,0);assert.match(result.data.error.message,/reader endpoint changed/);
 }
 assert.ok(f.commands().every(([method])=>['ping','session.snapshot'].includes(method)));
});
test('an unresolved sibling pane blocks capture with its identity and preserves the saved layout',t=>{
 const f=real(t);
 f.cli('native','--session-id',f.id,'capture');
 const arrangements=readdirSync(join(f.state,'arrangements'),{recursive:true}).filter(path=>path.endsWith('.json'));
 assert.equal(arrangements.length,1);
 const path=join(f.state,'arrangements',arrangements[0]),before=readFileSync(path,'utf8');
 const sibling={...f.native.pane,pane_id:'unresolved-sibling',terminal_id:'sibling-terminal',agent_session:null};
 f.native.snapshot.panes.push(sibling);f.native.snapshot.agents.push(sibling);
 f.native.snapshot.layouts[0].panes.push({pane_id:sibling.pane_id});
 for(const metadata of [null,undefined,{agent:'codex',kind:'id',value:'wrong-harness'}]) {
  sibling.agent_session=metadata;f.put();
  const refused=f.invoke('native','--session-id',f.id,'capture');
  assert.notEqual(refused.code,0);
  assert.match(refused.data.error.message,/pane unresolved-sibling/);
  assert.match(refused.data.error.message,/workspace workspace/);
  assert.match(refused.data.error.message,/tab tab/);
  assert.equal(readFileSync(path,'utf8'),before);
 }
 assert.ok(f.commands().every(([method])=>['ping','session.snapshot'].includes(method)));
});
test('Pi lifecycle preserves the named Herdr namespace through Rust source rereads, projection and capture',async t=>{
 const f=real(t);
 assert.equal(process.env.HERDR_SESSION,'fixture');
 const notices=[],controller=new NativeController({pi:f.pi,ctx:f.ctx,factory:f.load,capture:assert.fail,warning:text=>notices.push(text)});t.after(async()=>{controller.stop();await controller.idle();});
 controller.start();await controller.idle();assert.deepEqual(notices,[]);
 assert.equal(f.name(),'Real Rust lane');assert.ok(f.commands().some(([method])=>method.endsWith('report_metadata')));
 const allowed=['ping','events.subscribe','pane.current','workspace.get','session.snapshot','workspace.report_metadata','pane.report_metadata'];
 assert.ok(f.commands().every(([method])=>allowed.includes(method)));
 const snapshot=JSON.parse(readFileSync(join(f.directory,'native.json'),'utf8')).snapshot;
 assert.ok(snapshot.workspaces[0].tokens.strm_binding);
 const arrangements=readdirSync(join(f.state,'arrangements'),{recursive:true}).filter(path=>path.endsWith('.json'));
 assert.equal(arrangements.length,1);assert.ok(JSON.parse(readFileSync(join(f.state,'arrangements',arrangements[0]),'utf8')).tabs.length);
 controller.stop();await controller.idle();delete process.env.HERDR_SOCKET_PATH;
 const observed=f.cli('native','--session-id',f.id,'observe'),expected={version:1,scope:observed.scope,context:observed.context};
 // Without an explicit socket, the named session resolves to the same endpoint.
 assert.equal(observed.context.socket,f.native.socket);
 const captured=f.cli('native','--session-id',f.id,'--expect',JSON.stringify(expected),'capture');
 assert.equal(captured.stream_id,observed.scope.streamId);
 const file=join(f.directory,'native.json'),live=JSON.parse(readFileSync(file,'utf8'));
 live.replaceOwnerAfterSnapshot=2;
 writeFileSync(file,JSON.stringify(live));
 const arrangementFile=join(f.state,'arrangements',arrangements[0]),before=readFileSync(arrangementFile,'utf8');
 // The owner changes during topology capture; the second lock-held check must refuse persistence.
 const refused=f.invoke('native','--session-id',f.id,'--expect',JSON.stringify(expected),'capture');
 assert.notEqual(refused.code,0);assert.match(refused.data.error.message,/terminal|changed/i);
 assert.equal(readFileSync(arrangementFile,'utf8'),before);
 f.commands(); // Assert zero Herdr CLI spawns throughout the lifecycle.
});
