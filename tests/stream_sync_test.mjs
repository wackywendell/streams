import assert from 'node:assert/strict';
import test from 'node:test';
import { spawn, execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, chmodSync, renameSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createJiti } from 'jiti';
const jiti = createJiti(import.meta.url);
const load = name => jiti.import(new URL(`../extensions/stream-checkpoint/${name}.ts`, import.meta.url).pathname);
const { StreamCli, StreamsCliError } = await load('streams');
const { default: registerWorkCard } = await load('index');
const { editRequest } = await load('core');
const { defaults } = await load('settings');
const local = {local:'unchanged',sharing:'local-only',domain:null};
function hooks(cli, infer = () => assert.fail('lifecycle sync must not invoke a model'), settings = defaults) {
 const handlers=new Map(), warnings=[], entries=[];
 registerWorkCard({on:(name,fn)=>handlers.set(name,fn),registerCommand:()=>{},registerTool:()=>{},registerShortcut:()=>{},appendEntry:(type,value)=>entries.push({type,value})}, {cli,infer,settings});
 const ctx={cwd:'/repo',sessionManager:{getSessionId:()=> 'native',getLeafId:()=>null,getBranch:()=>[],buildSessionProjection:()=>({messages:[]})},ui:{notify:(text,level)=>warnings.push({text,level}),setStatus:()=>{}}};
 return {handlers,ctx,warnings,entries};
}

test('lifecycle CLI preserves failed publication results, passes the trigger and requests a total deadline',async()=>{
 for(const sharing of ['local-only','pending','confirmed','failed','blocked','uncertain']) {
  const calls=[], value={...local,sharing};
  const cli=new StreamCli('/tmp',async(...args)=>{calls.push(args);return {code:['failed','blocked','uncertain'].includes(sharing)?2:0,stdout:JSON.stringify(value),stderr:''};});
  assert.deepEqual(await cli.sync('start'),value);
  assert.deepEqual(calls[0],[['sync','--event','start','--json'],undefined,{timeout:10000}]);
  await cli.sync('shutdown');assert.deepEqual(calls[1][0],['sync','--event','shutdown','--json']);
 }
 for(const stdout of ['not JSON','{}',JSON.stringify({error:{kind:'repair-required',message:'Inspect marker'}}),JSON.stringify({...local,sharing:'made-up'})]) {
  await assert.rejects(new StreamCli('/tmp',async()=>({code:2,stdout,stderr:'failed'})).sync('shutdown'),StreamsCliError);
 }
});

test('startup awaits sync; shutdown is awaited, best effort and idempotent without a summary',async()=>{
 const events=[];let finish, settled=false;
 const f=hooks({sync:async event=>{events.push(event);if(event==='start')return local;return new Promise(resolve=>{finish=resolve;});}});
 await f.handlers.get('session_start')({},f.ctx);assert.deepEqual(events,['start']);
 // Pi constructs a fresh context per event, even within the same conversation.
 const shutdown=f.handlers.get('session_shutdown')({reason:'quit'},{...f.ctx});
 assert.equal(f.handlers.get('session_shutdown')({reason:'quit'},{...f.ctx}),shutdown);
 shutdown.then(()=>settled=true);await Promise.resolve();assert.equal(settled,false);
 finish({...local,sharing:'uncertain',detail:'lost push response'});await shutdown;
 assert.deepEqual(events,['start','shutdown']);assert.equal(f.entries.length,1);assert.equal(f.entries[0].type,'stream-catalog-sync');
 assert.match(f.warnings[0].text,/lost push response/);assert.equal(f.warnings[0].level,'warning');
 f.handlers.get('agent_end')({messages:[{role:'assistant',stopReason:'stop'}]},f.ctx);
 await f.handlers.get('session_start')({},f.ctx);assert.deepEqual(events,['start','shutdown','start']);
});

test('a late lifecycle failure cannot notify or append into a replacement conversation',async()=>{
 let finish, calls=0;
 const f=hooks({sync:async()=>++calls===1?new Promise(resolve=>finish=resolve):local});
 const previous=f.handlers.get('session_start')({},f.ctx);
 const next={...f.ctx,sessionManager:{getSessionId:()=> 'replacement'}};
 await f.handlers.get('session_start')({},next);
 finish({...local,sharing:'failed',detail:'old startup failed'});await previous;
 assert.equal(f.warnings.length,0);assert.equal(f.entries.length,0);
});

