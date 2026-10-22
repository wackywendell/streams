import assert from 'node:assert/strict';
import test from 'node:test';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createJiti } from 'jiti';
const jiti=createJiti(import.meta.url);
const {loadNative, NativeController}=await jiti.import('../extensions/stream-checkpoint/native.ts');
const root=fileURLToPath(new URL('../',import.meta.url));
const binary=join(resolve(root,process.env.CARGO_TARGET_DIR ?? 'target'),'debug','streams');
const plugin=join(root,'integrations/herdr-work-cards');
function temporary(t) {
 const directory=mkdtempSync(join(realpathSync(tmpdir()),'streams-rust-native-'));
 t.after(()=>rmSync(directory,{recursive:true,force:true}));return directory;
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
 const directory=temporary(t),id=randomUUID(),f=piContext(directory,id);
 const file=join(directory,'observation.json'),log=join(directory,'commands.jsonl'),executable=join(directory,'rust cli');
 const observation={version:1,scope:{catalogSource:'catalog',hostId:'host',streamId:'stream',sessionId:'session',nativeSessionId:id},native:{pane_id:'pane',tab_id:'tab',workspace_id:'workspace'},context:{socket:join(directory,'not-live.sock'),paneId:'pane',terminalId:'terminal',tabId:'tab',workspaceId:'workspace',binding:null},session:{title:'Saved title',title_mode:'automatic'},config:{automatic:false,sessionNaming:true}};
 const put=value=>writeFileSync(file,JSON.stringify(value));put(observation);
 writeFileSync(executable,`#!${process.execPath}\nconst fs=require('node:fs');if(process.env.STREAMS_RUST_BIN!==fs.realpathSync(${JSON.stringify(executable)}))throw Error('native launch binary was not canonicalized');const args=process.argv.slice(2);fs.appendFileSync(${JSON.stringify(log)},JSON.stringify(args)+'\\n');const value=JSON.parse(fs.readFileSync(${JSON.stringify(file)},'utf8'));const command=args.find(arg=>['observe','open','close','project','capture'].includes(arg));if(value.fail===command){console.log(JSON.stringify({error:{kind:value.failureKind??'error',message:value.failMessage??'uncertain native effect; pending retained'}}));process.exit(1);}if(value.wait===command){setInterval(()=>{},1000);}else{const native={workspace_id:value.context.workspaceId,owner_pane_id:value.context.paneId,owner_terminal_id:value.context.terminalId,session_id:value.scope.sessionId,native_session_id:value.scope.nativeSessionId,binding:value.ackBinding??value.context.binding};console.log(JSON.stringify(value.invalid===command?{}:command==='observe'?value:command==='open'?{display:'opened',native}:command==='close'?{closed:true}:command==='project'?{names:{workspace:'disabled',tab:'disabled'},native}:{captured:true}));}\n`,{mode:0o700});
 lifetime(t,directory,{STREAMS_BIN_PATH:executable});
 const commands=()=>existsSync(log)?readFileSync(log,'utf8').trim().split('\n').filter(Boolean).map(JSON.parse):[];
 return {...f,directory,id,executable,observation,put,commands,load:()=>loadNative({pi:f.pi,ctx:f.ctx,sessionId:id,cwd:directory,env:{...process.env}})};
}
function command(args) {return args.find(arg=>['observe','open','close','project','capture'].includes(arg));}
async function until(check) {const deadline=Date.now()+5000;while(!check()&&Date.now()<deadline) await new Promise(resolve=>setTimeout(resolve,5));assert.ok(check(),'child did not reach requested command');}

