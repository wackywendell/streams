import assert from 'node:assert/strict';
import test from 'node:test';
import { spawn, execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, writeFileSync, readFileSync, renameSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createJiti } from 'jiti';
import { childCli } from './helpers/stream-cli.mjs';
const jiti = createJiti(import.meta.url);
const load = name => jiti.import(new URL(`../extensions/stream-checkpoint/${name}.ts`, import.meta.url).pathname);
const { StreamCli, StreamsCliError } = await load('streams');
const { default: registerWorkCard } = await load('index');
const { editRequest } = await load('core');
const { defaults } = await load('settings');
const local = {local:'unchanged',sharing:'local-only',domain:null};
function context() {
 const stream={id:'effort',title:'Manual',title_mode:'manual',goal:'Manual goal',goal_short:null,goal_mode:'manual',status:'active',owner:{host_id:'host'},repositories:{},references:[]};
 const session={id:'session',host_id:'host',stream_id:'effort',harness:'pi',harness_session_id:'native',title:'Session',title_mode:'automatic',status:'active',state:'',state_short:null,remaining:'',checkout:null};
 return {catalog_source:'source',host_id:'host',stream,session,sessions:[session],title_defaults:{stream:'automatic',session:'automatic'}};
}
function hooks(cli, infer = () => assert.fail('lifecycle sync must not invoke a model'), settings = defaults) {
 const handlers=new Map(), warnings=[], entries=[];
 registerWorkCard({on:(name,fn)=>handlers.set(name,fn),registerCommand:()=>{},registerTool:()=>{},registerShortcut:()=>{},appendEntry:(type,value)=>entries.push({type,value})}, {cli,infer,settings});
 const ctx={cwd:'/repo',sessionManager:{getSessionId:()=> 'native',getLeafId:()=>null,getBranch:()=>[],buildSessionProjection:()=>({messages:[{role:'user',content:'Build work cards',timestamp:0}]})},ui:{notify:(text,level)=>warnings.push({text,level}),setStatus:()=>{}}};
 return {handlers,ctx,warnings,entries};
}

test('typed lifecycle preserves publication results and requests a total deadline',async()=>{
 for(const sharing of ['local-only','pending','confirmed','failed','blocked','uncertain']) {
  const calls=[], value={...local,sharing};
  const cli=new StreamCli('/tmp',async(...args)=>{calls.push(args);return {status:'success',result:value};});
  assert.deepEqual(await cli.sync(),value);
  assert.deepEqual(calls[0],[{kind:'sync'},{timeout:10000}]);
  await cli.sync();assert.deepEqual(calls[1][0],{kind:'sync'});
 }
 for(const outcome of [{status:'success',result:{}},{status:'error',error:{kind:'repair-required',message:'Inspect marker'}},{status:'success',result:{...local,sharing:'made-up'}}]) {
  await assert.rejects(new StreamCli('/tmp',async()=>outcome).sync(),StreamsCliError);
 }
});

test('startup awaits sync; shutdown is awaited, best effort and idempotent without a summary',async()=>{
 const events=[];let finish, settled=false;
 const f=hooks({sync:async()=>{events.push(events.length+1);if(events.length!==2)return local;return new Promise(resolve=>{finish=resolve;});}});
 await f.handlers.get('session_start')({},f.ctx);assert.deepEqual(events,[1]);
 // Pi constructs a fresh context per event, even within the same conversation.
 const shutdown=f.handlers.get('session_shutdown')({reason:'quit'},{...f.ctx});
 assert.equal(f.handlers.get('session_shutdown')({reason:'quit'},{...f.ctx}),shutdown);
 shutdown.then(()=>settled=true);await Promise.resolve();assert.equal(settled,false);
 finish({...local,sharing:'uncertain',detail:'lost push response'});await shutdown;
 assert.deepEqual(events,[1,2]);assert.equal(f.entries.length,1);assert.equal(f.entries[0].type,'stream-catalog-sync');
 assert.match(f.warnings[0].text,/lost push response/);assert.equal(f.warnings[0].level,'warning');
 f.handlers.get('agent_end')({messages:[{role:'assistant',stopReason:'stop'}]},f.ctx);
 await f.handlers.get('session_start')({},f.ctx);assert.deepEqual(events,[1,2,3]);
});