test('shutdown cancels in-flight inference, never adds a summary, and retains sync errors',async()=>{
 let entered, signal, models=0;
 const started=new Promise(resolve=>entered=resolve);
 const f=hooks({context:async()=>({catalog_source:'source',host_id:'host',stream:null,session:null,sessions:[],authority:null}),sync:async event=>{if(event==='shutdown')throw Error('offline or timed out');return local;}},async(_ctx,_request,abort)=>{models++;signal=abort;entered();return new Promise(resolve=>abort.addEventListener('abort',()=>resolve({meaningful:false}),{once:true}));});
 await f.handlers.get('session_start')({},f.ctx);
 f.handlers.get('agent_end')({messages:[{role:'assistant',stopReason:'stop'}]},f.ctx);await started;
 await f.handlers.get('session_shutdown')({reason:'reload'},{...f.ctx});
 assert.equal(signal.aborted,true);assert.equal(models,1);assert.equal(f.entries.length,1);
 assert.match(f.entries[0].value.error,/offline or timed out/);assert.match(f.warnings[0].text,/local saves and publication are separate/);
});

test('shutdown retires a pending startup before awaiting its own diagnostic result',async()=>{
 for(const reject of [false,true]) {
  let finishStart,finishShutdown;
  const f=hooks({sync:event=>new Promise((resolve,rejectStart)=>{
   if(event==='start')finishStart=()=>reject?rejectStart(Error('stale start error')):resolve({...local,sharing:'failed',detail:'stale start result'});
   else finishShutdown=resolve;
  })});
  const start=f.handlers.get('session_start')({},f.ctx);
  const shutdown=f.handlers.get('session_shutdown')({}, {...f.ctx});
  finishStart();await start;assert.deepEqual(f.entries,[]);assert.deepEqual(f.warnings,[]);
  finishShutdown({...local,sharing:'blocked',detail:'current shutdown result'});await shutdown;
  assert.equal(f.entries.length,1);assert.match(f.warnings[0].text,/current shutdown result/);
 }
});

test('a replacement startup retires an old shutdown completion but tree changes retain live sync diagnostics',async()=>{
 let finish;const f=hooks({sync:async event=>event==='shutdown'?new Promise(resolve=>finish=resolve):local});
 await f.handlers.get('session_start')({},f.ctx);
 const shutdown=f.handlers.get('session_shutdown')({}, {...f.ctx});
 const next={...f.ctx,sessionManager:{...f.ctx.sessionManager,getSessionId:()=> 'next'}};
 await f.handlers.get('session_start')({},next);
 finish({...local,sharing:'failed'});await shutdown;assert.deepEqual(f.entries,[]);
 let complete;const g=hooks({sync:()=>new Promise(resolve=>complete=resolve)});
 const start=g.handlers.get('session_start')({},g.ctx);
 g.handlers.get('session_tree')({}, {...g.ctx});complete({...local,sharing:'failed',detail:'live transport failure'});await start;
 assert.equal(g.entries.length,1);assert.match(g.warnings[0].text,/live transport failure/);
});

test('distinct event contexts share the live inference scope and route receipt warnings to the latest UI',async()=>{
 for(const [sharing,level] of [['local-only',null],['failed','warning'],['blocked','warning'],['uncertain','warning']]) {
 let release,entered;const started=new Promise(resolve=>entered=resolve), writes=[];
 const catalog={catalog_source:'source',host_id:'host',stream:{id:'effort',title:'Manual',title_mode:'manual',goal:'Manual goal',goal_mode:'manual'},session:{id:'session',harness_session_id:'native',stream_id:'effort',title:'Session',state:''},sessions:[],authority:{token:'owner'}};
 const f=hooks({sync:async()=>local,context:async()=>structuredClone(catalog),apply:async request=>{writes.push(request);return {save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}},publication:{local:'unchanged',sharing,domain:null}};}},async()=>{entered();await new Promise(resolve=>release=resolve);return {changes:{state:'Reported'}};});
 await f.handlers.get('session_start')({}, {...f.ctx});
 f.handlers.get('agent_end')({messages:[{role:'assistant',stopReason:'stop'}]}, {...f.ctx});await started;
 const notices=[],statuses=[];const latest={...f.ctx,ui:{...f.ctx.ui,notify:(text,level)=>notices.push({text,level}),setStatus:(_k,text)=>statuses.push(text)}};
 f.handlers.get('agent_end')({messages:[]},latest);
 release();for(let i=0;i<100&&(level?!notices.length:!statuses.some(s=>s&&s.includes('Updated')));i++)await new Promise(resolve=>setImmediate(resolve));
 assert.equal(writes.length,1);assert.equal(f.warnings.length,0);
 if(level){assert.equal(notices.length,1);assert.equal(notices[0].level,level);}
 else{assert.equal(notices.length,0);assert.match(statuses.at(-1),/Updated/);}
 }
});

