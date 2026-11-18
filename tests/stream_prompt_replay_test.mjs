import assert from 'node:assert/strict';
import test from 'node:test';
import {sessionId} from './helpers/session-id.mjs';
import { appendFileSync, chmodSync, existsSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, statSync, writeFileSync, mkdirSync, symlinkSync, unlinkSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createJiti } from 'jiti';
import { SessionManager } from '@earendil-works/pi-coding-agent';
const jiti = createJiti(import.meta.url);
const load = name => jiti.import(new URL(`../extensions/stream-checkpoint/${name}.ts`, import.meta.url).pathname);
const { captureSessions, loadCapture, parseCaptureInventory, privateDirectory, privateWrite } = await load('capture');
const { replayCapture } = await load('replay');
const { privateRead } = await load('private-storage');
const { buildInferencePayload, parseInferenceResponse, runInference } = await load('agent');
const { debugCommand, parseDebugCommand } = await load('debug');
const { StreamCli } = await load('streams');
const { defaults } = await load('settings');
const { helperSessionLogger } = await load('helper-log');
const model = {provider:'fake', id:'chosen', contextWindow:100000};
const stream = {id:'effort', title:'Work', title_mode:'automatic', goal:'Old goal', goal_short:'Old', goal_mode:'automatic', status:'active', owner:{host_id:'host'}, repositories:{}, references:[]};
const session = {id:sessionId('host','pi','native'),host_id:'host',stream_id:'effort',harness:'pi',harness_session_id:'native',title:'Lane',title_mode:'automatic',status:'active',state:'Old state',state_short:'Old',remaining:'Old remaining',checkout:null};
const catalog = native => {
 const card = {...session,id:sessionId('host','pi',native),harness_session_id:native};
 return structuredClone({catalog_source:'frozen-source',host_id:'host',stream,session:card,sessions:[card],title_defaults:{stream:'automatic',session:'automatic'}});
};
const response = args => ({role:'assistant',content:[{type:'toolCall',id:'call',name:'record_card',arguments:args}],api:'fake',provider:'fake',model:'chosen',stopReason:'toolUse',timestamp:0});
const runtime = generate => ({model:{...model,id:'not-selected'}, modelRegistry:{find:(provider,id)=>provider===model.provider && id===model.id?model:undefined,complete:async(selected,payload,options)=>{assert.equal(selected.id,'chosen');return generate(payload,options);}}});
async function temporary(run) {
 const root = mkdtempSync(join(realpathSync(tmpdir()),'streams-replay-'));
 try {return await run(root);} finally {rmSync(root,{recursive:true,force:true});}
}
function setup(root, ids=['native']) {
 const samples = ids.map(native => {
  const file = join(root,`${native}.jsonl`);
  const entries = [{type:'session',version:3,id:native,cwd:'/repo',timestamp:new Date(0).toISOString()}, {type:'message',id:'first',parentId:null,timestamp:new Date(0).toISOString(),message:{role:'user',content:`Evidence ${native}`,timestamp:0}}, {type:'message',id:'last',parentId:'first',timestamp:new Date(1).toISOString(),message:{role:'user',content:'Persisted leaf',timestamp:1}}];
  writeFileSync(file,entries.map(entry=>JSON.stringify(entry)).join('\n')+'\n',{mode:0o600});
  return {host_id:'host',harness:'pi',native_session_id:native,open:false,locations:[],catalog:catalog(native),transcript_file:file,unavailable:null};
 });
 const inventory = {version:1,host_id:'host',streams:[stream],workspaces:[],topology_error:null,target:{kind:'stream',id:'effort',title:'Work'},samples,unsupported:[]};
 return {inventory,...captureSessions(inventory,join(root,'private'))};
}
const taskInput = payload => JSON.parse(payload.messages.at(-1).content);
function previewFixture(onRequest=()=>{}) {
 return async (input,signal) => {
  signal.throwIfAborted();onRequest(structuredClone(input));
  const next = structuredClone(input.catalog), request=input.request;
  if(input.action==='apply') {
   Object.assign(next[request.kind],request.changes);
   if(request.kind==='session') next.sessions = next.sessions.map(card=>card.id===next.session.id?structuredClone(next.session):card);
  } else {
   next.stream ??= {...structuredClone(stream),...request.stream};
   next.session = {...structuredClone(session),...request.session,id:sessionId('host',request.harness,request.harness_session_id),harness_session_id:request.harness_session_id};next.sessions=[structuredClone(next.session)];
  }
  return {catalog:next,receipt:{save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}},simulated:true}};
 };
}