test('a late lifecycle failure cannot notify or append into a replacement conversation',async()=>{
 let finish, calls=0,entered;const started=new Promise(resolve=>entered=resolve);
 const f=hooks({sync:async()=>{if(++calls!==1)return local;entered();return new Promise(resolve=>finish=resolve);}});
 const previous=f.handlers.get('session_start')({},f.ctx);await started;
 const next={...f.ctx,sessionManager:{getSessionId:()=> 'replacement'}};
 await f.handlers.get('session_start')({},next);
 finish({...local,sharing:'failed',detail:'old startup failed'});await previous;
 assert.equal(f.warnings.length,0);assert.equal(f.entries.length,0);
});

test('shutdown cancels in-flight inference, never adds a summary, and retains sync errors',async()=>{
 let entered, signal, models=0, syncs=0;
 const started=new Promise(resolve=>entered=resolve);
 const f=hooks({context:async()=>({...context(),stream:null,session:null,sessions:[]}),sync:async()=>{if(++syncs===2)throw Error('offline or timed out');return local;}},async(_ctx,_request,abort)=>{models++;signal=abort;entered();return new Promise(resolve=>abort.addEventListener('abort',()=>resolve({meaningful:false}),{once:true}));});
 await f.handlers.get('session_start')({},f.ctx);
 f.handlers.get('agent_end')({messages:[{role:'assistant',stopReason:'stop'}]},f.ctx);await started;
 await f.handlers.get('session_shutdown')({reason:'reload'},{...f.ctx});
 assert.equal(signal.aborted,true);assert.equal(models,1);assert.equal(f.entries.length,1);
 assert.match(f.entries[0].value.error,/offline or timed out/);assert.match(f.warnings[0].text,/local saves and publication are separate/);
});

test('shutdown retires a pending startup before awaiting its own diagnostic result',async()=>{
 for(const reject of [false,true]) {
  let finishStart,finishShutdown,calls=0,entered;const started=new Promise(resolve=>entered=resolve);
  const f=hooks({sync:()=>new Promise((resolve,rejectStart)=>{
   if(++calls===1){finishStart=()=>reject?rejectStart(Error('stale start error')):resolve({...local,sharing:'failed',detail:'stale start result'});entered();}
   else finishShutdown=resolve;
  })});
  const start=f.handlers.get('session_start')({},f.ctx);await started;
  const shutdown=f.handlers.get('session_shutdown')({}, {...f.ctx});
  finishStart();await start;assert.deepEqual(f.entries,[]);assert.deepEqual(f.warnings,[]);
  finishShutdown({...local,sharing:'blocked',detail:'current shutdown result'});await shutdown;
  assert.equal(f.entries.length,1);assert.match(f.warnings[0].text,/current shutdown result/);
 }
});

test('a replacement startup retires an old shutdown completion but tree changes retain live sync diagnostics',async()=>{
 let finish,calls=0;const f=hooks({sync:async()=>++calls===2?new Promise(resolve=>finish=resolve):local});
 await f.handlers.get('session_start')({},f.ctx);
 const shutdown=f.handlers.get('session_shutdown')({}, {...f.ctx});
 const next={...f.ctx,sessionManager:{...f.ctx.sessionManager,getSessionId:()=> 'next'}};
 await f.handlers.get('session_start')({},next);
 finish({...local,sharing:'failed'});await shutdown;assert.deepEqual(f.entries,[]);
 let complete,entered;const started=new Promise(resolve=>entered=resolve);
 const g=hooks({sync:()=>new Promise(resolve=>{complete=resolve;entered();})});
 const start=g.handlers.get('session_start')({},g.ctx);await started;
 g.handlers.get('session_tree')({}, {...g.ctx});complete({...local,sharing:'failed',detail:'live transport failure'});await start;
 assert.equal(g.entries.length,1);assert.match(g.warnings[0].text,/live transport failure/);
});