test('shutdown suppresses late accepted report receipts without another inference',async()=>{
 let complete,entered,models=0;const applying=new Promise(resolve=>entered=resolve);
 const catalog={catalog_source:'source',host_id:'host',stream:null,session:{id:'session',host_id:'host',harness_session_id:'native',stream_id:'effort',title:'Session',state:''},sessions:[],authority:{token:'owner'}};
 const f=hooks({sync:async()=>local,context:async()=>structuredClone(catalog),apply:()=>{entered();return new Promise(resolve=>complete=resolve);}},async()=>{models++;return {changes:{state:'Accepted locally'}};});
 await f.handlers.get('session_start')({},f.ctx);
 f.handlers.get('agent_end')({messages:[{role:'assistant',stopReason:'stop'}]}, {...f.ctx});await applying;
 await f.handlers.get('session_shutdown')({}, {...f.ctx});
 complete({save:{local:'persisted',sharing:'failed',domain:{outcome:'changes'}}});
 await new Promise(resolve=>setImmediate(resolve));assert.equal(models,1);assert.deepEqual(f.warnings,[]);assert.deepEqual(f.entries,[]);
});

test('native lifecycle deadline kills the CLI and its child processes', {timeout:20000}, async t=>{
 const root=mkdtempSync(join(tmpdir(),'streams-sync-timeout-'));t.after(()=>rmSync(root,{recursive:true,force:true}));
 const bin=join(root,'bin');mkdirSync(bin);const pidfile=join(root,'pids.json');
 const shim=join(bin,'streams');
 writeFileSync(shim,`#!${process.execPath}\nconst {spawn}=require('node:child_process');const fs=require('node:fs');const child=spawn(process.execPath,['-e','setInterval(()=>{},1000)'],{stdio:'inherit'});fs.writeFileSync(${JSON.stringify(pidfile)},JSON.stringify([process.pid,child.pid]));setInterval(()=>{},1000);\n`);chmodSync(shim,0o755);
 const before=process.env.STREAMS_BIN_PATH;process.env.STREAMS_BIN_PATH=shim;t.after(()=>{if(before===undefined)delete process.env.STREAMS_BIN_PATH;else process.env.STREAMS_BIN_PATH=before;});
 await assert.rejects(new StreamCli(root).sync('shutdown'),/timed out.*uncertain.*repair state/);
 for(const pid of JSON.parse(readFileSync(pidfile,'utf8'))) {
  let state='';try {state=execFileSync('ps',['-p',String(pid),'-o','stat='],{encoding:'utf8',stdio:['ignore','pipe','pipe']}).trim();} catch(error) {assert.equal(error.status,1);}
  assert.ok(!state || state.startsWith('Z'),`sync process ${pid} still running: ${state}`);
 }
});

test('native capture forwards cancellation and never starts a pre-aborted request', async()=>{
 const calls=[], controller=new AbortController();
 const cli=new StreamCli('/tmp',async(...args)=>{calls.push(args);return {code:0,stdout:'{}',stderr:''};});
 await cli.captureArrangement('effort',controller.signal);
 assert.deepEqual(calls[0],[['workspace','capture','effort','--json'],undefined,{timeout:10000,signal:controller.signal}]);
 controller.abort();await assert.rejects(cli.captureArrangement('effort',controller.signal),/abort/i);
 assert.equal(calls.length,1);
});

