import assert from 'node:assert/strict';
import test from 'node:test';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createJiti } from 'jiti';
import { SessionManager } from '@earendil-works/pi-coding-agent';
const jiti = createJiti(import.meta.url);
const load = name => jiti.import(new URL(`../extensions/stream-checkpoint/${name}.ts`, import.meta.url).pathname);
const { infer, buildInferencePayload, parseInferenceResponse, checkpointContext } = await load('agent');
const { captureSessions, loadCapture, parseCaptureInventory, captureIds } = await load('capture');
const { replayCapture } = await load('replay');
const { debugCommand, parseDebugCommand } = await load('debug');
const { StreamCli } = await load('streams');
const { defaults } = await load('settings');
const { default: extension } = await load('index');
const stream = {id:'effort',title:'Work',title_mode:'manual',goal:'Original goal',goal_short:'Old goal',goal_mode:'automatic',status:'active',archived:false,owner:{host_id:'host'},repositories:{},issues:[],prs:[],documents:[]};
const session = {id:'session',host_id:'host',stream_id:'effort',harness:'pi',harness_session_id:'native',title:'Lane',title_mode:'automatic',status:'active',state:'Original state',state_short:'Old state',remaining:'Next',checkout:null};
const context = () => structuredClone({catalog_source:'source',host_id:'host',stream,session,sessions:[session],authority:{host_id:'host',token:'real-live-token'}});
const response = args => ({role:'assistant',api:'openai-responses',provider:'test',model:'model',timestamp:1,stopReason:'toolUse',usage:{input:0,output:0,cacheRead:0,cacheWrite:0,totalTokens:0,cost:{input:0,output:0,cacheRead:0,cacheWrite:0,total:0}},content:[{type:'toolCall',id:'record',name:'record_card',arguments:args}]});
function fixture() {
 const dir = mkdtempSync(join(tmpdir(),'streams-prompt-lab-')),root = join(dir,'debug');
 const manager = SessionManager.inMemory(dir, {id:'native'});
 manager.appendMessage({role:'user',content:'We implemented a synthetic feature. Keep the caveat.',timestamp:1});
 manager.appendMessage({...response({}),stopReason:'stop',content:[{type:'text',text:'The synthetic feature is implemented; review remains.'}]});
 const source = join(dir,'source.jsonl'),raw = [manager.getHeader(),...manager.getEntries()].map(e=>JSON.stringify(e)).join('\n')+'\n';
 writeFileSync(source,raw);
 const inventory = {version:1,host_id:'host',streams:[stream],workspaces:[{id:'w1',label:'Work',stream_id:'effort'}],topology_error:null,target:{kind:'stream',id:'effort',title:'Work'},samples:[{host_id:'host',harness:'pi',native_session_id:'native',open:true,locations:[{workspace_id:'w1',tab_id:'t1',pane_id:'p1',terminal_id:'term1'}],catalog:context(),transcript_file:source,unavailable:null}]};
 return {dir,root,manager,source,raw,inventory,dispose:()=>rmSync(dir,{recursive:true,force:true})};
}
const model = {provider:'test',id:'model',contextWindow:100000};
function runtime(complete) {return {model,modelRegistry:{find:(provider,id)=>provider===model.provider&&id===model.id?model:undefined,complete}};}
const update = {kind:'report',force:true};
async function project({catalog,action,request}) {
 assert.equal(action,'apply');
 const target = catalog[request.kind];Object.assign(target,request.changes);
 if (request.kind==='session') catalog.sessions = catalog.sessions.map(s=>s.id===target.id&&s.host_id===target.host_id?structuredClone(target):s);
 return {catalog,receipt:{operation_id:request.operation_id,save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}},simulated:true}};
}
test('pure payload construction and strict parsing share schemas without mutating frozen inputs',()=>{
 const request = {type:'stream',catalog:context(),messages:[]},before = structuredClone(request);
 const payload = buildInferencePayload(request);
 assert.deepEqual(payload,buildInferencePayload(request));assert.deepEqual(request,before);
 assert.doesNotMatch(JSON.stringify(payload),/real-live-token|catalog_source/);
 const raw = response({changes:{title:'Work',goal_short:'Short',state:'wrong target',remaining:'wrong target',status:'active'}});
 assert.throws(()=>parseInferenceResponse(request,raw,payload),error=>{
  assert.match(String(error),/invalid structured card fields/);assert.match(String(error),/changes.*state|state.*changes/);return true;
 });
 assert.equal(raw.content[0].arguments.changes.status,'active');
 assert.throws(()=>parseInferenceResponse({type:'session',catalog:context(),messages:[]},response({changes:{session_state:'wrong alias'}})),/invalid structured/);
});
test('capture freezes cards, projected context, open locations and source bytes with private permissions',()=>{
 const f = fixture();try {
  const {capture,directory} = captureSessions(parseCaptureInventory(f.inventory),f.root,'trimmed');
  assert.deepEqual(capture.samples[0].evidence,checkpointContext(f.manager,'trimmed'));
  assert.equal(capture.samples[0].projection_source,'persisted-leaf');assert.equal(capture.samples[0].open,true);
  assert.equal(capture.samples[0].catalog.authority.token,'[redacted]');assert.equal(f.inventory.samples[0].catalog.authority.token,'real-live-token');
  assert.equal(readFileSync(f.source,'utf8'),f.raw);assert.equal(readFileSync(join(directory,capture.samples[0].snapshot_file),'utf8'),f.raw);
  assert.equal(statSync(directory).mode&0o777,0o700);assert.equal(statSync(join(directory,'capture.json')).mode&0o777,0o600);
  assert.deepEqual(captureIds(f.root),[capture.id]);assert.deepEqual(loadCapture(f.root,capture.id),capture);
  writeFileSync(f.source,'changed after capture');f.inventory.samples[0].catalog.session.state='Changed live';
  assert.equal(loadCapture(f.root,capture.id).samples[0].catalog.session.state,'Original state');
 } finally {f.dispose();}
});
test('capture uses the exact current-session leaf, preserving evidence projection rather than concatenating JSONL',()=>{
 const f=fixture();try {
  const leaf=f.manager.getLeafId();f.manager.appendMessage({role:'user',content:'Abandoned branch',timestamp:2});f.manager.branch(leaf);
  const {capture}=captureSessions(f.inventory,f.root,'full',f.manager);
  assert.equal(capture.samples[0].projection_source,'current-session');assert.equal(capture.samples[0].evidence.leafId,leaf);
  assert.doesNotMatch(JSON.stringify(capture.samples[0].evidence),/Abandoned branch/);
 }finally{f.dispose();}
});
test('capture retains unavailable foreign sessions and unknown openness rather than dropping them',()=>{
 const f=fixture();try {
  const sample=structuredClone(f.inventory.samples[0]);sample.host_id='foreign';sample.native_session_id='remote';sample.catalog.session={...session,host_id:'foreign',harness_session_id:'remote'};sample.open=null;sample.locations=[];sample.transcript_file=null;sample.unavailable='foreign host';f.inventory.samples.push(sample);
  f.inventory.topology_error='Herdr unavailable';
  const {capture}=captureSessions(f.inventory,f.root,'trimmed');
  assert.equal(capture.samples.length,2);assert.equal(capture.samples[1].unavailable,'foreign host');assert.equal(capture.samples[1].open,null);
 }finally{f.dispose();}
});
test('replay is incremental, runs the shared session-to-stream workflow, and never rereads source transcripts',async()=>{
 const f=fixture();try {
  const {capture}=captureSessions(f.inventory,f.root,'trimmed');writeFileSync(f.source,'unreadable now');
  const requests=[];
  const rt=runtime(async(_model,payload)=>{const task=JSON.parse(payload.messages.at(-1).content);requests.push(task);return response({changes:task.task==='session'?{state:'Updated state'}:{goal:'Updated goal'}});});
  const {run,directory}=await replayCapture(capture,f.root,rt,model,update,project,new AbortController().signal);
  assert.equal(requests[0].catalog.session.state,'Original state');assert.equal(requests[0].reset,undefined);
  assert.equal(requests[1].catalog.session.state,'Updated state');assert.equal(run.samples[0].after.stream.goal,'Updated goal');
  assert.equal(capture.samples[0].catalog.session.state,'Original state');assert.equal(run.samples[0].status,'completed');
  assert.equal(run.samples[0].tasks.length,2);assert.ok(run.samples[0].tasks.every(t=>t.helper_file&&t.prompt_hash));
  assert.match(readFileSync(join(directory,'comparison.md'),'utf8'),/incremental|simulated/);
  const log=readFileSync(join(directory,run.samples[0].tasks[0].helper_file),'utf8');assert.match(log,/stream-helper-result/);assert.match(log,/Original state/);
 }finally{f.dispose();}
});
test('reset --auto unlocks only the sandbox scope and preserves captured manual modes',async()=>{
 const f=fixture();try {
  f.inventory.samples[0].catalog.session.title_mode='manual';
  const {capture}=captureSessions(f.inventory,f.root,'trimmed'),requests=[];
  const rt=runtime(async(_model,payload)=>{requests.push(JSON.parse(payload.messages.at(-1).content));return response({changes:{title:'Fresh',state:'Regenerated'}});});
  const {run}=await replayCapture(capture,f.root,rt,model,{kind:'reset',target:'session',auto:true},project,new AbortController().signal);
  assert.equal(requests.length,1);assert.match(requests[0].reset,/Infer all allowed/);assert.equal(requests[0].catalog.session.title,'');
  assert.equal(run.samples[0].after.session.title_mode,'automatic');assert.equal(capture.samples[0].catalog.session.title_mode,'manual');
  assert.equal(run.samples[0].after.session.state_short,null);
 }finally{f.dispose();}
});
test('per-task failures preserve earlier simulated saves, raw cross-target args and schema diagnostics',async()=>{
 const f=fixture();try {
  const {capture}=captureSessions(f.inventory,f.root,'trimmed');let calls=0;
  const rt=runtime(async()=>++calls===1?response({changes:{state:'Accepted session update'}}):response({changes:{goal:'Goal',state:'Session-only field',session_status:'bad alias'}}));
  const {run,directory}=await replayCapture(capture,f.root,rt,model,update,project,new AbortController().signal);
  const result=run.samples[0];assert.equal(result.status,'failed');assert.equal(result.after.session.state,'Accepted session update');assert.equal(result.after.stream.goal,'Original goal');
  assert.equal(result.tasks[0].status,'parsed');assert.equal(result.tasks[1].status,'failed');assert.match(result.tasks[1].error,/state|session_status/);
  const log=readFileSync(join(directory,result.tasks[1].helper_file),'utf8');assert.match(log,/Session-only field/);assert.match(log,/stream-helper-error/);
 }finally{f.dispose();}
});
test('reset on an uncarded sample is skipped with the refusal notice, not reported completed',async()=>{
 const f=fixture();try {
  Object.assign(f.inventory.samples[0].catalog,{session:null,sessions:[]});
  const {capture}=captureSessions(f.inventory,f.root,'trimmed');
  const {run,directory}=await replayCapture(capture,f.root,runtime(()=>assert.fail('no model')),model,{kind:'reset',target:'all',auto:false},()=>assert.fail('no preview'),new AbortController().signal);
  assert.equal(run.samples[0].status,'skipped');assert.deepEqual(run.samples[0].tasks,[]);
  assert.match(readFileSync(join(directory,'comparison.md'),'utf8'),/skipped[\s\S]*Reset requires an attached work card/);
 }finally{f.dispose();}
});
test('stream resets with only protected manual fields explicitly skip inference',async()=>{
 for(const auto of [false,true]) {
  const f=fixture();try {
   f.inventory.samples[0].catalog.stream.goal_mode='manual';
   if(auto)f.inventory.samples[0].catalog.authority=null;
   const {capture}=captureSessions(f.inventory,f.root,'trimmed');
   const {run}=await replayCapture(capture,f.root,runtime(()=>assert.fail('no model')),model,{kind:'reset',target:'stream',auto},()=>assert.fail('no preview'),new AbortController().signal);
   assert.equal(run.samples[0].status,'skipped');assert.deepEqual(run.samples[0].tasks,[]);
   assert.match(run.samples[0].notices[0],/No helper ran/);
  }finally{f.dispose();}
 }
});
test('valid empty helper deltas still complete even when no simulated save is needed',async()=>{
 const f=fixture();try {
  const {capture}=captureSessions(f.inventory,f.root,'trimmed');
  const {run}=await replayCapture(capture,f.root,runtime(async()=>response({changes:{}})),model,update,()=>assert.fail('no preview'),new AbortController().signal);
  assert.equal(run.samples[0].status,'completed');assert.equal(run.samples[0].tasks.length,2);assert.deepEqual(run.samples[0].receipts,[]);
 }finally{f.dispose();}
});
test('refused simulated saves fail the sample and retain earlier accepted updates',async()=>{
 for(const failAt of [1,2]) {
  const f=fixture();try {
   const {capture}=captureSessions(f.inventory,f.root,'trimmed');let previews=0;
   const rt=runtime(async(_model,payload)=>response({changes:JSON.parse(payload.messages.at(-1).content).task==='session'?{state:'Accepted update'}:{goal:'Refused goal'}}));
   const {run}=await replayCapture(capture,f.root,rt,model,update,async input=>++previews===failAt?{catalog:input.catalog,receipt:{operation_id:input.request.operation_id,save:{local:'unchanged',sharing:'local-only',domain:{outcome:'conflict'},detail:'Synthetic refusal'},simulated:true}}:project(input),new AbortController().signal);
   const sample=run.samples[0];assert.equal(sample.status,'failed');assert.match(sample.error,/Simulated save refused.*conflict/);
   assert.equal(sample.tasks.length,failAt);assert.equal(sample.receipts.length,failAt);
   assert.equal(sample.after.session.state,failAt===2?'Accepted update':'Original state');assert.equal(sample.after.stream.goal,'Original goal');
  }finally{f.dispose();}
 }
});
test('capture listing ignores interrupted directories without hiding completed captures',()=>{
 const f=fixture();try {
  const {capture}=captureSessions(f.inventory,f.root,'trimmed');
  mkdirSync(join(f.root,'captures','00000000-0000-0000-0000-000000000001'));
  assert.deepEqual(captureIds(f.root),[capture.id]);
 }finally{f.dispose();}
});
test('the current in-memory session can be captured even without a saved transcript',()=>{
 const f=fixture();try {
  Object.assign(f.inventory.samples[0],{transcript_file:null,unavailable:'No saved transcript'});
  const {capture}=captureSessions(f.inventory,f.root,'trimmed',f.manager);
  assert.equal(capture.samples[0].unavailable,null);assert.ok(capture.samples[0].evidence);
 }finally{f.dispose();}
});
test('raw linked helper logs are copied privately after identity and parent verification',async()=>{
 const f=fixture();try {
  const ctx={cwd:f.dir,sessionManager:f.manager,...runtime(async()=>response({changes:{}}))};
  await infer(ctx,{type:'session',catalog:context(),messages:[]},new AbortController().signal,undefined,{directory:join(f.dir,'original-helpers'),onCreate:link=>f.manager.appendCustomEntry('stream-helper-session',link)});
  f.manager.appendCustomEntry('stream-helper-session',{session_id:'missing',session_file:join(f.dir,'missing.jsonl')});
  const {capture,directory}=captureSessions(f.inventory,f.root,'trimmed',f.manager),logs=capture.samples[0].helper_logs;
  assert.equal(logs.length,2);assert.match(readFileSync(join(directory,logs[0].file),'utf8'),/stream-helper-request/);
  assert.equal(statSync(join(directory,logs[0].file)).mode&0o777,0o600);assert.match(logs[1].error,/ENOENT/);
 }finally{f.dispose();}
});
test('debug storage refuses version-controlled agent directories before copying any content',async()=>{
 const f=fixture();try {
  const repo=join(f.dir,'repository');mkdirSync(join(repo,'.git'),{recursive:true});
  const root=join(repo,'.pi','stream-debug');
  assert.throws(()=>captureSessions(f.inventory,root,'trimmed'),/outside version control/);assert.equal(existsSync(root),false);
  const {capture}=captureSessions(f.inventory,f.root,'trimmed');
  await assert.rejects(replayCapture(capture,root,runtime(()=>assert.fail('no model')),model,update,project,new AbortController().signal),/outside version control/);
  assert.equal(existsSync(root),false);
 }finally{f.dispose();}
});
test('storage rejects dangling links and symlinks into VCS before copying any content',()=>{
 const f=fixture();try {
  const repo=join(f.dir,'repository');mkdirSync(join(repo,'.git'),{recursive:true});
  const alias=join(f.dir,'alias'),dangling=join(f.dir,'dangling');
  symlinkSync(repo,alias);symlinkSync(join(repo,'future'),dangling);
  assert.throws(()=>captureSessions(f.inventory,join(alias,'debug'),'trimmed'),/outside version control/);
  assert.throws(()=>captureSessions(f.inventory,join(dangling,'debug'),'trimmed'),/dangling symbolic link/);
  assert.equal(existsSync(join(repo,'debug')),false);assert.equal(existsSync(join(repo,'future')),false);
 }finally{f.dispose();}
});
test('capture reads reject linked storage and manifests without modifying snapshots',()=>{
 const f=fixture();try {
  const {capture,directory}=captureSessions(f.inventory,f.root,'trimmed');
  const alias=join(f.dir,'alias');mkdirSync(alias);symlinkSync(join(f.root,'captures'),join(alias,'captures'));
  assert.throws(()=>loadCapture(alias,capture.id),/symbolic link/);assert.throws(()=>captureIds(alias),/symbolic link/);
  const manifest=join(directory,'capture.json'),saved=join(directory,'original.json');renameSync(manifest,saved);symlinkSync(saved,manifest);
  assert.throws(()=>loadCapture(f.root,capture.id),/regular file/);assert.deepEqual(JSON.parse(readFileSync(saved,'utf8')),capture);
  const moved=join(f.dir,'original-capture');renameSync(directory,moved);symlinkSync(moved,directory);
  assert.throws(()=>loadCapture(f.root,capture.id),/symbolic link/);
 }finally{f.dispose();}
});
test('loading a capture is read-only and enforces the VCS boundary as well as writing',()=>{
 const f=fixture();try {
  const {capture,directory}=captureSessions(f.inventory,f.root,'trimmed'),manifest=join(directory,'capture.json');
  const before=statSync(manifest);loadCapture(f.root,capture.id);
  assert.equal(statSync(manifest).mtimeMs,before.mtimeMs);assert.equal(statSync(manifest).mode,before.mode);
  mkdirSync(join(f.root,'.git'));assert.throws(()=>loadCapture(f.root,capture.id),/outside version control/);
  assert.equal(readFileSync(manifest,'utf8'),JSON.stringify(capture,null,2)+'\n');
  const missing=join(f.dir,'not-created');assert.throws(()=>loadCapture(missing,capture.id),/ENOENT/);assert.equal(existsSync(missing),false);
 }finally{f.dispose();}
});
test('loaded captures reject malformed samples and incomplete or unsafe evidence metadata at the boundary',()=>{
 const f=fixture();try {
  const {capture,directory}=captureSessions(f.inventory,f.root,'trimmed');
  const malformed=[value=>value.samples=null,value=>value.samples=[null],value=>value.samples=[{}],value=>value.samples[0].catalog=null,
   value=>delete value.samples[0].cwd,value=>value.samples[0].cwd='',value=>delete value.samples[0].snapshot_file,
   value=>value.samples[0].snapshot_file='../outside.jsonl',value=>value.samples[0].snapshot_file='/outside.jsonl',value=>value.samples[0].evidence.messages=null];
  for(const alter of malformed) {
   const value=structuredClone(capture);alter(value);writeFileSync(join(directory,'capture.json'),JSON.stringify(value));
   assert.throws(()=>loadCapture(f.root,capture.id),error=>!(error instanceof TypeError)&&/Invalid|frozen/i.test(String(error)));
  }
 }finally{f.dispose();}
});
test('real preview subprocess applies live derivative-clearing and reconciliation rules only to supplied cards',async()=>{
 const f=fixture();try {
  const {capture}=captureSessions(f.inventory,f.root,'trimmed'),client=new StreamCli(f.dir);
  const rt=runtime(async(_model,payload)=>response({changes:JSON.parse(payload.messages.at(-1).content).task==='session'?{remaining:'Finish tests'}:{goal:'Updated goal'}}));
  const {run}=await replayCapture(capture,f.root,rt,model,update,(input,signal)=>client.preview(input,signal),new AbortController().signal);
  const result=run.samples[0];assert.equal(result.status,'completed',result.error);
  assert.equal(result.after.session.remaining,'Finish tests');assert.equal(result.after.session.state_short,null);
  assert.equal(result.after.stream.goal_short,null);assert.equal(result.after.stream.title_mode,'manual');
  assert.ok(result.receipts.every(receipt=>receipt.simulated&&receipt.save.local==='unchanged'));
  assert.equal(readFileSync(f.source,'utf8'),f.raw);
 }finally{f.dispose();}
});
test('real enrollment preview feeds later helpers with deterministic private working-copy identities',async()=>{
 const f=fixture();try {
  Object.assign(f.inventory.samples[0].catalog,{stream:null,session:null,sessions:[],authority:null});
  const {capture}=captureSessions(f.inventory,f.root,'trimmed'),client=new StreamCli(f.dir);
  const rt=runtime(async(_model,payload)=>{
   const request=JSON.parse(payload.messages.at(-1).content);
   return response(request.task==='enroll'?{meaningful:true,stream:{title:'Synthetic effort',goal:'Finish work'},session:{title:'Contribution'}}:{changes:{}});
  });
  const execute=()=>replayCapture(capture,f.root,rt,model,update,(input,signal)=>client.preview(input,signal),new AbortController().signal);
  const first=(await execute()).run.samples[0],second=(await execute()).run.samples[0];
  assert.equal(first.status,'completed',first.error);assert.deepEqual(first.tasks.map(t=>t.task),['enroll','session','stream']);
  assert.deepEqual(first.after,second.after);assert.equal(capture.samples[0].catalog.stream,null);assert.equal(first.after.session.harness_session_id,'native');
  assert.ok(first.receipts[0].saves.every(save=>save.save.simulated));
 }finally{f.dispose();}
});
test('cancellation retains the failed task artifact and explicitly leaves later samples unrun',async()=>{
 const f=fixture();try {
  const {capture}=captureSessions(f.inventory,f.root,'trimmed');capture.samples.push(structuredClone(capture.samples[0]));
  const controller=new AbortController();let calls=0;
  const rt=runtime(async()=>{calls++;controller.abort();return response({changes:{state:'Must not apply'}});});
  const {run,directory}=await replayCapture(capture,f.root,rt,model,update,()=>assert.fail('no projection after abort'),controller.signal);
  assert.equal(calls,1);assert.equal(run.samples[0].status,'cancelled');assert.equal(run.samples[0].tasks[0].status,'failed');assert.equal(run.samples[1].status,'not-run');
  assert.equal(run.samples[0].after.session.state,'Original state');assert.ok(existsSync(join(directory,'run.json')));
 }finally{f.dispose();}
});
test('debug argument grammar keeps incremental default and scoped reset/--auto semantics',()=>{
 assert.deepEqual(parseDebugCommand('capture workspace w1').target,{kind:'workspace',id:'w1'});
 assert.deepEqual(parseDebugCommand('replay').action,update);
 const parsed=parseDebugCommand('replay capture-id --model test/family/model --reset session --auto');
 assert.deepEqual(parsed.model,{provider:'test',id:'family/model'});assert.deepEqual(parsed.action,{kind:'reset',target:'session',auto:true});
 for(const args of ['replay --auto','replay --model missing-slash','replay --reset --reset','capture unknown x','replay id extra','replay --model x/y --model x/y']) assert.throws(()=>parseDebugCommand(args));
});
test('enabled Pi commands capture both cards and evidence, then replay reset through the real preview adapter',async()=>{
 const f=fixture();try {
  let command,calls=0;const notices=[],statuses=[],dialogs=[],before=f.manager.getEntries();
  f.inventory.samples[0].catalog.session.title_mode='manual';
  const rt=runtime(async()=>{calls++;return response({changes:{title:'Regenerated',state:'Verified synthetic result'}});});
  rt.modelRegistry.getAvailable=()=>[model];
  const ctx={cwd:f.dir,hasUI:true,sessionManager:f.manager,...rt,ui:{
   notify:text=>notices.push(text),setStatus:(...args)=>statuses.push(args),
   confirm:async(title,message,options)=>{assert.ok(options.signal);dialogs.push([title,message]);return true;},
   select:async(_title,labels,options)=>{assert.ok(options.signal);return labels[0];},
  }};
  const real=new StreamCli(f.dir),client={captureInputs:async target=>{assert.deepEqual(target,{kind:'stream',id:'effort'});return f.inventory;},preview:(input,signal)=>real.preview(input,signal)};
  extension({registerCommand:(_name,value)=>{command=value;},registerTool(){},on(){},appendEntry:()=>assert.fail('debug must not append to real history')},{agentDir:f.dir,cli:client,settings:{...defaults,debug_commands:true}});
  assert.equal(command.getArgumentCompletions('debug')[0].value,'debug');
  await command.handler('debug capture stream effort',ctx);assert.equal(calls,0);
  const root=join(f.dir,'stream-debug'),id=captureIds(root)[0];assert.ok(id,notices.join('\n'));
  const snapshot=loadCapture(root,id);assert.ok(snapshot.samples[0].evidence);assert.equal(snapshot.samples[0].catalog.session.title,'Lane');
  await command.handler(`debug replay ${id} --reset session --auto`,ctx);
  assert.equal(calls,1,notices.join('\n'));
  const comparison=notices.at(-1).split('Comparison: ')[1],run=JSON.parse(readFileSync(comparison.replace(/comparison\.md$/,'run.json'),'utf8'));
  assert.equal(run.samples[0].status,'completed',run.samples[0].error);assert.equal(run.samples[0].after.session.title,'Regenerated');
  assert.equal(loadCapture(root,id).samples[0].catalog.session.title_mode,'manual');assert.deepEqual(f.manager.getEntries(),before);
  assert.equal(readFileSync(f.source,'utf8'),f.raw);assert.ok(dialogs[0][1].includes('Lane · native · open'));
  assert.deepEqual(statuses.at(-1),['stream-debug',undefined]);
 }finally{f.dispose();}
});
test('disabled commands cannot read inventory, create files, append history or call models',async()=>{
 const f=fixture();try {
  const ctx={cwd:f.dir,hasUI:true,sessionManager:f.manager,ui:{notify:()=>assert.fail('no notification before gate')},...runtime(()=>assert.fail('no model'))};
  const client=new Proxy({},{get:()=>assert.fail('no CLI capability before debug gate')});
  await assert.rejects(debugCommand('capture',ctx,client,f.dir,defaults,new AbortController().signal),/Debug commands are disabled/);
  assert.equal(existsSync(join(f.dir,'stream-debug')),false);
  let command;const notices=[];
  extension({registerCommand:(_name,value)=>{command=value;},registerTool(){},on(){},appendEntry:()=>assert.fail('no parent history writes')},{agentDir:f.dir,cli:client});
  ctx.ui.notify=text=>notices.push(text);
  assert.equal(command.getArgumentCompletions('debug'),null);await command.handler('debug capture',ctx);
  assert.match(notices[0],/Debug commands are disabled/);
 }finally{f.dispose();}
});
test('unavailable replay model fails before inference or output creation',async()=>{
 const f=fixture();try {
  const {capture}=captureSessions(f.inventory,f.root,'trimmed');
  await assert.rejects(replayCapture(capture,f.root,runtime(()=>assert.fail('no call')),{provider:'missing',id:'none'},update,project,new AbortController().signal),/unavailable/);
 }finally{f.dispose();}
});