function helperFixture(root, inventory) {
 const helperRoot = join(root, 'stream-reporting-sessions');privateDirectory(helperRoot);
 const file = join(helperRoot, 'helper.jsonl'), session_id = 'helper';
 const raw = [{type:'session',version:3,id:session_id,cwd:'/repo'}, {type:'custom',id:'request',parentId:null,customType:'stream-helper-request',data:{parent_session_id:inventory.samples[0].native_session_id}}].map(JSON.stringify).join('\n')+'\n';
 writeFileSync(file,raw,{mode:0o600});
 appendFileSync(inventory.samples[0].transcript_file,JSON.stringify({type:'custom',id:'helper-link',parentId:'last',customType:'stream-helper-session',data:{session_id,session_file:file}})+'\n');
 return {helperRoot,file,raw};
}

test('saved setup freezes addressed sources without authority and replays independent session-before-effort trials', async()=>temporary(async root=>{
 const {capture,directory,inventory}=setup(root,['one','two']);
 const original = JSON.stringify(capture), bytes=readFileSync(inventory.samples[0].transcript_file,'utf8');
 assert.equal(Object.hasOwn(capture.samples[0].catalog,'authority'),false);assert.deepEqual(capture.samples[0].catalog.title_defaults,{stream:'automatic',session:'automatic'});
 writeFileSync(inventory.samples[0].transcript_file,'source changed after capture');
 const frozen=loadCapture(join(root,'private'),capture.id);assert.deepEqual(frozen,capture);
 const seen=[], projected=[];
 const fake=runtime(async payload=>{
  const input=taskInput(payload);seen.push(input);
  assert.equal(input.catalog.authority,undefined);assert.equal(input.catalog.catalog_source,undefined);
  if(input.task==='session') {assert.equal(input.catalog.stream.goal,'Old goal');return response({changes:{state:'Fresh state'}});}
  assert.equal(Object.hasOwn(input.catalog,'session'),false);assert.equal(input.catalog.sessions.find(card=>card.harness_session_id===seen.at(-2).catalog.session.harness_session_id).state,'Fresh state');return response({changes:{goal:'Fresh goal'}});
 });
 const {run,directory:runDir}=await replayCapture(frozen,join(root,'private'),fake,model,{kind:'report',force:true},previewFixture(input=>projected.push(input)),new AbortController().signal);
 assert.deepEqual(seen.map(input=>input.task),['session','stream','session','stream']);
 assert.equal(projected[2].catalog.stream.goal,'Old goal','second trial must not inherit first effort update');
 assert.ok(run.samples.every(sample=>sample.status==='completed' && sample.after.stream.goal==='Fresh goal'));
 assert.equal(JSON.stringify(capture),original);assert.equal(readFileSync(join(directory,'session-0.jsonl'),'utf8'),bytes);
 assert.deepEqual(run.capture_source_files,capture.source_files);assert.equal(run.samples[0].tasks[0].model.id,'chosen');
 const task=run.samples[0].tasks[0],prepared=JSON.parse(readFileSync(join(root,'private','requests',task.request_id,'request.json'),'utf8'));
 assert.ok(prepared.model_request.systemPrompt && prepared.model_request.tools[0].parameters);
 assert.ok(task.raw_response);assert.ok(task.result);
 assert.match(readFileSync(join(runDir,task.helper_file),'utf8'),/stream-helper-result/);
 for(const dir of [directory,runDir]) assert.equal(statSync(dir).mode & 0o777,0o700);
 for(const path of [join(directory,'capture.json'),join(runDir,'run.json'),join(runDir,task.helper_file)]) assert.equal(statSync(path).mode & 0o777,0o600);
}));

test('current in-memory leaf is distinct from another process persisted leaf; sources are never opened for writes',async()=>temporary(async root=>{
 const {inventory}=setup(root,['native','other']);
 const path=inventory.samples[0].transcript_file, bytes=readFileSync(path,'utf8');
 const manager=SessionManager.inMemory('/repo',undefined,bytes.trim().split('\n').map(JSON.parse));manager.branch('first');
 inventory.samples.forEach(sample=>sample.open=true);
 const {capture}=captureSessions(inventory,join(root,'second-private'),manager);
 assert.equal(capture.samples[0].projection_source,'current-session');assert.equal(capture.samples[0].evidence.leafId,'first');assert.equal(capture.samples[0].evidence.messages.length,1);
 assert.equal(capture.samples[1].projection_source,'persisted-leaf');assert.equal(capture.samples[1].evidence.leafId,'last');assert.equal(capture.samples[1].evidence.messages.length,2);
 assert.match(capture.samples[1].warnings[0],/persisted leaf/);assert.equal(readFileSync(path,'utf8'),bytes);
}));