test('native capture abort settles only after CLI and descendants stop', {timeout:10000}, async t=>{
 const root=mkdtempSync(join(tmpdir(),'streams-capture-abort-'));t.after(()=>rmSync(root,{recursive:true,force:true}));
 const bin=join(root,'bin');mkdirSync(bin);const pidfile=join(root,'pids.json');
 const shim=join(bin,'streams');
 writeFileSync(shim,`#!${process.execPath}\nconst {spawn}=require('node:child_process');const fs=require('node:fs');process.on('SIGTERM',()=>{});const child=spawn(process.execPath,['-e','process.on("SIGTERM",()=>{});setInterval(()=>{},1000)'],{stdio:'inherit'});fs.writeFileSync(${JSON.stringify(pidfile+'.tmp')},JSON.stringify([process.pid,child.pid]));fs.renameSync(${JSON.stringify(pidfile+'.tmp')},${JSON.stringify(pidfile)});setInterval(()=>{},1000);\n`);chmodSync(shim,0o755);
 const before=process.env.STREAMS_BIN_PATH;process.env.STREAMS_BIN_PATH=shim;t.after(()=>{if(before===undefined)delete process.env.STREAMS_BIN_PATH;else process.env.STREAMS_BIN_PATH=before;});
 const controller=new AbortController(), pending=new StreamCli(root).captureArrangement('effort',controller.signal);
 const rejected=assert.rejects(pending,/abort/i), deadline=performance.now()+5000;
 while(true) {
  try {readFileSync(pidfile);break;} catch(error) {if(error.code!=='ENOENT'||performance.now()>deadline)throw error;}
  await new Promise(resolve=>setTimeout(resolve,5));
 }
 controller.abort();await rejected;
 for(const pid of JSON.parse(readFileSync(pidfile,'utf8'))) {
  let state='';try {state=execFileSync('ps',['-p',String(pid),'-o','stat='],{encoding:'utf8',stdio:['ignore','pipe','pipe']}).trim();} catch(error) {assert.equal(error.status,1);}
  assert.ok(!state || state.startsWith('Z'),`native capture process ${pid} still running: ${state}`);
 }
});