test('distinct event contexts share the live inference scope and route receipt warnings to the latest UI',async()=>{
 for(const [sharing,level] of [['local-only',null],['failed','warning'],['blocked','warning'],['uncertain','warning']]) {
 let release,entered;const started=new Promise(resolve=>entered=resolve), writes=[];
 const catalog=context();
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
 const catalog=context();
 const f=hooks({sync:async()=>local,context:async()=>structuredClone(catalog),apply:()=>{entered();return new Promise(resolve=>complete=resolve);}},async()=>{models++;return {changes:{state:'Accepted locally'}};});
 await f.handlers.get('session_start')({},f.ctx);
 f.handlers.get('agent_end')({messages:[{role:'assistant',stopReason:'stop'}]}, {...f.ctx});await applying;
 await f.handlers.get('session_shutdown')({}, {...f.ctx});
 complete({save:{local:'persisted',sharing:'failed',domain:{outcome:'changes'}}});
 await new Promise(resolve=>setImmediate(resolve));assert.equal(models,1);assert.deepEqual(f.warnings,[]);assert.deepEqual(f.entries,[]);
});

test('explicit debug capture inputs forward cancellation and never start a pre-aborted request', async()=>{
 const calls=[], controller=new AbortController();
 const cli=new StreamCli('/tmp',async(...args)=>{calls.push(args);return {status:'success',result:{}};});
 await cli.captureInputs({kind:'stream',id:'effort'},controller.signal);
 assert.deepEqual(calls[0],[{kind:'capture_inputs',target:{kind:'stream',stream_id:'effort'}},{timeout:10000,signal:controller.signal}]);
 controller.abort();await assert.rejects(cli.captureInputs({kind:'stream',id:'effort'},controller.signal),/abort/i);
 assert.equal(calls.length,1);
});

test('Pi/CLI reports publish across two isolated Git catalogs, remain saved offline and retry after restart', {timeout:120000}, async t=>{
 const root=mkdtempSync(join(tmpdir(),'streams-sync-acceptance-')),clients=[];t.after(async()=>{await Promise.all(clients.map(cli=>cli.stop()));rmSync(root,{recursive:true,force:true});});
 const checkout=fileURLToPath(new URL('..',import.meta.url)), remote=join(root,'remote.git'), unavailable=join(root,'offline.git');
 const binary=resolve(checkout,process.env.CARGO_TARGET_DIR??'target','debug','streams');
 const gitconfig=join(root,'gitconfig');writeFileSync(gitconfig,'[user]\n name = Acceptance\n email = acceptance@example.org\n[commit]\n gpgsign = false\n');
 const baseEnv={...process.env,GIT_CONFIG_GLOBAL:gitconfig,GIT_CONFIG_NOSYSTEM:'1',GIT_CONFIG_COUNT:'0',GIT_TERMINAL_PROMPT:'0'};
 for(const key of Object.keys(baseEnv))if(key.startsWith('STREAMS_') || /^GIT_(DIR|WORK_TREE|INDEX_FILE|COMMON_DIR|CONFIG_KEY_\d+|CONFIG_VALUE_\d+)$/.test(key))delete baseEnv[key];
 const git=(path,...args)=>execFileSync('git',['-C',path,...args],{env:baseEnv,encoding:'utf8',stdio:['ignore','pipe','pipe']}).trim();
 execFileSync('git',['init','--bare',remote],{env:baseEnv,stdio:'pipe'});
 const runner=host=>{
  const home=join(root,host);mkdirSync(home,{recursive:true});
  const env={...baseEnv,HOME:home,XDG_CONFIG_HOME:join(home,'config'),XDG_DATA_HOME:join(home,'data'),XDG_STATE_HOME:join(home,'state')};
  const run=(args,input)=>new Promise((resolve,reject)=>{
   const child=spawn(binary,args,{cwd:checkout,env,stdio:['pipe','pipe','pipe']});let stdout='',stderr='';
   child.stdout.on('data',chunk=>stdout+=chunk);child.stderr.on('data',chunk=>stderr+=chunk);child.on('error',reject);child.on('close',code=>resolve({code,stdout,stderr}));child.stdin.on('error',error=>{if(error.code!=='EPIPE')reject(error);});child.stdin.end(input);
  });
  return Object.assign(run,{env});
 };
 const a=runner('a'), b=runner('b');
 const command=async(run,...args)=>{const value=await run([...args,'--json']);assert.equal(value.code,0,value.stderr||value.stdout);return JSON.parse(value.stdout);};
 for(const run of [a,b])await command(run,'config','set','frontend.mode','none');
 const initialized=await command(a,'init','--sync','git','--remote',remote);
 let cli=childCli(checkout,'native',a.env);clients.push(cli);const empty=await cli.context('native');
 const enrollment=await cli.enroll({version:2,catalog_source:empty.catalog_source,harness:'pi',harness_session_id:'native',stream:{title:'Shared effort'},session:{title:'Implement sharing'},actor:'automatic',cwd:checkout});
 assert.equal(enrollment.publication,undefined);assert.ok(enrollment.saves.every(item=>item.save.local==='persisted'));
 assert.equal((await cli.sync()).sharing,'confirmed');
 await command(b,'init','--sync','git','--remote',remote);
 const imported=await command(b,'sync');assert.equal(imported.sharing,'confirmed',JSON.stringify(imported));
 const shown=await command(b,'show',enrollment.stream.id,'--refresh');assert.equal(shown.sessions.length,1);
 let captured=await cli.context('native');
 const saved=await cli.apply(editRequest(captured,'session',{state:'First report'},'automatic'));
 assert.equal(saved.save.local,'persisted');assert.equal(saved.publication,undefined);
 const before=git(remote,'rev-parse','main');
 let f=hooks(cli);await f.handlers.get('session_start')({},f.ctx);assert.notEqual(git(remote,'rev-parse','main'),before);
 assert.equal((await command(b,'show',enrollment.stream.id,'--refresh')).sessions[0].state,'First report');
 captured=await cli.context('native');
 const final=await cli.apply(editRequest(captured,'session',{state:'Final saved report'},'automatic'));assert.equal(final.save.local,'persisted');assert.equal(final.publication,undefined);
 await f.handlers.get('session_shutdown')({reason:'quit'},f.ctx);assert.equal(f.warnings.length,0);
 const published=git(remote,'rev-parse','main');
 const second=await command(b,'show',enrollment.stream.id,'--refresh');assert.equal(second.sessions[0].state,'Final saved report');assert.equal(git(remote,'rev-parse','main'),published);
 f=hooks(cli);await f.handlers.get('session_start')({},f.ctx);
 renameSync(remote,unavailable);
 captured=await cli.context('native');
 const localTip=git(initialized.catalog,'rev-parse','HEAD'),offlineIntent=editRequest(captured,'session',{state:'Saved while offline'},'automatic');
 const offline=await cli.apply(offlineIntent);
 assert.equal(offline.save.local,'persisted');assert.equal(offline.publication,undefined);
 assert.equal(git(initialized.catalog,'rev-parse','HEAD'),localTip,'saving does not start Git');
 await f.handlers.get('session_shutdown')({reason:'quit'},f.ctx);
 assert.equal(f.warnings.length,1);assert.equal(f.entries[0].value.result.sharing,'failed');assert.notEqual(git(initialized.catalog,'rev-parse','HEAD'),localTip,'explicit sync checkpoints locally before contacting the remote');
 assert.equal((await cli.context('native')).session.state,'Saved while offline');
 renameSync(unavailable,remote);
 // A new session child loses all in-memory state.
 await cli.stop();cli=childCli(checkout,'native',a.env);clients.push(cli);
 const retained=await cli.apply(offlineIntent);assert.equal(retained.save.local,'unchanged');assert.equal(retained.save.domain.outcome,'no-change');
 assert.equal(git(remote,'rev-parse','main'),published,'retained catalog retries do not publish');
 f=hooks(cli);await f.handlers.get('session_start')({},f.ctx);assert.deepEqual(f.warnings,[]);
 assert.equal((await command(b,'show',enrollment.stream.id,'--refresh')).sessions[0].state,'Saved while offline');
 assert.notEqual(git(remote,'rev-parse','main'),published);
});

test('Pi/CLI refuses adoption and keeps captured membership and receipts valid without a remote', {timeout:120000}, async t=>{
 const root=mkdtempSync(join(tmpdir(),'streams-local-git-acceptance-')),clients=[];t.after(async()=>{await Promise.all(clients.map(cli=>cli.stop()));rmSync(root,{recursive:true,force:true});});
 const checkout=fileURLToPath(new URL('..',import.meta.url)), home=join(root,'home');mkdirSync(home);
 const binary=resolve(checkout,process.env.CARGO_TARGET_DIR??'target','debug','streams');
 const gitconfig=join(root,'gitconfig');writeFileSync(gitconfig,'[user]\n name = Local Acceptance\n email = local@example.org\n[commit]\n gpgsign = false\n');
 const env={...process.env,HOME:home,XDG_CONFIG_HOME:join(home,'config'),XDG_DATA_HOME:join(home,'data'),XDG_STATE_HOME:join(home,'state'),GIT_CONFIG_GLOBAL:gitconfig,GIT_CONFIG_NOSYSTEM:'1',GIT_TERMINAL_PROMPT:'0'};
 for(const key of Object.keys(env))if(key.startsWith('STREAMS_') || key.startsWith('HERDR_') || key.startsWith('CMUX_') || key.startsWith('GIT_')&&!['GIT_CONFIG_GLOBAL','GIT_CONFIG_NOSYSTEM','GIT_TERMINAL_PROMPT'].includes(key))delete env[key];
 const run=(args,input)=>new Promise((resolve,reject)=>{
  const child=spawn(binary,args,{cwd:checkout,env,stdio:['pipe','pipe','pipe']});let stdout='',stderr='';
  child.stdout.on('data',chunk=>stdout+=chunk);child.stderr.on('data',chunk=>stderr+=chunk);child.on('error',reject);child.on('close',code=>resolve({code,stdout,stderr}));child.stdin.on('error',error=>{if(error.code!=='EPIPE')reject(error);});child.stdin.end(input);
 });
 const command=async(...args)=>{const result=await run([...args,'--json']);assert.equal(result.code,0,result.stderr||result.stdout);return JSON.parse(result.stdout);};
 await command('config','set','frontend.mode','none');
 const initialized=await command('init','--sync','none');
 let cli=childCli(checkout,'native',env);clients.push(cli);const empty=await cli.context('native');
 const enrollment=await cli.enroll({version:2,catalog_source:empty.catalog_source,harness:'pi',harness_session_id:'native',stream:{title:'Existing effort'},session:{title:'Continue work'},actor:'automatic',cwd:checkout});
 assert.equal(enrollment.publication,undefined);assert.ok(enrollment.saves.every(item=>item.save.local==='persisted'));
 const captured=await cli.context('native');
 const transport=join(initialized.catalog,'.catalog-local/transport.json'),before=readFileSync(transport,'utf8');
 for(const args of [['init','--sync','git','--adopt'],['init','--sync','git']]) {
  const refused=await run([...args,'--json']);assert.notEqual(refused.code,0);assert.ok(JSON.parse(refused.stdout).error);
  assert.equal(readFileSync(transport,'utf8'),before);assert.equal(existsSync(join(initialized.catalog,'.git')),false);
 }
 const after=await cli.context('native');assert.equal(after.catalog_source,captured.catalog_source);assert.deepEqual(after.session,captured.session);
 const intent=editRequest(captured,'session',{state:'Reported after refused adoption'},'automatic'),report=await cli.apply(intent);
 assert.equal(report.save.local,'persisted');assert.equal(report.publication,undefined);
 assert.deepEqual(JSON.parse(readFileSync(transport,'utf8')),{version:2,mode:{strategy:'none'}});
 await cli.stop();cli=childCli(checkout,'native',env);clients.push(cli);const f=hooks(cli);await f.handlers.get('session_start')({},f.ctx);await f.handlers.get('session_shutdown')({reason:'quit'},f.ctx);
 assert.deepEqual(f.warnings,[]);assert.deepEqual(f.entries,[]);
 const retained=await cli.apply(intent);assert.equal(retained.save.local,'unchanged');assert.equal(retained.save.domain.outcome,'no-change');
 const shown=await command('show',enrollment.stream.id,'--refresh');assert.equal(shown.sessions[0].id,enrollment.session.id);assert.equal(shown.sessions[0].state,'Reported after refused adoption');
 assert.equal((await command('sync')).sharing,'local-only');assert.equal(readFileSync(transport,'utf8'),before);
 assert.equal(existsSync(join(initialized.catalog,'.git')),false);assert.equal(existsSync(join(initialized.catalog,'.catalog-sync')),false);
});