for(const auto of [false,true]) test(`reset respects manual fields unless explicit --auto (${auto})`,async()=>temporary(async root=>{
 const {inventory}=setup(root);inventory.samples[0].catalog.session.title_mode='manual';inventory.samples[0].catalog.stream.title_mode='manual';inventory.samples[0].catalog.stream.goal_mode='manual';
 const {capture}=captureSessions(inventory,join(root,'manual'));const writes=[],inputs=[];
 const {run}=await replayCapture(capture,join(root,'manual'),runtime(async payload=>{
  const input=taskInput(payload);inputs.push(input);
  if(input.task==='session') {
   assert.equal(Object.hasOwn(input.catalog.session,'state'),false);assert.equal(Object.hasOwn(input.catalog.session,'title'),false);assert.deepEqual(input.catalog.sessions,[]);
   return response({changes:{state:'Reset state',...(auto?{title:'Fresh lane'}:{})}});
  }
  assert.equal(auto,true);assert.equal(input.catalog.stream.goal,null);return response({changes:{goal:'New goal',title:'Fresh effort'}});
 }),model,{kind:'reset',target:'all',auto},previewFixture(input=>writes.push(input)),new AbortController().signal);
 assert.equal(run.samples[0].after.session.title_mode,auto?'automatic':'manual');
 assert.equal(run.samples[0].after.session.state_short,null);
 assert.deepEqual(inputs.map(input=>input.task),auto?['session','stream']:['session']);
 assert.equal(writes.filter(input=>input.request.actor==='user').length,auto?2:0);
}));

test('reset replay uses frozen successful and unavailable location hints',async()=>temporary(async root=>{
 const {inventory}=setup(root),catalog=inventory.samples[0].catalog;
 catalog.checkouts=[{repository_url:'repo',checkout_id:'checkout',root:'/work'}];
 catalog.document_locations=[{repository_url:'repo',path:'plan',checkout_id:'checkout',root:'/work',kind:'local',file:'/work/plan',error:null},{repository_url:'repo',path:'missing',checkout_id:null,root:null,kind:null,file:null,error:'Unavailable'}];
 const expected=structuredClone({checkouts:catalog.checkouts,document_locations:catalog.document_locations});
 const {capture}=captureSessions(inventory,join(root,'frozen-hints'));catalog.checkouts=[];catalog.document_locations=[];
 const seen=[];const {run}=await replayCapture(capture,join(root,'frozen-hints'),runtime(async payload=>{const input=taskInput(payload);seen.push(input);return response({changes:{}});}),model,{kind:'reset',target:'session',auto:false},previewFixture(),new AbortController().signal);
 assert.equal(run.samples[0].status,'completed');assert.deepEqual(seen[0].catalog.checkouts,expected.checkouts);assert.deepEqual(seen[0].catalog.document_locations,expected.document_locations);
 assert.deepEqual(run.samples[0].before.checkouts,expected.checkouts);assert.deepEqual(run.samples[0].after.document_locations,expected.document_locations);
}));
test('partial reference replay completes with attention and full receipt, without add/drop notices',async()=>temporary(async root=>{
 const {inventory}=setup(root),base=inventory.samples[0].catalog;
 const dropped={name:'Retired',purpose:'Old',role:'dependency',state:'dropped',target:{kind:'link',url:'https://example.org/retired'}};
 const active={name:'Brief',purpose:'Old',role:'brief',state:'active',target:{kind:'issue',url:'https://example.org/brief'}};
 base.stream.references=[dropped,active];base.stream.title_mode='manual';base.stream.goal_mode='manual';
 const restore={...dropped},update={...active};restore.state='active';update.purpose='Updated';
 const {capture}=captureSessions(inventory,join(root,'partial'));
 const preview=async input=>{const rows=input.request.changes.reference_changes,next=structuredClone(input.catalog);next.stream.references[1]={...update};return {catalog:next,receipt:{simulated:true,save:{local:'unchanged',domain:{outcome:'changes',reference_results:[{key:rows[0].key,action:'restore',outcome:'superseded',reason:'reference is Dropped; explicit restoration is required'},{key:rows[1].key,action:'update',outcome:'changes',reason:''}]}}}};};
 const {run}=await replayCapture(capture,join(root,'partial'),runtime(async()=>response({changes:{},reference_proposals:[restore,update]})),model,{kind:'report',force:true},preview,new AbortController().signal);
 const sample=run.samples[0];assert.equal(sample.status,'completed',sample.error);assert.equal(sample.after.stream.references[1].purpose,'Updated');
 assert.deepEqual(sample.receipts[0].save.domain.reference_results.map(row=>row.outcome),['superseded','changes']);
 assert.ok(sample.notices.some(text=>text.includes('Reference not applied:')));assert.ok(sample.notices.every(text=>!text.startsWith('Reference added:')&&!text.startsWith('Reference dropped:')));
}));
test('unattached trials simulate enrollment before ordered reporting and keep unavailable samples listed',async()=>temporary(async root=>{
 const {inventory}=setup(root,['native','missing']);
 const base=inventory.samples[0].catalog;base.stream=null;base.session=null;base.sessions=[];
 inventory.target={kind:'workspace',id:'workspace',title:'Workspace'};
 inventory.samples[1].unavailable='Transcript unavailable';inventory.samples[1].transcript_file=null;
 const {capture}=captureSessions(inventory,join(root,'enrollment'));const tasks=[],writes=[];
 const {run}=await replayCapture(capture,join(root,'enrollment'),runtime(async payload=>{
  const input=taskInput(payload);tasks.push(input.task);
  return response(input.task==='enroll'?{meaningful:true,stream:{title:'Effort'},session:{title:'Lane'}}:{changes:input.task==='session'?{state:'Fresh'}:{goal:'Goal'}});
 }),model,{kind:'report',force:true},previewFixture(input=>writes.push(input.action)),new AbortController().signal);
 assert.deepEqual(tasks,['enroll','session','stream']);assert.deepEqual(writes,['enroll','apply','apply']);
 assert.deepEqual(run.samples.map(sample=>sample.status),['completed','unavailable']);assert.equal(capture.samples[0].catalog.session,null);
}));