test('Pi/CLI reports publish across two isolated Git catalogs, remain saved offline and retry after restart', {timeout:120000}, async t=>{
 const root=mkdtempSync(join(tmpdir(),'streams-sync-acceptance-'));t.after(()=>rmSync(root,{recursive:true,force:true}));
 const checkout=fileURLToPath(new URL('..',import.meta.url)), remote=join(root,'remote.git'), unavailable=join(root,'offline.git');
 const gitconfig=join(root,'gitconfig');writeFileSync(gitconfig,'[user]\n name = Acceptance\n email = acceptance@example.org\n[commit]\n gpgsign = false\n');
 const baseEnv={...process.env,GIT_CONFIG_GLOBAL:gitconfig,GIT_CONFIG_NOSYSTEM:'1',GIT_CONFIG_COUNT:'0',GIT_TERMINAL_PROMPT:'0'};
 for(const key of Object.keys(baseEnv))if(key.startsWith('STREAMS_') || /^GIT_(DIR|WORK_TREE|INDEX_FILE|COMMON_DIR|CONFIG_KEY_\d+|CONFIG_VALUE_\d+)$/.test(key))delete baseEnv[key];
 const git=(path,...args)=>execFileSync('git',['-C',path,...args],{env:baseEnv,encoding:'utf8',stdio:['ignore','pipe','pipe']}).trim();
 execFileSync('git',['init','--bare',remote],{env:baseEnv,stdio:'pipe'});
 const runner=host=>{
  const home=join(root,host);mkdirSync(home,{recursive:true});
  const env={...baseEnv,HOME:home,XDG_CONFIG_HOME:join(home,'config'),XDG_DATA_HOME:join(home,'data'),XDG_STATE_HOME:join(home,'state')};
  return (args,input)=>new Promise((resolve,reject)=>{
   const child=spawn('uv',['run','--project',checkout,'streams',...args],{cwd:checkout,env,stdio:['pipe','pipe','pipe']});let stdout='',stderr='';
   child.stdout.on('data',chunk=>stdout+=chunk);child.stderr.on('data',chunk=>stderr+=chunk);child.on('error',reject);child.on('close',code=>resolve({code,stdout,stderr}));child.stdin.on('error',error=>{if(error.code!=='EPIPE')reject(error);});child.stdin.end(input);
  });
 };
 const a=runner('a'), b=runner('b');
 const command=async(run,...args)=>{const value=await run([...args,'--json']);assert.equal(value.code,0,value.stderr||value.stdout);return JSON.parse(value.stdout);};
 for(const run of [a,b])await command(run,'config','set','frontend.mode','none');
 await command(a,'config','set','sync.stream_seconds','0');await command(a,'config','set','sync.session_seconds','999999');
 const initialized=await command(a,'init','--sync','git','--remote',remote);
 let cli=new StreamCli(checkout,a);const empty=await cli.context('native');
 const enrollment=await cli.enroll({catalog_source:empty.catalog_source,operation_id:'enrollment',harness:'pi',harness_session_id:'native',stream:{title:'Shared effort'},session:{title:'Implement sharing'},actor:'automatic',cwd:checkout});
 assert.equal(enrollment.publication.sharing,'confirmed');
 await command(b,'init','--sync','git','--remote',remote);
 const shown=await command(b,'show',enrollment.stream.id,'--refresh');assert.equal(shown.sessions.length,1);
 await command(a,'config','set','sync.stream_seconds','999999');
 let captured=await cli.context('native');
 const saved=await cli.apply(editRequest(captured,'session',{state:'First report'},'automatic'));
 assert.equal(saved.save.local,'committed');assert.equal(saved.publication.sharing,'pending');
 const before=git(remote,'rev-parse','main');
 let f=hooks(cli);await f.handlers.get('session_start')({},f.ctx);assert.equal(git(remote,'rev-parse','main'),before);
 await command(a,'config','set','sync.stream_seconds','0');
 f=hooks(cli);await f.handlers.get('session_start')({},f.ctx);assert.notEqual(git(remote,'rev-parse','main'),before);
 assert.equal((await command(b,'show',enrollment.stream.id,'--refresh')).sessions[0].state,'First report');
 captured=await cli.context('native');
 const final=await cli.apply(editRequest(captured,'session',{state:'Final saved report'},'automatic'));assert.equal(final.publication.sharing,'pending');
 await f.handlers.get('session_shutdown')({reason:'quit'},f.ctx);assert.equal(f.warnings.length,0);
 const published=git(remote,'rev-parse','main');
 const second=await command(b,'show',enrollment.stream.id,'--refresh');assert.equal(second.sessions[0].state,'Final saved report');assert.equal(git(remote,'rev-parse','main'),published);
 f=hooks(cli);await f.handlers.get('session_start')({},f.ctx);
 renameSync(remote,unavailable);
 captured=await cli.context('native');
 const offline=await cli.apply(editRequest(captured,'session',{state:'Saved while offline'},'automatic'));
 assert.equal(offline.save.local,'committed');assert.equal(offline.publication.sharing,'pending');
 const localTip=git(initialized.catalog,'rev-parse','HEAD');
 await f.handlers.get('session_shutdown')({reason:'quit'},f.ctx);
 assert.equal(f.warnings.length,1);assert.equal(f.entries[0].value.result.sharing,'failed');assert.equal(git(initialized.catalog,'rev-parse','HEAD'),localTip);
 assert.equal((await cli.context('native')).session.state,'Saved while offline');
 renameSync(unavailable,remote);
 // Each command is a fresh CLI process; the new client also loses all in-memory state.
 cli=new StreamCli(checkout,a);f=hooks(cli);await f.handlers.get('session_start')({},f.ctx);
 assert.equal(git(remote,'rev-parse','main'),published); // persisted failure cooldown, not discarded intent
 assert.equal((await command(a,'sync')).sharing,'confirmed');
 assert.equal((await command(b,'show',enrollment.stream.id,'--refresh')).sessions[0].state,'Saved while offline');
 assert.notEqual(git(remote,'rev-parse','main'),published);
});

