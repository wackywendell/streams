import assert from 'node:assert/strict';
import test from 'node:test';
import {sessionId} from './helpers/session-id.mjs';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createJiti } from 'jiti';
import { childCli } from './helpers/stream-cli.mjs';
import { SessionManager } from '@earendil-works/pi-coding-agent';
const jiti = createJiti(import.meta.url);
const load = name => jiti.import(new URL(`../extensions/stream-checkpoint/${name}.ts`, import.meta.url).pathname);
const { infer, buildInferencePayload, parseInferenceResponse, runPreparedInference } = await load('agent');
const { captureSessions, loadCapture, parseCaptureInventory, captureIds } = await load('capture');
const { replayCapture } = await load('replay');
const { debugCommand, parseDebugCommand } = await load('debug');
const { StreamCli } = await load('streams');
const { defaults } = await load('settings');
const { default: extension } = await load('index');
const stream = {id:'effort',title:'Work',title_mode:'manual',goal:'Original goal',goal_short:'Old goal',goal_mode:'automatic',status:'active',archived:false,owner:{host_id:'host'},repositories:{},issues:[],prs:[],documents:[]};
const session = {id:sessionId('host','pi','native'),host_id:'host',stream_id:'effort',harness:'pi',harness_session_id:'native',title:'Lane',title_mode:'automatic',status:'active',state:'Original state',state_short:'Old state',remaining:'Next',checkout:null};
const context = () => structuredClone({catalog_source:'source',host_id:'host',stream,session,sessions:[session],title_defaults:{stream:'automatic',session:'automatic'}});
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
test('stream preparation uses only the addressed contribution without claiming a complete roster',()=>{
 const catalog=context(),sibling={...session,id:'sibling',title:'Other lane',state:'Independent contribution'};
 catalog.sessions=[{...catalog.session,state:'Stale target'},sibling];catalog.session.state='Latest target';
 const request={type:'stream',catalog,messages:[{role:'user',content:'Rename this effort to my local task',timestamp:1}]},before=structuredClone(request);
 const payload=buildInferencePayload(request),sent=JSON.parse(payload.messages.at(-1).content);
 assert.equal(payload.messages.length,1);assert.deepEqual(Object.keys(sent.catalog),['stream','sessions']);
 assert.deepEqual(sent.catalog.sessions,[catalog.session]);assert.deepEqual(sent.catalog.stream,catalog.stream);
 assert.doesNotMatch(JSON.stringify(payload),/Rename this effort to my local task|Stale target/);
 assert.deepEqual(request,before);
 catalog.sessions=[sibling];
 assert.deepEqual(JSON.parse(buildInferencePayload(request).messages[0].content).catalog.sessions,[catalog.session]);
});
test('report prompts name one write target and its exact automatic fields',()=>{
 for(const [kind,label] of [['stream','Stream'],['session','Session']]) for(const manual of [false,true]) {
  const catalog=context();
  if(manual) {catalog[kind].title_mode='manual';if(kind==='stream')catalog.stream.goal_mode='manual';}
  const payload=buildInferencePayload({type:kind,catalog,messages:[]});
  const allowed=Object.keys(payload.tools[0].parameters.properties.changes.properties);
  assert.ok(payload.systemPrompt.includes(`Update only the ${label} card.`));
  assert.ok(payload.systemPrompt.includes(`Allowed fields in changes: ${JSON.stringify(allowed)}.`));
  assert.match(payload.systemPrompt,/other catalog records are read-only evidence/);
  assert.match(payload.systemPrompt,/Never use stream_ or session_ prefixes/);
  if(manual)assert.ok(!allowed.includes('title'));
 }
});
test('report prompts prefer current tasks, enduring responsibilities and actionable next steps',()=>{
 const catalog=context(),before=structuredClone(catalog);
 const sessionPrompt=buildInferencePayload({type:'session',catalog,messages:[]}).systemPrompt;
 assert.match(sessionPrompt,/newest task the session has accepted or started/);
 assert.match(sessionPrompt,/late review notification about past work does not finish newer work/);
 assert.match(sessionPrompt,/Do not confuse a request or a plan with work already completed/);
 assert.match(sessionPrompt,/continuing responsibility, not its latest task/);
 assert.match(sessionPrompt,/Keep recognizable titles that already fit/);
 assert.match(sessionPrompt,/what this Session is for and its main result/);
 assert.match(sessionPrompt,/one short sentence of roughly 12 words/);
 assert.match(sessionPrompt,/Do not put instructions or next-action lists here/);
 assert.match(sessionPrompt,/never "This session"/);
 assert.match(sessionPrompt,/shorten an overlong recap even if its facts have not changed/);
 assert.match(sessionPrompt,/concrete current position of the work/);
 assert.match(sessionPrompt,/not first-person narration/);
 assert.match(sessionPrompt,/Produce recap_short, state_short and remaining_short alongside their full text/);
 assert.match(sessionPrompt,/prefer 1–2 words, shorter is better/);
 assert.match(sessionPrompt,/preferably fewer than 20 characters, always fewer than 25/);
 assert.match(sessionPrompt,/Remaining: ONE concrete next action or decision not already stated in Status/);
 assert.match(sessionPrompt,/empty string if Status already covers the next step/);
 assert.match(sessionPrompt,/Empty Remaining does not mean the Session is done/);
 assert.match(sessionPrompt,/roughly 12 words/);
 assert.match(sessionPrompt,/Name the action and its target/);
 assert.match(sessionPrompt,/done only when no newer accepted task remains/);
 assert.match(sessionPrompt,/Keep genuine uncertainty and blockers/);
 assert.ok(sessionPrompt.indexOf('Final editing rules')>sessionPrompt.indexOf('Update only the Session card'));
 const streamPrompt=buildInferencePayload({type:'stream',catalog,messages:[]}).systemPrompt;
 assert.match(streamPrompt,/Prefer the established project name/);
 assert.match(streamPrompt,/Keep a fitting title when only progress or wording changes/);
 assert.match(streamPrompt,/Keep a fitting goal unchanged when a Session starts, finishes or changes topic/);
 assert.match(streamPrompt,/Do not append that Session's subject, feature, milestone or verification work/);
 assert.match(streamPrompt,/do not reconstruct it from a Session's topic/);
 assert.doesNotMatch(streamPrompt,/Final editing rules|Remaining names/);
 assert.deepEqual(catalog,before);
 assert.deepEqual(parseInferenceResponse({type:'session',catalog,messages:[]},response({changes:{}})),{changes:{}});
});
test('Session text schema describes distinct rows without changing constraints or empty-next-step semantics',()=>{
 const request={type:'session',catalog:context(),messages:[]},payload=buildInferencePayload(request);
 const fields=payload.tools[0].parameters.properties.changes.properties;
 assert.match(fields.recap.description,/purpose and main result.*roughly 12 words/);
 assert.match(fields.state.description,/done or underway.*immediate wait or blocker/);
 assert.match(fields.remaining.description,/not already stated in Status/);
 for(const name of ['recap_short','state_short','remaining_short']) {
  assert.equal(fields[name].anyOf[0].maxLength,24);
  assert.match(fields[name].description,/at most 24 characters.*Null/);
 }
 const streamFields=buildInferencePayload({type:'stream',catalog:context(),messages:[]}).tools[0].parameters.properties.changes.properties;
 assert.match(streamFields.goal.description,/whole effort's enduring user purpose/);
 const changes={status:'waiting',remaining:'',remaining_short:null};
 assert.deepEqual(parseInferenceResponse(request,response({changes}),payload),{changes});
});
test('a done card retains later task evidence and can report resumed work',()=>{
 const catalog=context();catalog.session.status='done';catalog.session.state='Proof wiring integrated';catalog.session.remaining='';
 const request={type:'session',catalog,messages:[
  {...response({}),stopReason:'stop',content:[{type:'text',text:'Proof wiring is finished.'}]},
  {role:'user',content:'Resume the catalog switch-over and agree the move-handler API.',timestamp:2},
 ]},before=structuredClone(request);
 const payload=buildInferencePayload(request),sent=JSON.stringify(payload.messages);
 assert.ok(sent.indexOf('Proof wiring is finished.')<sent.indexOf('Resume the catalog switch-over'));
 const changes={status:'active',state:'Catalog activation boundary pending',remaining:'Agree the move-handler inputs before wiring catalog activation'};
 assert.deepEqual(parseInferenceResponse(request,response({changes}),payload),{changes});
 assert.deepEqual(request,before);
});
const proseResponses=JSON.parse(readFileSync(new URL('./fixtures/reporting-prose-responses.json',import.meta.url),'utf8'));
for(const fixture of proseResponses) test(`prose remains invalid and Responses inference forces record_card: ${fixture.name}`,async()=>{
 const request={type:'session',catalog:context(),messages:[]},payload=buildInferencePayload(request);
 const prose={...response({}),stopReason:'stop',content:[{type:'text',text:fixture.text}]};
 assert.throws(()=>parseInferenceResponse(request,prose,payload),/exactly one structured record_card call/);
 const signal=new AbortController().signal,selected={...model,api:'openai-responses'};
 const rt={model:selected,modelRegistry:{find:()=>selected,complete:async(actual,sent,options)=>{
  assert.equal(actual,selected);assert.deepEqual(sent,payload);
  assert.deepEqual(options,{signal,toolChoice:{type:'function',name:'record_card'}});
  return response({changes:{}});
 }}};
 assert.deepEqual(await runPreparedInference(rt,{input:request,model_request:payload},signal),{changes:{}});
 rt.modelRegistry.complete=async()=>prose;
 await assert.rejects(runPreparedInference(rt,{input:request,model_request:payload},signal),/exactly one structured record_card call/);
});
test('other APIs use the explicit report contract without assuming Responses tool-choice support',async()=>{
 const request={type:'session',catalog:context(),messages:[]},payload=buildInferencePayload(request),signal=new AbortController().signal;
 assert.match(payload.systemPrompt,/Do not answer its questions, address its user, or offer to do its work/);
 const rt=runtime(async(_model,_payload,options)=>{assert.deepEqual(options,{signal});return response({changes:{}});});
 assert.deepEqual(await runPreparedInference(rt,{input:request,model_request:payload},signal),{changes:{}});
});
const targetConfusion=JSON.parse(readFileSync(new URL('./fixtures/reporting-target-confusion.json',import.meta.url),'utf8'));
for(const fixture of targetConfusion) test(`captured failure remains rejected: ${fixture.name}`,()=>{
 const catalog=context();catalog.stream.title_mode='automatic';
 const request={type:'stream',catalog,messages:[]},raw=response(fixture.arguments),before=structuredClone({request,raw});
 const payload=buildInferencePayload(request);
 assert.equal(payload.tools[0].parameters.properties.changes.additionalProperties,false);
 assert.throws(()=>parseInferenceResponse(request,raw,payload),error=>{
  assert.match(String(error),/invalid structured card fields; nothing was saved/);
  for(const field of fixture.rejected_fields)assert.ok(String(error).includes(field),String(error));
  return true;
 });
 assert.deepEqual({request,raw},before);
 assert.deepEqual(parseInferenceResponse(request,response({changes:{goal:'Keep reports scoped to their target.'}}),payload),{changes:{goal:'Keep reports scoped to their target.'}});
});
test('capture freezes cards, projected context, open locations and source bytes with private permissions',()=>{
 const f = fixture();try {
  const {capture,directory} = captureSessions(parseCaptureInventory(f.inventory),f.root);
  assert.equal(capture.context_mode,'full');
  assert.deepEqual(capture.samples[0].evidence,{messages:f.manager.buildSessionProjection().messages,leafId:f.manager.getLeafId()});
  assert.equal(capture.samples[0].projection_source,'persisted-leaf');assert.equal(capture.samples[0].open,true);
  assert.equal(Object.hasOwn(capture.samples[0].catalog,'authority'),false);assert.deepEqual(capture.samples[0].catalog.title_defaults,{stream:'automatic',session:'automatic'});
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
  const {capture}=captureSessions(f.inventory,f.root,f.manager);
  assert.equal(capture.samples[0].projection_source,'current-session');assert.equal(capture.samples[0].evidence.leafId,leaf);
  assert.doesNotMatch(JSON.stringify(capture.samples[0].evidence),/Abandoned branch/);
 }finally{f.dispose();}
});
test('capture retains unavailable foreign sessions and unknown openness rather than dropping them',()=>{
 const f=fixture();try {
  const sample=structuredClone(f.inventory.samples[0]);sample.host_id='foreign';sample.native_session_id='remote';sample.catalog.session={...session,id:sessionId('foreign','pi','remote'),host_id:'foreign',harness_session_id:'remote'};sample.catalog.sessions=[sample.catalog.session];sample.catalog.stream.owner.host_id='foreign';sample.open=null;sample.locations=[];sample.transcript_file=null;sample.unavailable='foreign host';f.inventory.samples.push(sample);
  f.inventory.topology_error='Herdr unavailable';
  const {capture}=captureSessions(f.inventory,f.root);
  assert.equal(capture.samples.length,2);assert.equal(capture.samples[1].unavailable,'foreign host');assert.equal(capture.samples[1].open,null);
 }finally{f.dispose();}
});
test('replay is incremental, runs the shared session-to-stream workflow, and never rereads source transcripts',async()=>{
 const f=fixture();try {
  const {capture}=captureSessions(f.inventory,f.root);writeFileSync(f.source,'unreadable now');
  const requests=[];
  const rt=runtime(async(_model,payload)=>{const task=JSON.parse(payload.messages.at(-1).content);requests.push(task);return response({changes:task.task==='session'?{state:'Updated state'}:{goal:'Updated goal'}});});
  const {run,directory}=await replayCapture(capture,f.root,rt,model,update,project,new AbortController().signal);
  assert.equal(requests[0].catalog.session.state,'Original state');assert.equal(requests[0].reset,undefined);
  assert.equal(requests[1].catalog.sessions[0].state,'Updated state');assert.equal(run.samples[0].after.stream.goal,'Updated goal');
  assert.equal(capture.samples[0].catalog.session.state,'Original state');assert.equal(run.samples[0].status,'completed');
  assert.equal(run.samples[0].tasks.length,2);assert.ok(run.samples[0].tasks.every(t=>t.helper_file&&t.prompt_hash));
  assert.match(readFileSync(join(directory,'comparison.md'),'utf8'),/incremental|simulated/);
  const log=readFileSync(join(directory,run.samples[0].tasks[0].helper_file),'utf8');assert.match(log,/stream-helper-result/);assert.match(log,/Original state/);
 }finally{f.dispose();}
});
test('reset --auto unlocks only the sandbox scope and preserves captured manual modes',async()=>{
 const f=fixture();try {
  f.inventory.samples[0].catalog.session.title_mode='manual';
  const {capture}=captureSessions(f.inventory,f.root),requests=[];
  const rt=runtime(async(_model,payload)=>{requests.push(JSON.parse(payload.messages.at(-1).content));return response({changes:{title:'Fresh',state:'Regenerated'}});});
  const {run}=await replayCapture(capture,f.root,rt,model,{kind:'reset',target:'session',auto:true},project,new AbortController().signal);
  assert.equal(requests.length,1);assert.match(requests[0].reset,/Infer all allowed/);assert.equal(Object.hasOwn(requests[0].catalog.session,'title'),false);
  assert.equal(run.samples[0].after.session.title_mode,'automatic');assert.equal(capture.samples[0].catalog.session.title_mode,'manual');
  assert.equal(run.samples[0].after.session.state_short,null);
 }finally{f.dispose();}
});
test('per-task failures preserve earlier simulated saves, raw cross-target args and schema diagnostics',async()=>{
 const f=fixture();try {
  const {capture}=captureSessions(f.inventory,f.root);let calls=0;
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
  const {capture}=captureSessions(f.inventory,f.root);
  const {run,directory}=await replayCapture(capture,f.root,runtime(()=>assert.fail('no model')),model,{kind:'reset',target:'all',auto:false},()=>assert.fail('no preview'),new AbortController().signal);
  assert.equal(run.samples[0].status,'skipped');assert.deepEqual(run.samples[0].tasks,[]);
  assert.match(readFileSync(join(directory,'comparison.md'),'utf8'),/skipped[\s\S]*Reset requires an attached work card/);
 }finally{f.dispose();}
});
test('stream reset without auto preserves protected manual fields and explicitly skips inference',async()=>{
 const f=fixture();try {
  f.inventory.samples[0].catalog.stream.goal_mode='manual';
  const {capture}=captureSessions(f.inventory,f.root);
  const {run}=await replayCapture(capture,f.root,runtime(()=>assert.fail('no model')),model,{kind:'reset',target:'stream',auto:false},()=>assert.fail('no preview'),new AbortController().signal);
  assert.equal(run.samples[0].status,'skipped');assert.deepEqual(run.samples[0].tasks,[]);
  assert.match(run.samples[0].notices[0],/No helper ran/);
 }finally{f.dispose();}
});
test('valid empty helper deltas still complete even when no simulated save is needed',async()=>{
 const f=fixture();try {
  const {capture}=captureSessions(f.inventory,f.root);
  const {run}=await replayCapture(capture,f.root,runtime(async()=>response({changes:{}})),model,update,()=>assert.fail('no preview'),new AbortController().signal);
  assert.equal(run.samples[0].status,'completed');assert.equal(run.samples[0].tasks.length,2);assert.deepEqual(run.samples[0].receipts,[]);
 }finally{f.dispose();}
});
test('refused simulated saves fail the sample and retain earlier accepted updates',async()=>{
 for(const failAt of [1,2]) {
  const f=fixture();try {
   const {capture}=captureSessions(f.inventory,f.root);let previews=0;
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
  const {capture}=captureSessions(f.inventory,f.root);
  mkdirSync(join(f.root,'captures','00000000-0000-0000-0000-000000000001'));
  assert.deepEqual(captureIds(f.root),[capture.id]);
 }finally{f.dispose();}
});
test('the current in-memory session can be captured even without a saved transcript',()=>{
 const f=fixture();try {
  Object.assign(f.inventory.samples[0],{transcript_file:null,unavailable:'No saved transcript'});
  const {capture}=captureSessions(f.inventory,f.root,f.manager);
  assert.equal(capture.samples[0].unavailable,null);assert.ok(capture.samples[0].evidence);
 }finally{f.dispose();}
});
test('raw linked helper logs are copied privately after identity and parent verification',async()=>{
 const f=fixture();try {
  const ctx={cwd:f.dir,sessionManager:f.manager,...runtime(async()=>response({changes:{}}))};
  await infer(ctx,{type:'session',catalog:context(),messages:[]},new AbortController().signal,undefined,{directory:join(f.dir,'original-helpers'),onCreate:link=>f.manager.appendCustomEntry('stream-helper-session',link)});
  f.manager.appendCustomEntry('stream-helper-session',{session_id:'missing',session_file:join(f.dir,'missing.jsonl')});
  const {capture,directory}=captureSessions(f.inventory,f.root,f.manager,undefined,join(f.dir,'original-helpers')),logs=capture.samples[0].helper_logs;
  assert.equal(logs.length,2);assert.match(readFileSync(join(directory,logs[0].file),'utf8'),/stream-helper-request/);
  assert.equal(statSync(join(directory,logs[0].file)).mode&0o777,0o600);assert.match(logs[1].error,/reporting sessions root/);
 }finally{f.dispose();}
});
test('debug storage refuses version-controlled agent directories before copying any content',async()=>{
 const f=fixture();try {
  const repo=join(f.dir,'repository');mkdirSync(join(repo,'.git'),{recursive:true});
  const root=join(repo,'.pi','stream-debug');
  assert.throws(()=>captureSessions(f.inventory,root),/outside version control/);assert.equal(existsSync(root),false);
  const {capture}=captureSessions(f.inventory,f.root);
  await assert.rejects(replayCapture(capture,root,runtime(()=>assert.fail('no model')),model,update,project,new AbortController().signal),/outside version control/);
  assert.equal(existsSync(root),false);
 }finally{f.dispose();}
});
test('storage rejects dangling links and symlinks into VCS before copying any content',()=>{
 const f=fixture();try {
  const repo=join(f.dir,'repository');mkdirSync(join(repo,'.git'),{recursive:true});
  const alias=join(f.dir,'alias'),dangling=join(f.dir,'dangling');
  symlinkSync(repo,alias);symlinkSync(join(repo,'future'),dangling);
  assert.throws(()=>captureSessions(f.inventory,join(alias,'debug')),/outside version control/);
  assert.throws(()=>captureSessions(f.inventory,join(dangling,'debug')),/ENOENT/);
  assert.equal(existsSync(join(repo,'debug')),false);assert.equal(existsSync(join(repo,'future')),false);
 }finally{f.dispose();}
});
test('capture reads reject linked storage and manifests without modifying snapshots',()=>{
 const f=fixture();try {
  const {capture,directory}=captureSessions(f.inventory,f.root);
  const alias=join(f.dir,'alias');mkdirSync(alias,{mode:0o700});symlinkSync(join(f.root,'captures'),join(alias,'captures'));
  assert.throws(()=>loadCapture(alias,capture.id),/symlinks/);assert.throws(()=>captureIds(alias),/symlinks/);
  const manifest=join(directory,'capture.json'),saved=join(directory,'original.json');renameSync(manifest,saved);symlinkSync(saved,manifest);
  assert.throws(()=>loadCapture(f.root,capture.id),/ELOOP/);assert.deepEqual(JSON.parse(readFileSync(saved,'utf8')),capture);
  const moved=join(f.dir,'original-capture');renameSync(directory,moved);symlinkSync(moved,directory);
  assert.throws(()=>loadCapture(f.root,capture.id),/symlinks/);
 }finally{f.dispose();}
});
test('loading a capture is read-only and enforces the VCS boundary as well as writing',()=>{
 const f=fixture();try {
  const {capture,directory}=captureSessions(f.inventory,f.root),manifest=join(directory,'capture.json');
  const before=statSync(manifest);loadCapture(f.root,capture.id);
  assert.equal(statSync(manifest).mtimeMs,before.mtimeMs);assert.equal(statSync(manifest).mode,before.mode);
  mkdirSync(join(f.root,'.git'));assert.throws(()=>loadCapture(f.root,capture.id),/outside version control/);
  assert.equal(readFileSync(manifest,'utf8'),JSON.stringify(capture,null,2)+'\n');
  const missing=join(f.dir,'not-created');assert.throws(()=>loadCapture(missing,capture.id),/ENOENT/);assert.equal(existsSync(missing),false);
 }finally{f.dispose();}
});
test('loaded captures reject malformed samples and incomplete or unsafe evidence metadata at the boundary',()=>{
 const f=fixture();try {
  const {capture,directory}=captureSessions(f.inventory,f.root);
  const malformed=[value=>value.samples=null,value=>value.samples=[null],value=>value.samples=[{}],value=>value.samples[0].catalog=null,
   value=>delete value.samples[0].cwd,value=>value.samples[0].cwd='',value=>delete value.samples[0].snapshot_file,
   value=>value.samples[0].snapshot_file='../outside.jsonl',value=>value.samples[0].snapshot_file='/outside.jsonl',value=>value.samples[0].evidence.messages=null];
  for(const alter of malformed) {
   const value=structuredClone(capture);alter(value);writeFileSync(join(directory,'capture.json'),JSON.stringify(value));
   assert.throws(()=>loadCapture(f.root,capture.id),error=>!(error instanceof TypeError)&&/Invalid|frozen/i.test(String(error)));
  }
 }finally{f.dispose();}
});
test('real preview child applies live derivative-clearing and reconciliation rules only to supplied cards',async t=>{
 const f=fixture();try {
  const {capture}=captureSessions(f.inventory,f.root),client=childCli(f.dir);t.after(()=>client.stop());
  const rt=runtime(async(_model,payload)=>response({changes:JSON.parse(payload.messages.at(-1).content).task==='session'?{remaining:'Finish tests'}:{goal:'Updated goal'}}));
  const {run}=await replayCapture(capture,f.root,rt,model,update,(input,signal)=>client.preview(input,signal),new AbortController().signal);
  const result=run.samples[0];assert.equal(result.status,'completed',result.error);
  assert.equal(result.after.session.remaining,'Finish tests');assert.equal(result.after.session.state_short,'Old state');assert.equal(result.after.session.remaining_short,null);
  assert.equal(result.after.stream.goal_short,null);assert.equal(result.after.stream.title_mode,'manual');
  assert.ok(result.receipts.every(receipt=>receipt.simulated&&receipt.save.local==='unchanged'));
  assert.equal(readFileSync(f.source,'utf8'),f.raw);
 }finally{f.dispose();}
});
test('real enrollment preview feeds later helpers with deterministic private working-copy identities',async t=>{
 const f=fixture();try {
  Object.assign(f.inventory.samples[0].catalog,{stream:null,session:null,sessions:[]});
  const {capture}=captureSessions(f.inventory,f.root),client=childCli(f.dir);t.after(()=>client.stop());
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
  const {capture}=captureSessions(f.inventory,f.root);capture.samples.push(structuredClone(capture.samples[0]));
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
test('enabled Pi commands capture both cards and evidence, then replay reset through the real preview adapter',async t=>{
 const f=fixture();try {
  let command,calls=0;const notices=[],statuses=[],dialogs=[],menus=[],before=f.manager.getEntries();
  f.inventory.samples[0].catalog.session.title_mode='manual';
  const rt=runtime(async()=>{calls++;return response({changes:{title:'Regenerated',state:'Verified synthetic result'}});});
  rt.modelRegistry.getAvailable=()=>[model];
  const ctx={cwd:f.dir,hasUI:true,sessionManager:f.manager,...rt,ui:{
   notify:text=>notices.push(text),setStatus:(...args)=>statuses.push(args),
   confirm:async(title,message,options)=>{assert.ok(options.signal);dialogs.push([title,message]);return true;},
   select:async(title,labels,options)=>{assert.ok(options.signal);menus.push([title,labels]);return labels[0];},
  }};
  const real=childCli(f.dir);t.after(()=>real.stop());const client={captureInputs:async target=>{assert.deepEqual(target,{kind:'stream',id:'effort'});return f.inventory;},preview:(input,signal)=>real.preview(input,signal)};
  extension({registerCommand:(_name,value)=>{command=value;},registerTool(){},on(){},appendEntry:()=>assert.fail('debug must not append to real history')},{agentDir:f.dir,cli:client,settings:{...defaults,debug_commands:true}});
  assert.equal(command.getArgumentCompletions('debug')[0].value,'debug capture');
  await command.handler('debug capture stream effort',ctx);assert.equal(calls,0);
  const root=join(f.dir,'stream-debug'),id=captureIds(root)[0];assert.ok(id,notices.join('\n'));
  const snapshot=loadCapture(root,id);assert.ok(snapshot.samples[0].evidence);assert.equal(snapshot.samples[0].catalog.session.title,'Lane');
  await command.handler(`debug replay ${id} --reset session --auto`,ctx);
  assert.equal(calls,1,notices.join('\n'));
  const comparison=notices.at(-1).split('Comparison: ')[1],run=JSON.parse(readFileSync(comparison.replace(/comparison\.md$/,'run.json'),'utf8'));
  assert.equal(run.samples[0].status,'completed',run.samples[0].error);assert.equal(run.samples[0].after.session.title,'Regenerated');
  assert.equal(loadCapture(root,id).samples[0].catalog.session.title_mode,'manual');assert.deepEqual(f.manager.getEntries(),before);
  assert.equal(readFileSync(f.source,'utf8'),f.raw);assert.match(menus[0][0],/1 Pi conversations; 1 available/);assert.equal(menus[0][1][0],'Capture private samples');
  assert.match(dialogs[0][0],/Run model inference/);
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
  const {capture}=captureSessions(f.inventory,f.root);
  await assert.rejects(replayCapture(capture,f.root,runtime(()=>assert.fail('no call')),{provider:'missing',id:'none'},update,project,new AbortController().signal),/unavailable/);
 }finally{f.dispose();}
});