test('later helper failure retains earlier projected save plus raw invalid response and rendered schema',async()=>temporary(async root=>{
 const {capture}=setup(root);let calls=0;
 const {run,directory}=await replayCapture(capture,join(root,'private'),runtime(async()=>response(++calls===1?{changes:{state:'Saved simulation'}}:{changes:{unknown_field:'must reject'}})),model,{kind:'report',force:true},previewFixture(),new AbortController().signal);
 const sample=run.samples[0];assert.equal(sample.status,'failed');assert.equal(sample.after.session.state,'Saved simulation');assert.equal(sample.after.stream.goal,'Old goal');assert.equal(sample.receipts.length,1);
 const task=sample.tasks[1],prepared=JSON.parse(readFileSync(join(root,'private','requests',task.request_id,'request.json'),'utf8'));
 assert.equal(task.raw_response.content[0].arguments.changes.unknown_field,'must reject');
 assert.ok(prepared.model_request.tools[0].parameters);assert.match(task.error,/invalid structured/);
}));

test('production parser preserves manual protection and rejects unknown fields, replay has no permissive parser',()=>{
 const base=catalog('native');base.session.title_mode='manual';
 const request={type:'session',catalog:base,messages:[]};
 assert.deepEqual(parseInferenceResponse(request,response({changes:{title:'Unapproved',state:null}})),{changes:{state:''}});
 assert.throws(()=>parseInferenceResponse(request,response({changes:{unrecognized:'x'}})),/invalid structured/);
 const payload=buildInferencePayload(request);
 assert.match(payload.systemPrompt,/Infer session.status/);assert.match(payload.systemPrompt,/in one short sentence/);
 assert.equal(payload.tools[0].parameters.additionalProperties,false);
 assert.equal(payload.tools[0].parameters.properties.changes.additionalProperties,false);
});

test('no model fallback and cancellation before or during a trial never previews writes',async()=>temporary(async root=>{
 const {capture}=setup(root,['one','two']);const aborted=new AbortController();aborted.abort();
 await assert.rejects(replayCapture(capture,join(root,'private'),runtime(()=>assert.fail('model called')),model,{kind:'report',force:true},()=>assert.fail('preview'),aborted.signal));
 await assert.rejects(replayCapture(capture,join(root,'private'),runtime(()=>assert.fail('model called')),{provider:'missing',id:'missing'},{kind:'report',force:true},()=>assert.fail('preview'),new AbortController().signal),/no fallback/);
 const controller=new AbortController();
 const {run}=await replayCapture(capture,join(root,'private'),runtime(async()=>{controller.abort();return response({changes:{state:'No write'}});}),model,{kind:'report',force:true},()=>assert.fail('preview'),controller.signal);
 assert.deepEqual(run.samples.map(sample=>sample.status),['cancelled','not-run']);assert.equal(run.samples[0].tasks[0].status,'failed');
}));