test('Rust routing stays lazy and routes the Pi lane without TypeScript native effects or Python capture',async t=>{
 const f=fake(t),notices=[];
 const adapter=await f.load();assert.deepEqual(f.commands(),[]);
 const controller=new NativeController({pi:f.pi,ctx:f.ctx,factory:async()=>adapter,capture:assert.fail,warning:text=>notices.push(text)});
 t.after(()=>controller.stop());controller.start();await controller.idle();
 assert.deepEqual(f.commands().map(command),['observe','observe','project','observe','capture']);
 assert.equal(f.name(),'Saved title');assert.deepEqual(notices,[]);
 assert.equal(adapter.automatic,false);assert.equal(adapter.sessionNaming,true);
 for(const args of f.commands().filter(args=>command(args)!=='observe')) {
  assert.deepEqual(JSON.parse(args[args.indexOf('--expect')+1]),{version:1,scope:f.observation.scope,context:f.observation.context});
  assert.equal(args[args.indexOf('--plugin-root')+1],plugin);assert.equal(args[args.indexOf('--session-id')+1],f.id);
 }
 await controller.open();await controller.close();
 assert.equal(f.commands().filter(args=>command(args)==='open').length,1);
 assert.equal(command(f.commands().at(-1)),'close');
 for(const [target,reclaim] of [['all','all'],['session','tab'],['stream','workspace']]) {
  await controller.reclaimNames(target);
  const args=f.commands().findLast(args=>command(args)==='project');assert.equal(args[args.indexOf('--reclaim')+1],reclaim);
 }
 assert.equal(f.commands().filter(args=>command(args)==='open').length,1);
});
test('Rust adapter pins scope, endpoint, terminal and binding for the whole Pi lifetime',async t=>{
 const f=fake(t),adapter=await f.load();await adapter.read();
 f.observation.context.binding='first-binding';f.put(f.observation);await adapter.read();
 const context=await adapter.bootstrap();
 await assert.rejects(adapter.open({...context,terminalId:'reused'},new AbortController().signal,'explicit'),/context changed/);
 f.observation.context.binding='replacement';f.put(f.observation);
 await assert.rejects(adapter.read(),/scope changed/);
 f.observation.context.binding='first-binding';f.put(f.observation);
 const before=f.commands().length;await assert.rejects(adapter.read(),/scope changed/);
 assert.equal(f.commands().length,before);assert.ok(f.commands().every(args=>command(args)==='observe'));
});
for(const operation of ['open','project','close']) test(`Rust ${operation} rejects malformed acknowledgements and preserves actionable failure`,async t=>{
 const f=fake(t),adapter=await f.load();await adapter.read();const context=await adapter.bootstrap(),signal=new AbortController().signal;
 const invoke=()=>adapter[operation](context,signal,'explicit');
 f.observation.invalid=operation;f.put(f.observation);await assert.rejects(invoke(),/acknowledgement/);
 delete f.observation.invalid;f.observation.fail=operation;f.put(f.observation);
 await assert.rejects(invoke(),/uncertain native effect; pending retained/);
 assert.equal(f.commands().filter(args=>command(args)===operation).length,2);
});
test('an effect scope mismatch permanently fences the Rust adapter even after restoration',async t=>{
 const f=fake(t),adapter=await f.load();await adapter.read();const context=await adapter.bootstrap();
 f.observation.fail='project';f.observation.failureKind='native-context-changed';f.put(f.observation);
 await assert.rejects(adapter.project(context,new AbortController().signal),/scope changed/);
 delete f.observation.fail;f.put(f.observation);
 const before=f.commands().length;await assert.rejects(adapter.read(),/scope changed/);assert.equal(f.commands().length,before);
});
test('an acknowledged first binding is required by the very next effect subprocess',async t=>{
 const f=fake(t),adapter=await f.load();await adapter.read();const context=await adapter.bootstrap();
 f.observation.ackBinding='created-binding';f.put(f.observation);
 await adapter.open(context,new AbortController().signal,'explicit');
 f.observation.context.binding='created-binding';f.put(f.observation);
 await adapter.project(context,new AbortController().signal);
 const args=f.commands().at(-1);assert.equal(JSON.parse(args[args.indexOf('--expect')+1]).context.binding,'created-binding');
});
for(const message of ['native projection requires an enrolled Stream and Session','requested Pi session differs from the native pane session','native projection requires a ready workspace association for this Stream']) test(`unready Rust startup remains quiet: ${message}`,async t=>{
 const f=fake(t);f.observation.fail='observe';f.observation.failMessage=message;f.put(f.observation);
 const notices=[],controller=new NativeController({pi:f.pi,ctx:f.ctx,warning:text=>notices.push(text)});t.after(()=>controller.stop());
 controller.start();await controller.idle();assert.deepEqual(notices,[]);await assert.rejects(controller.open(),error=>error.message.includes(message));
 assert.ok(f.commands().every(args=>command(args)==='observe'));
});
test('Rust native cancellation drains the subprocess before shutdown and never invokes close',async t=>{
 const f=fake(t);f.observation.config.automatic=true;f.observation.wait='open';f.put(f.observation);
 const notices=[],controller=new NativeController({pi:f.pi,ctx:f.ctx,warning:text=>notices.push(text)});t.after(()=>controller.stop());
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
   await assert.rejects(loadNative({pi:f.pi,ctx:f.ctx,sessionId:f.id,cwd:f.directory,env}),new RegExp(selector));
  }
 }
 assert.deepEqual(f.commands(),[]);
});
test('native rejects conflicting reporting and compatibility selectors before any subprocess',async t=>{
 const f=fake(t),other=join(f.directory,'other-cli');symlinkSync(process.execPath,other);
 process.env.STREAMS_RUST_BIN=other;
 await assert.rejects(f.load(),/STREAMS_BIN_PATH and STREAMS_RUST_BIN select different/);
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

function real(t) {
 assert.ok(existsSync(binary),`Build the requested Rust CLI first: ${binary}`);
 const directory=temporary(t),id=randomUUID(),socket=join(directory,'not-live.sock'),fixture=join(directory,'native.json'),log=join(directory,'native-commands.jsonl'),executable=join(directory,'herdr'),f=piContext(directory,id);
 lifetime(t,directory,{HERDR_BIN_PATH:executable,HERDR_SESSION:'fixture'});
 const pane={pane_id:'pane',terminal_id:'terminal',workspace_id:'workspace',tab_id:'tab',cwd:directory,agent:'pi',agent_session:{agent:'pi',kind:'id',value:id},tokens:{}};
 const native={pane,socket,snapshot:{workspaces:[{workspace_id:'workspace',label:'Fixture',number:1,tokens:{}}],tabs:[{workspace_id:'workspace',tab_id:'tab',label:'1',number:1}],panes:[pane],layouts:[{workspace_id:'workspace',tab_id:'tab',panes:[{pane_id:'pane'}]}],agents:[pane]}};
 const put=()=>writeFileSync(fixture,JSON.stringify(native));put();
 writeFileSync(executable,`#!${process.execPath}\nconst fs=require('node:fs');const file=${JSON.stringify(fixture)},log=${JSON.stringify(log)};const value=JSON.parse(fs.readFileSync(file,'utf8')),a=process.argv.slice(2).filter(arg=>arg!=='--json');fs.appendFileSync(log,JSON.stringify(a)+'\\n');let result;if(a.join(' ')==='status'){result={server:{running:true,compatible:true,endpoint_compatible:true,version:'0.9.3',socket:process.env.HERDR_SOCKET_PATH??value.socket,session:process.env.HERDR_SESSION??null}};}else if(a.join(' ')==='pane current --current'){result={result:{type:'pane_current',pane:value.pane}};}else if(a[0]==='agent'&&a[1]==='get'){result={result:{type:'agent_info',agent:value.pane}};}else if(a[0]==='workspace'&&a[1]==='get'){result={result:{type:'workspace_info',workspace:value.snapshot.workspaces[0]}};}else if(a.join(' ')==='api snapshot'){result={result:{type:'session_snapshot',snapshot:value.snapshot}};if(value.switchDiscoveryAfterSnapshot&&--value.switchDiscoveryAfterSnapshot===0){value.socket=value.replacementSocket;fs.writeFileSync(file,JSON.stringify(value));}if(value.replaceOwnerAfterSnapshot&&--value.replaceOwnerAfterSnapshot===0){value.pane.terminal_id='reused-terminal';value.snapshot.panes[0].terminal_id='reused-terminal';value.snapshot.agents[0].terminal_id='reused-terminal';fs.writeFileSync(file,JSON.stringify(value));}if(value.switchDiscoveryAfterSnapshot||value.replaceOwnerAfterSnapshot){fs.writeFileSync(file,JSON.stringify(value));}}else if(['workspace','pane'].includes(a[0])&&a[1]==='report-metadata'){const target=(a[0]==='workspace'?value.snapshot.workspaces:value.snapshot.panes).find(row=>(row.workspace_id===a[2]&&a[0]==='workspace')||(row.pane_id===a[2]&&a[0]==='pane'));for(let i=3;i<a.length;i++){if(a[i]==='--token'){const [k,...v]=a[++i].split('=');target.tokens[k]=v.join('=');}else if(a[i]==='--clear-token'){delete target.tokens[a[++i]];}}fs.writeFileSync(file,JSON.stringify(value));result={result:{type:'metadata_reported'}};}else{console.error('FORBIDDEN native command',a);process.exit(91);}console.log(JSON.stringify(result));\n`,{mode:0o700});
 const invoke=(...args)=>{
  const result=spawnSync(binary,[...args,'--json'],{env:process.env,cwd:directory,encoding:'utf8',timeout:120000});assert.ifError(result.error);
  let data;try{data=JSON.parse(result.stdout);}catch{assert.fail(result.stdout+result.stderr);}return {code:result.status,data};
 };
 const cli=(...args)=>{const result=invoke(...args);assert.equal(result.code,0,JSON.stringify(result.data));return result.data;};
 cli('init','--sync','none');cli('config','set','frontend.mode','none');
 const stream=cli('create','Effort').stream;
 const session=cli('session','attach',stream.id,'--harness','pi','--session-id',id,'--title','Real Rust lane').session;
 cli('config','set','frontend.mode','herdr');
 const config=join(directory,'cards.json');writeFileSync(config,JSON.stringify({workspace_startup:'manual',session_naming:true,workspace_naming:false,tab_naming:false}));process.env.WORK_CARDS_CONFIG_PATH=config;
 const catalog=cli('config','get','catalog.root'),hash=value=>createHash('sha256').update(value).digest('hex'),key=(...parts)=>hash('['+parts.map(JSON.stringify).join(', ')+']');
 const state=join(directory,'state/streams/catalogs',hash(realpathSync(catalog)),key(session.host_id));
 const association=join(state,'associations',key('herdr',socket,'fixture'),key('workspace')+'.json');
 mkdirSync(dirname(association),{recursive:true,mode:0o700});writeFileSync(association,JSON.stringify({version:1,workspace_id:'workspace',stream_id:stream.id,session_id:null,command:null,ready:true}),{mode:0o600});
 const transcript=join(directory,'pi/sessions',id+'.jsonl');mkdirSync(dirname(transcript),{recursive:true});
 writeFileSync(transcript,JSON.stringify({type:'session',version:3,id,cwd:directory,timestamp:new Date(0).toISOString()})+'\n');
 return {...f,directory,id,state,native,put,invoke,cli,commands:()=>readFileSync(log,'utf8').trim().split('\n').filter(Boolean).map(JSON.parse)};
}
test('native operation Herdr request budgets',t=>{
 const f=real(t);writeFileSync(join(f.directory,'native-commands.jsonl'),'');
 for(const operation of ['observe','check','project','capture']) {
  const start=f.commands().length;
  f.cli('native','--session-id',f.id,operation);
  const calls=f.commands().slice(start),counts={};
  for(const call of calls) {const key=call.slice(0,2).join(' ');counts[key]=(counts[key]??0)+1;}
  // Baseline process starts, tightened after the socket/observation change.
  const budget={observe:27,check:27,project:251,capture:74}[operation];
  assert.equal(calls.length,budget,`${operation}: ${JSON.stringify(counts)}`);
 }
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
 assert.ok(f.commands().every(args=>!args.includes('report-metadata')));
});
test('Pi lifecycle preserves the named Herdr namespace through Rust source rereads, projection and capture',async t=>{
 const f=real(t);
 assert.equal(process.env.HERDR_SESSION,'fixture');
 const notices=[],controller=new NativeController({pi:f.pi,ctx:f.ctx,capture:assert.fail,warning:text=>notices.push(text)});t.after(()=>controller.stop());
 controller.start();await controller.idle();assert.deepEqual(notices,[]);
 assert.equal(f.name(),'Real Rust lane');assert.ok(f.commands().some(args=>args.includes('report-metadata')));
 assert.ok(f.commands().every(args=>['status','pane','api','agent','workspace'].includes(args[0])));
 const snapshot=JSON.parse(readFileSync(join(f.directory,'native.json'),'utf8')).snapshot;
 assert.ok(snapshot.workspaces[0].tokens.strm_binding);
 const arrangements=readdirSync(join(f.state,'arrangements'),{recursive:true}).filter(path=>path.endsWith('.json'));
 assert.equal(arrangements.length,1);assert.ok(JSON.parse(readFileSync(join(f.state,'arrangements',arrangements[0]),'utf8')).tabs.length);
 controller.stop();delete process.env.HERDR_SOCKET_PATH;
 const observed=f.cli('native','--session-id',f.id,'observe'),expected={version:1,scope:observed.scope,context:observed.context};
 const file=join(f.directory,'native.json');
 let live=JSON.parse(readFileSync(file,'utf8'));
 live.replacementSocket=join(f.directory,'different-not-live.sock');live.switchDiscoveryAfterSnapshot=3;
 writeFileSync(file,JSON.stringify(live));
 // Discovery changes after bootstrap; capture must keep the original resolved endpoint.
 const captured=f.cli('native','--session-id',f.id,'--expect',JSON.stringify(expected),'capture');
 assert.equal(captured.stream_id,observed.scope.streamId);
 live=JSON.parse(readFileSync(file,'utf8'));assert.equal(live.socket,live.replacementSocket);
 live.socket=expected.context.socket;live.replaceOwnerAfterSnapshot=5;
 writeFileSync(file,JSON.stringify(live));
 const arrangementFile=join(f.state,'arrangements',arrangements[0]),before=readFileSync(arrangementFile,'utf8');
 // The owner changes during topology capture; the second lock-held check must refuse persistence.
 const refused=f.invoke('native','--session-id',f.id,'--expect',JSON.stringify(expected),'capture');
 assert.notEqual(refused.code,0);assert.match(refused.data.error.message,/terminal|changed/i);
 assert.equal(readFileSync(arrangementFile,'utf8'),before);
});