test('Pi/CLI adopts populated local state and keeps captured membership valid without a remote', {timeout:120000}, async t=>{
 const root=mkdtempSync(join(tmpdir(),'streams-local-git-acceptance-'));t.after(()=>rmSync(root,{recursive:true,force:true}));
 const checkout=fileURLToPath(new URL('..',import.meta.url)), home=join(root,'home');mkdirSync(home);
 const gitconfig=join(root,'gitconfig');writeFileSync(gitconfig,'[user]\n name = Local Acceptance\n email = local@example.org\n[commit]\n gpgsign = false\n');
 const env={...process.env,HOME:home,XDG_CONFIG_HOME:join(home,'config'),XDG_DATA_HOME:join(home,'data'),XDG_STATE_HOME:join(home,'state'),GIT_CONFIG_GLOBAL:gitconfig,GIT_CONFIG_NOSYSTEM:'1',GIT_TERMINAL_PROMPT:'0'};
 for(const key of Object.keys(env))if(key.startsWith('STREAMS_') || key.startsWith('HERDR_') || key.startsWith('CMUX_') || key.startsWith('GIT_')&&!['GIT_CONFIG_GLOBAL','GIT_CONFIG_NOSYSTEM','GIT_TERMINAL_PROMPT'].includes(key))delete env[key];
 const run=(args,input)=>new Promise((resolve,reject)=>{
  const child=spawn('uv',['run','--offline','--no-sync','--project',checkout,'streams',...args],{cwd:checkout,env,stdio:['pipe','pipe','pipe']});let stdout='',stderr='';
  child.stdout.on('data',chunk=>stdout+=chunk);child.stderr.on('data',chunk=>stderr+=chunk);child.on('error',reject);child.on('close',code=>resolve({code,stdout,stderr}));child.stdin.on('error',error=>{if(error.code!=='EPIPE')reject(error);});child.stdin.end(input);
 });
 const command=async(...args)=>{const result=await run([...args,'--json']);assert.equal(result.code,0,result.stderr||result.stdout);return JSON.parse(result.stdout);};
 await command('config','set','frontend.mode','none');
 const initialized=await command('init','--sync','none');
 let cli=new StreamCli(checkout,run);const empty=await cli.context('native');
 const enrollment=await cli.enroll({catalog_source:empty.catalog_source,operation_id:'local-enrollment',harness:'pi',harness_session_id:'native',stream:{title:'Existing effort'},session:{title:'Continue work'},actor:'automatic',cwd:checkout});
 assert.equal(enrollment.publication.sharing,'local-only');assert.ok(enrollment.saves.every(item=>item.save.local==='persisted'));
 const captured=await cli.context('native');
 const adopted=await command('init','--sync','git','--adopt');
 assert.equal(adopted.host_id,initialized.host_id);assert.equal(adopted.catalog,initialized.catalog);
 const after=await cli.context('native');assert.equal(after.catalog_source,captured.catalog_source);assert.deepEqual(after.session,captured.session);assert.deepEqual(after.authority,captured.authority);
 const report=await cli.apply(editRequest(captured,'session',{state:'Reported after adoption'},'automatic'));
 assert.equal(report.save.local,'committed');assert.equal(report.save.sharing,'local-only');assert.equal(report.publication.sharing,'local-only');
 const git=(...args)=>execFileSync('git',['-C',adopted.catalog,...args],{env,encoding:'utf8',stdio:['ignore','pipe','pipe']}).trim();
 assert.equal(git('rev-parse','HEAD^'),adopted.baseline_revision);assert.equal(git('remote'),'');
 assert.equal(JSON.parse(readFileSync(join(adopted.catalog,'.catalog-sync/config.json'),'utf8')).remote,null);
 cli=new StreamCli(checkout,run);const f=hooks(cli);await f.handlers.get('session_start')({},f.ctx);await f.handlers.get('session_shutdown')({reason:'quit'},f.ctx);
 assert.deepEqual(f.warnings,[]);assert.deepEqual(f.entries,[]);
 const shown=await command('show',enrollment.stream.id,'--refresh');assert.equal(shown.sessions[0].id,enrollment.session.id);assert.equal(shown.sessions[0].state,'Reported after adoption');
 assert.equal((await command('sync')).sharing,'local-only');assert.equal((await command('init','--sync','git','--adopt')).baseline_revision,adopted.baseline_revision);
 for(const name of ['sync.json','publication.json'])assert.throws(()=>readFileSync(join(adopted.catalog,'.catalog-sync',name)),{code:'ENOENT'});
});