test('debug gate and cancelled consent do not call models or preview; capture consent precedes copying',async()=>temporary(async root=>{
 const signal=new AbortController().signal, client={captureInputs:()=>assert.fail('disabled read'),preview:()=>assert.fail('live capability')};
 await assert.rejects(debugCommand('capture',{},client,root,defaults,signal),/disabled/);
 const {inventory}=setup(root);const {capture}=captureSessions(inventory,join(root,'stream-debug'));let confirmations=0;
 const ctx={hasUI:true,modelRegistry:{find:()=>model,complete:()=>assert.fail('no consent')},ui:{confirm:async(title,text)=>{confirmations++;assert.match(text,/costs/);return false;}}};
 await debugCommand(`replay ${capture.id} --model fake/chosen`,ctx,client,root,{...defaults,debug_commands:true},signal);
 assert.equal(confirmations,1);
 const captureCtx={hasUI:true,sessionManager:{getSessionId:()=> 'native'},ui:{select:async()=>{confirmations++;return 'Cancel';}}};
 await debugCommand('capture stream effort',captureCtx,{captureInputs:async()=>inventory,preview:()=>assert.fail('preview')},root,{...defaults,debug_commands:true},signal);
 assert.equal(confirmations,2);
 assert.deepEqual(parseDebugCommand('replay --model fake/chosen --reset session --auto').action,{kind:'reset',target:'session',auto:true});
 assert.throws(()=>parseDebugCommand('replay --auto'),/requires --reset/);
}));

test('private storage refuses repositories, symlink files and symlink directories without changing targets',async()=>temporary(async root=>{
 const target=join(root,'target');writeFileSync(target,'original',{mode:0o600});
 const directory=join(root,'private');privateDirectory(directory);symlinkSync(target,join(directory,'linked'));
 assert.throws(()=>privateWrite(join(directory,'linked'),'replacement'));assert.equal(readFileSync(target,'utf8'),'original');
 symlinkSync(directory,join(root,'redirect'));assert.throws(()=>privateWrite(join(root,'redirect','artifact'),'private'),/symlink/);
 const repository=join(root,'repo');mkdirSync(repository);mkdirSync(join(repository,'.git'));assert.throws(()=>privateDirectory(join(repository,'captures')),/version control/);
}));

test('tampered capture paths, snapshot hashes, card identity, and live tokens are rejected',async()=>temporary(async root=>{
 const {capture,directory}=setup(root);const path=join(directory,'capture.json'), original=readFileSync(path,'utf8');
 const cases=[
  value=>{value.samples[0].snapshot_file='../../outside.jsonl';},
  value=>{value.samples[0].native_session_id='different';},
  value=>{value.samples[0].catalog.authority={host_id:'host',token:'LIVE'};},
  value=>{value.samples[0].snapshot_hash='tampered';},
  value=>{value.samples[0].evidence.messages=[];},
  value=>{value.target.id='different';},
 ];
 for(const tamper of cases) {const value=JSON.parse(original);tamper(value);writeFileSync(path,JSON.stringify(value));assert.throws(()=>loadCapture(join(root,'private'),capture.id));}
 writeFileSync(path,original);assert.throws(()=>loadCapture(join(root,'private'),'../escape'),/exact capture ID/);
 unlinkSync(path);symlinkSync(join(root,'native.jsonl'),path);assert.throws(()=>loadCapture(join(root,'private'),capture.id));
 const inventory=setup(root,['different']).inventory;inventory.samples[0].catalog.session.harness_session_id='wrong';assert.throws(()=>parseCaptureInventory(inventory),/identity mismatch/);
}));

test('StreamCli debug boundary sends typed capture-inputs and pure preview',async()=>{
 const calls=[], base=catalog('native');const receipt={save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}},simulated:true};
 const cli=new StreamCli('/repo',async request=>{calls.push(request);return {status:'success',result:request.kind==='preview'?{catalog:base,receipt}:{version:1}};});
 await cli.captureInputs({kind:'workspace',id:'workspace'},new AbortController().signal);
 await cli.preview({catalog:base,action:'apply',request:{}});
 assert.deepEqual(calls,[{kind:'capture_inputs',target:{kind:'workspace',workspace_id:'workspace'}},{kind:'preview',catalog:base,action:'apply',request:{}}]);
});

test('opt-in helper logging saves a private Pi transcript with response and parse error',async()=>temporary(async root=>{
 const manager=SessionManager.inMemory('/repo');manager.appendMessage({role:'user',content:'Evidence',timestamp:0});let link;
 const observe=helperSessionLogger({cwd:'/repo',sessionId:manager.getSessionId(),leafId:manager.getLeafId(),sessionFile:manager.getSessionFile()},{directory:join(root,'logs'),onCreate:value=>{link=value;}});
 await assert.rejects(runInference(runtime(async()=>response({changes:{unknown:'invalid'}})),{type:'session',catalog:catalog('native'),messages:[]},new AbortController().signal,model,observe),/invalid structured/);
 const entries=readFileSync(link.session_file,'utf8').trim().split('\n').map(JSON.parse);
 assert.equal(entries[0].type,'session');assert.equal(statSync(link.session_file).mode & 0o777,0o600);
 assert.ok(entries.some(entry=>entry.type==='message' && entry.message.role==='assistant'));assert.ok(entries.some(entry=>entry.customType==='stream-helper-error'));assert.doesNotMatch(readFileSync(link.session_file,'utf8'),/LIVE-SECRET/);
 let parentId=null;
 for(const entry of entries.slice(1)) {assert.equal(entry.parentId,parentId);parentId=entry.id;}
}));

test('foreign samples use the local catalog host, round-trip, and stay unavailable without inference',async()=>temporary(async root=>{
 const {inventory}=setup(root);const foreign=inventory.samples[0];
 foreign.host_id='remote';foreign.catalog.stream.owner.host_id='remote';
 Object.assign(foreign.catalog.session,{host_id:'remote',id:sessionId('remote','pi',foreign.native_session_id)});
 foreign.catalog.sessions=[foreign.catalog.session];
 foreign.transcript_file=null;foreign.unavailable='Transcript belongs to another host';
 const validated=parseCaptureInventory(JSON.parse(JSON.stringify(inventory)));
 const {capture}=captureSessions(validated,join(root,'foreign'));
 const frozen=loadCapture(join(root,'foreign'),capture.id);assert.deepEqual(frozen,capture);
 const {run}=await replayCapture(frozen,join(root,'foreign'),runtime(()=>assert.fail('foreign inference')),model,{kind:'report',force:true},()=>assert.fail('foreign preview'),new AbortController().signal);
 assert.equal(run.samples[0].status,'unavailable');
 for(const tamper of [sample=>{sample.catalog.host_id='remote';},sample=>{sample.catalog.session.host_id='host';},sample=>{sample.unavailable=' ';},sample=>{sample.transcript_file='/local.jsonl';}]) {
  const bad=structuredClone(inventory);tamper(bad.samples[0]);assert.throws(()=>parseCaptureInventory(bad));
 }
}));

test('cross-stream live panes require a workspace mapped to the selected stream and survive load',async()=>temporary(async root=>{
 const {inventory}=setup(root);inventory.target={kind:'stream',id:'selected',title:'Selected'};
 inventory.workspaces=[{id:'workspace',label:'Workspace',stream_id:'selected'}];
 inventory.samples[0].open=true;inventory.samples[0].locations=[{workspace_id:'workspace',tab_id:'tab',pane_id:'pane',terminal_id:'terminal'}];
 const {capture}=captureSessions(JSON.parse(JSON.stringify(inventory)),join(root,'cross-stream'));
 assert.deepEqual(loadCapture(join(root,'cross-stream'),capture.id),capture);
 assert.equal(capture.samples[0].catalog.stream.id,'effort');assert.ok(capture.samples[0].evidence);
 for(const tamper of [value=>{value.samples[0].open=false;},value=>{value.samples[0].open=null;},value=>{value.samples[0].locations=[];},value=>{value.samples[0].locations[0].workspace_id='forged';},value=>{value.workspaces[0].stream_id='unrelated';},value=>{value.samples[0].catalog.host_id='forged';}]) {
  const bad=structuredClone(inventory);tamper(bad);assert.throws(()=>parseCaptureInventory(bad),/identity mismatch/);
 }
}));

test('private roots allow trusted alias ancestors, including missing bases, but not managed symlinks',async()=>temporary(async root=>{
 const actual=join(root,'actual');mkdirSync(actual,{mode:0o700});
 const alias=join(root,'os-prefix');symlinkSync(actual,alias);
 for(const base of [alias,join(alias,'missing','base')]) {
  const managed=join(base,'stream-debug'), directory=join(managed,'captures','one');
  privateDirectory(directory,managed);privateWrite(join(directory,'snapshot'),'private',managed);
  assert.equal(privateRead(directory,'snapshot',managed),'private');
  assert.equal(statSync(managed).mode & 0o777,0o700);
 }
 const owned=join(alias,'stream-debug');
 symlinkSync(owned,join(alias,'linked-root'));
 assert.throws(()=>privateDirectory(join(alias,'linked-root','runs'),join(alias,'linked-root')),/symlink/);
 symlinkSync(actual,join(owned,'runs'));
 assert.throws(()=>privateWrite(join(owned,'runs','artifact'),'private',owned),/symlink/);
 symlinkSync(actual,join(owned,'captures','linked'));
 assert.throws(()=>privateRead(join(owned,'captures','linked'),'snapshot',owned),/symlink/);
 chmodSync(join(owned,'captures'),0o755);
 assert.throws(()=>privateDirectory(join(owned,'captures','one'),owned),/0700/);
}));

test('repository containment is rejected through aliases and at the managed root itself',async()=>temporary(async root=>{
 const repo=join(root,'repo');mkdirSync(repo,{mode:0o700});mkdirSync(join(repo,'.git'));
 const alias=join(root,'alias');symlinkSync(repo,alias);
 assert.throws(()=>privateDirectory(join(alias,'missing','stream-debug'),join(alias,'missing','stream-debug')),/version control/);
 assert.equal(existsSync(join(repo,'missing')),false);
 const managed=join(root,'stream-debug');mkdirSync(managed,{mode:0o700});writeFileSync(join(managed,'.git'),'gitdir: elsewhere');
 assert.throws(()=>privateDirectory(join(managed,'captures'),managed),/version control/);
}));

test('helper links copy only private regular files within the explicit reporting root',async()=>temporary(async root=>{
 const {inventory}=setup(root);const {helperRoot,file,raw}=helperFixture(root,inventory);
 const {capture}=captureSessions(inventory,join(root,'helpers'),undefined,undefined,helperRoot);
 const frozen=loadCapture(join(root,'helpers'),capture.id);
 assert.equal(frozen.samples[0].helper_logs[0].session_id,'helper');
 assert.equal(privateRead(join(root,'helpers','captures',capture.id),frozen.samples[0].helper_logs[0].file,join(root,'helpers')),raw);
 chmodSync(file,0o644);
 const insecure=captureSessions(inventory,join(root,'insecure-helper'),undefined,undefined,helperRoot).capture.samples[0].helper_logs[0];
 assert.equal(insecure.file,undefined);assert.match(insecure.error,/0600/);
 chmodSync(file,0o600);
 const outside=captureSessions(inventory,join(root,'outside-helper'),undefined,undefined,join(root,'other-root')).capture.samples[0].helper_logs[0];
 assert.equal(outside.file,undefined);assert.match(outside.error,/reporting sessions root/);
 const target=join(root,'other.jsonl');writeFileSync(target,raw,{mode:0o600});unlinkSync(file);symlinkSync(target,file);
 const linked=captureSessions(inventory,join(root,'linked-helper'),undefined,undefined,helperRoot).capture.samples[0].helper_logs[0];
 assert.equal(linked.file,undefined);assert.ok(linked.error);
}));

test('FIFO helper links are rejected without blocking capture',async()=>temporary(async root=>{
 const {inventory}=setup(root);const {helperRoot,file}=helperFixture(root,inventory);unlinkSync(file);
 const fifo=spawnSync('mkfifo',[file]);assert.equal(fifo.status,0,fifo.stderr?.toString());chmodSync(file,0o600);
 const script=`import assert from 'node:assert/strict';import {createJiti} from 'jiti';const jiti=createJiti(import.meta.url);const {captureSessions}=await jiti.import(${JSON.stringify(new URL('../extensions/stream-checkpoint/capture.ts',import.meta.url).pathname)});const {capture}=captureSessions(${JSON.stringify(inventory)},${JSON.stringify(join(root,'fifo-capture'))},undefined,undefined,${JSON.stringify(helperRoot)});const log=capture.samples[0].helper_logs[0];assert.equal(log.file,undefined);assert.match(log.error,/0600 file/);`;
 const result=spawnSync(process.execPath,['--input-type=module','-e',script],{cwd:new URL('..',import.meta.url),timeout:10000,encoding:'utf8'});
 assert.equal(result.error,undefined);assert.equal(result.status,0,result.stderr);
}));

test('malformed frozen helper log objects and empty identities are rejected',async()=>temporary(async root=>{
 const {inventory}=setup(root);const {helperRoot}=helperFixture(root,inventory);
 const {capture,directory}=captureSessions(inventory,join(root,'helpers'),undefined,undefined,helperRoot);
 const path=join(directory,'capture.json'), original=readFileSync(path,'utf8');
 for(const log of [null,[],{},'helper',{session_id:''},{session_id:' '},{session_id:'helper',file:null},{session_id:'helper',error:4}]) {
  const value=JSON.parse(original);value.samples[0].helper_logs=[log];writeFileSync(path,JSON.stringify(value));
  assert.throws(()=>loadCapture(join(root,'helpers'),capture.id),/Invalid frozen helper log/);
 }
}));

for(const kind of ['snapshot','helper']) test(`failed ${kind} writes leave no dangling artifact metadata`,async t=>temporary(async root=>{
 const {inventory}=setup(root);const {helperRoot}=helperFixture(root,inventory);
 const managed=join(root,'failed-write'), manager=SessionManager.inMemory('/repo',undefined,readFileSync(inventory.samples[0].transcript_file,'utf8').trim().split('\n').map(JSON.parse));
 const header=manager.getHeader.bind(manager);let collision;
 t.mock.method(manager,'getHeader',()=>{
  const directory=join(managed,'captures',readdirSync(join(managed,'captures'))[0]);
  collision=join(directory,kind==='snapshot'?'session-0.jsonl':'session-0-helper-1.jsonl');
  writeFileSync(collision,'occupied',{mode:0o600});return header();
 });
 const saved=captureSessions(inventory,managed,manager,undefined,helperRoot);
 assert.equal(readFileSync(collision,'utf8'),'occupied');
 const sample=saved.capture.samples[0];
 if(kind==='snapshot') {
  assert.match(sample.unavailable,/EEXIST/);
  for(const key of ['cwd','projection_source','evidence','evidence_hash','snapshot_file','snapshot_hash']) assert.equal(sample[key],undefined,key);
 } else {
  assert.ok(sample.evidence);assert.equal(sample.helper_logs[0].file,undefined);assert.match(sample.helper_logs[0].error,/EEXIST/);
 }
 assert.deepEqual(loadCapture(join(root,'failed-write'),saved.capture.id),saved.capture);
}));

test('unattached reset is skipped with its notice and zero inference or preview calls',async()=>temporary(async root=>{
 const {inventory}=setup(root);inventory.target={kind:'workspace',id:'workspace',title:'Workspace'};
 Object.assign(inventory.samples[0].catalog,{session:null,stream:null,sessions:[]});
 const {capture}=captureSessions(inventory,join(root,'reset'));
 const {run}=await replayCapture(capture,join(root,'reset'),runtime(()=>assert.fail('reset inference')),model,{kind:'reset',target:'all',auto:true},()=>assert.fail('reset preview'),new AbortController().signal);
 assert.equal(run.samples[0].status,'skipped');assert.equal(run.samples[0].tasks.length,0);
 assert.deepEqual(run.samples[0].notices,['Reset requires an attached work card. Use /streams attach first.']);
}));

test('refused projection after inference is failed rather than completed',async()=>temporary(async root=>{
 const {capture}=setup(root);
 const {run}=await replayCapture(capture,join(root,'private'),runtime(async()=>response({changes:{state:'Refused'}})),model,{kind:'report',force:true},async input=>({catalog:input.catalog,receipt:{save:{local:'unchanged',sharing:'local-only',domain:{outcome:'superseded'}}}}),new AbortController().signal);
 assert.equal(run.samples[0].status,'failed');assert.equal(run.samples[0].tasks.length,1);assert.equal(run.samples[0].after.session.state,'Old state');
}));

test('debug replay clears UI status after cancellation',async()=>temporary(async root=>{
 const {inventory}=setup(root);const {capture}=captureSessions(inventory,join(root,'stream-debug'));
 const controller=new AbortController(), statuses=[];
 const ctx={...runtime(async()=>{controller.abort();return response({changes:{state:'Cancelled'}});}),hasUI:true,ui:{confirm:async()=>true,setStatus:(...args)=>statuses.push(args),notify:()=>assert.fail('cancelled notification')}};
 await debugCommand(`replay ${capture.id} --model fake/chosen`,ctx,{preview:()=>assert.fail('cancelled preview')},root,{...defaults,debug_commands:true},controller.signal);
 assert.deepEqual(statuses,[['stream-debug','Streams debug · Replaying…'],['stream-debug',undefined]]);
}));
