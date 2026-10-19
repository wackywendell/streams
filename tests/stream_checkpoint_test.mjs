import assert from 'node:assert/strict';
import test from 'node:test';
import { spawn } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, statSync, writeFileSync, chmodSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';
import { createJiti } from 'jiti';
const jiti = createJiti(import.meta.url);
const load = (name) => jiti.import(new URL(`../extensions/stream-checkpoint/${name}.ts`, import.meta.url).pathname);
const { StreamCli, StreamsCliError, receiptText } = await load('streams');
const { automaticFields, editRequest, FieldEdit } = await load('core');
const { checkpointContext, infer, changesSchema } = await load('agent');
const { chooseInitial } = await load('chooser');
const { openCard, cardSummary, compose, editField } = await load('panel');
const { validateReferences, validateChanges } = await load('core');
const { ModelRuntime, ModelRegistry, SessionManager, initTheme } = await import('@earendil-works/pi-coding-agent');
initTheme('dark');
const { createAssistantMessageEventStream } = await import('@earendil-works/pi-ai');
const { default: registerWorkCard } = await load('index');
const { Reporter } = await load('reporter');
const { defaults, validateSettings, loadSettings, saveSettings, settingsDialog } = await load('settings');
const stream = { id:'effort', title:'Work', title_mode:'manual', goal:'Goal', goal_mode:'automatic', goal_short:null, status:'active', archived:false, owner:{host_id:'host'}, repositories:{repo:{url:'https://example.com/repo'}}, issues:[], prs:[], documents:[] };
const session = { id:'session', host_id:'host', stream_id:'effort', harness:'pi', harness_session_id:'native', title:'Session', title_mode:'automatic', status:'active', state:'', state_short:null, remaining:'', checkout:null };
const context = () => structuredClone({catalog_source:'source-a',host_id:'host',stream,session,sessions:[session],authority:{host_id:'host',token:'owner'}});
const localSync = async () => ({local:'unchanged',sharing:'local-only',domain:null});

test('catalog CLI uses stdin; accepted code2 receipt is retained without retry', async () => {
 const calls=[]; const receipt={operation_id:'op',save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}},session,completion_error:'offline'};
 const cli=new StreamCli('/tmp',async(args,input)=>{calls.push({args,input});return {code:2,stdout:JSON.stringify(receipt),stderr:'offline'};});
 assert.deepEqual(await cli.apply({operation_id:'op'}),receipt);
 assert.equal(calls.length,1); assert.deepEqual(calls[0].args,['agent','apply','--json']); assert.equal(JSON.parse(calls[0].input).operation_id,'op');
});
test('idempotent enrollment with no new saves is a valid receipt, including publication',async()=>{
 const value={stream,session,authority:context().authority,saves:[],publication:{local:'unchanged',sharing:'local-only',domain:null}};
 const cli=new StreamCli('/tmp',async()=>({code:0,stdout:JSON.stringify(value),stderr:''}));
 assert.deepEqual(await cli.enroll({}),value);assert.match(receiptText(value),/Already enrolled; no new local saves/);
});
test('repair-required null-domain receipts retain uncertainty and dirty input without retry',async()=>{
 const value={operation_id:'uncertain',save:{local:'repair-required',sharing:'local-only',domain:null,detail:'interrupted save marker'}};let calls=0;const receipts=[];
 const cli=new StreamCli('/tmp',async()=>{calls++;return {code:2,stdout:JSON.stringify(value),stderr:''};});
 const edit=new FieldEdit('session',context(),'state');edit.value='Keep this input';
 await assert.rejects(edit.save(cli,receipt=>receipts.push(receipt)),/uncertain/);
 assert.equal(calls,1);assert.deepEqual(receipts,[value]);assert.equal(edit.value,'Keep this input');assert.match(receiptText(value),/interrupted save marker/);assert.match(receiptText(value),/uncertain/);
});
test('CLI rejects invalid payload and nonreceipt failures',async()=>{
 for(const stdout of ['not json','{}',JSON.stringify({error:{kind:'invalid',message:'Fix input'}})]){
  const cli=new StreamCli('/tmp',async()=>({code:2,stdout,stderr:'failed'}));
  await assert.rejects(cli.apply({}),StreamsCliError);
 }
});
test('automatic fields omit protected names and all lifecycle and identity fields',()=>{
 assert.deepEqual(automaticFields('stream',stream),['goal','goal_short']);
 assert.deepEqual(automaticFields('session',session),['title','status','state','state_short','remaining']);
});
test('request freezes original target and stream dependencies; session progress does not guard siblings or stream',()=>{
 const captured=context(); const req=editRequest(captured,'session',{state:'Done'},'automatic');
 const streamReq=editRequest(captured,'stream',{goal:'Updated'},'automatic');
 captured.session.state='later';captured.stream.goal='later';
 assert.equal(req.before.state,''); assert.deepEqual(req.inputs,[]);assert.equal(req.ownership_token,'owner');
 assert.equal(streamReq.inputs[0].kind,'session');assert.equal(streamReq.inputs[0].record.state,'');assert.equal(req.actor,'automatic');
 assert.throws(()=>editRequest(context(),'stream',{status:'done'},'automatic'),/allowed/);
});
test('dirty field retains base and failed input while clean values refresh',async()=>{
 const edit=new FieldEdit('stream',context(),'title');edit.value='';
 await assert.rejects(edit.save({apply:async()=>assert.fail('validation must precede save')}),/nonempty/);
 assert.equal(edit.value,'');assert.match(edit.error,/nonempty/);
 edit.value='Mine';const newer=context();newer.stream.title='Other';edit.refresh(newer);
 let sent;await assert.rejects(edit.save({apply:async(req)=>{sent=req;throw Error('stale; reopen field');}}),/stale/);
 assert.equal(sent.before.title,'Work');assert.equal(edit.value,'Mine');assert.match(edit.error,/stale/);
});
const turn=()=>new Promise(resolve=>setImmediate(resolve));
async function until(check){for(let i=0;i<100&&!check();i++)await turn();assert.ok(check());}
function reporterFixture(settings={}) {
 let leaf='one';const branch=['one'];const saves=[];const inferences=[];const notices=[];let release;
 const catalog=context();const ctx={cwd:'/repo',sessionManager:{getSessionId:()=> 'native',getLeafId:()=>leaf,getBranch:()=>branch.map(id=>({id})),buildSessionProjection:()=>({messages:[{role:'user',content:'Build catalog reporting'},{role:'assistant',content:[{type:'text',text:'Done'}],stopReason:'stop'}]})}};
 const cli={context:async()=>structuredClone(catalog),apply:async(req)=>{saves.push(req);Object.assign(catalog[req.kind],req.changes);return {save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}}};}};
 const reporter=new Reporter({cli:()=>cli,settings:()=>({...defaults,...settings}),infer:async(_ctx,req,signal)=>{inferences.push({req,signal});if(inferences.length===1)await new Promise(resolve=>{release=resolve;});return {changes:req.type==='session'?{state:'Reported'}:{goal:'Refined'}};},notice:text=>notices.push(text),receipt:()=>{}});
 return {reporter,ctx,saves,inferences,notices,catalog,append(){leaf='two';branch.push(leaf);},rewind(){leaf='other';branch.splice(0,branch.length,'other');},release:()=>release?.()};
}
test('response cadence coalesces and accepts old prefixes then follows up; captures metadata after session save',async()=>{
 const f=reporterFixture({frequency:2});f.reporter.response(f.ctx);assert.equal(f.inferences.length,0);
 f.reporter.response(f.ctx);await until(()=>f.inferences.length===1);f.append();
 f.reporter.response(f.ctx);f.reporter.response(f.ctx);f.reporter.response(f.ctx);f.reporter.response(f.ctx);
 assert.equal(f.inferences[0].signal.aborted,false);f.release();await f.reporter.idle();
 assert.equal(f.saves[0].kind,'session');assert.equal(f.saves[1].kind,'stream');
 assert.equal(f.saves[1].inputs[0].record.state,'Reported');assert.equal(f.inferences.length,4);
});
test('rewind and explicit invalidation cancel inference and cannot save stale evidence',async()=>{
 for(const invalidate of [false,true]){const f=reporterFixture();f.reporter.response(f.ctx);await until(()=>f.inferences.length===1);
 if(invalidate)f.reporter.invalidate();else f.rewind();f.release();await f.reporter.idle();assert.equal(f.saves.length,0);}
});
test('tree and start invalidations reset automatic reporting cadence',async()=>{
 for(const reset of [reporter=>reporter.invalidate(),(reporter,ctx)=>reporter.scope.reset({...ctx})]) {
  const f=reporterFixture({frequency:2});f.reporter.response(f.ctx);reset(f.reporter,f.ctx);
  f.reporter.response({...f.ctx});await turn();assert.equal(f.inferences.length,0);
  f.reporter.response({...f.ctx});await until(()=>f.inferences.length===1);f.release();await f.reporter.idle();assert.equal(f.saves.length,2);
 }
});
test('native, cwd and environment changes cannot reuse captured report evidence',async()=>{
 const previous=process.env.STREAMS_STREAM_ID;
 try {
  for(const change of [f=>f.ctx.cwd='/different',f=>f.ctx.sessionManager.getSessionId=()=> 'different',f=>f.reporter.scope.use({...f.ctx,cwd:'/different-event-cwd'}),()=>process.env.STREAMS_STREAM_ID='different']) {
   const f=reporterFixture();f.reporter.request(f.ctx);await until(()=>f.inferences.length===1);change(f);f.release();await f.reporter.idle();assert.equal(f.saves.length,0);
   if(previous===undefined)delete process.env.STREAMS_STREAM_ID;else process.env.STREAMS_STREAM_ID=previous;
  }
 } finally {if(previous===undefined)delete process.env.STREAMS_STREAM_ID;else process.env.STREAMS_STREAM_ID=previous;}
});
test('publication failures retain accepted saves and allow metadata, but repair or refusal stops inference',async()=>{
 for(const publication of [{local:'unchanged',sharing:'failed',domain:null},{local:'unchanged',sharing:'blocked',domain:null},{local:'unchanged',sharing:'uncertain',domain:null},{local:'repair-required',sharing:'local-only',domain:null}]) {
  const steps=[],receipts=[];const f=reporterFixture();
  const reporter=new Reporter({cli:()=>({context:async()=>context(),apply:async()=>({save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}},publication})}),settings:()=>defaults,infer:async(_ctx,request)=>{steps.push(request.type);return {changes:request.type==='session'?{state:'Saved'}:{goal:'Metadata'}};},notice:text=>assert.fail(text),receipt:value=>receipts.push(value)});
  reporter.request(f.ctx);await reporter.idle();assert.deepEqual(steps,publication.local==='repair-required'?['session']:['session','stream']);assert.equal(receipts.length,steps.length);
 }
 for(const outcome of ['conflict','superseded']) {
  const steps=[];const f=reporterFixture();
  const reporter=new Reporter({cli:()=>({context:async()=>context(),apply:async()=>({save:{local:'unchanged',sharing:'local-only',domain:{outcome}}})}),settings:()=>defaults,infer:async(_ctx,request)=>{steps.push(request.type);return {changes:{state:'Refused'}};},notice:text=>assert.fail(text),receipt:()=>{}});
  reporter.request(f.ctx);await reporter.idle();assert.deepEqual(steps,['session']);
 }
});
test('manual mode does not report automatically but explicit checkpoint does',async()=>{
 const f=reporterFixture({reporting:'manual'});f.reporter.response(f.ctx);await turn();assert.equal(f.inferences.length,0);
 f.reporter.request(f.ctx);await until(()=>f.inferences.length===1);f.release();await f.reporter.idle();assert.equal(f.saves.length,2);
});
test('settings change in place, pick searchable models, and preserve Herdr config keys',async()=>{
 const root=mkdtempSync(join(tmpdir(),'card-settings-ui-'));let factories=0;const saved=[];
 try {
  writeFileSync(join(root,'work-cards.json'),JSON.stringify({show_refresh_spinner:true,tab_naming:false}));
  const ctx={modelRegistry:{getAvailable:()=>[{provider:'z',id:'last'},{provider:'a',id:'first'}]},ui:{notify:text=>assert.fail(text),custom:async factory=>{
   factories++;return new Promise(resolve=>{
    const component=factory({terminal:{rows:30},requestRender(){}},{fg:(_color,text)=>text,bold:text=>text},{},resolve);
    const press=data=>component.handleInput(data);
    press('\r');press('\r');assert.equal(saved.at(-1).reporting,'automatic');
    // Cursor stays on reporting after each toggle; move to the model submenu.
    for(let i=0;i<4;i++)press('\x1b[B');press('\r');press('a/first');press('\r');
    assert.deepEqual(saved.at(-1).model,{provider:'a',id:'first'});
    press('\x1b[B');press('\r'); // workspace startup
    press('\x1b[B');press('\r'); // workspace naming: enabled -> disabled
    const native=JSON.parse(readFileSync(join(root,'work-cards.json'),'utf8'));
    assert.equal(native.workspace_startup,'manual');assert.equal(native.workspace_naming,false);
    assert.equal(native.tab_naming,false);assert.equal(native.show_refresh_spinner,true);
    assert.match(component.render(100).join('\n'),/Pi session naming/);
    assert.match(component.render(100).join('\n'),/Work card & Herdr settings/);press('\x1b');
   });
  }}};
  await settingsDialog(ctx,{...defaults},value=>saved.push(structuredClone(value)),root);
  assert.equal(factories,1);
 }finally{rmSync(root,{recursive:true,force:true});}
});
test('all settings explain their effects and leave description room on short terminals',async()=>{
 const root=mkdtempSync(join(tmpdir(),'card-settings-descriptions-'));
 try {
  await settingsDialog({modelRegistry:{getAvailable:()=>[]},ui:{custom:async factory=>{
   const component=factory({terminal:{rows:16},requestRender(){}},{fg:(_color,text)=>text,bold:text=>text},{},()=>{});
   for(const description of [
    'Automatically summarize your progress into the work card',
    'Automatically attach new Pi chats to a work card',
    'How many completed agent responses to wait',
    'What conversation history the summary model reads',
    'Model used to summarize progress and generate short titles.',
    'Automatically open the side-by-side Herdr work-card pane',
    "Sync the Herdr workspace name with the effort's short title",
    "Sync this Herdr tab's name with this session's short title",
    "Sync the Pi session name (/name) with this session's short title",
   ]) {
    const lines=component.render(100);assert.ok(lines.length<=16,lines.join('\n'));
    assert.ok(lines.join(' ').includes(description),lines.join('\n'));
    component.handleInput('\x1b[B');
   }
   component.handleInput('\x1b');
  }}},{...defaults},()=>assert.fail('viewing settings must not save'),root);
 }finally{rmSync(root,{recursive:true,force:true});}
});
test('automatic reset unlocks manual names before fresh inference and keeps lifecycle',async()=>{
 const catalog=context();catalog.session.title_mode='manual';catalog.stream.goal_mode='manual';
 const f=commandFixture({catalog,infer:async req=>({changes:req.type==='session'?{title:'Fresh lane',state:'Fresh state'}:{title:'Fresh effort',goal:'Fresh goal'}})});
 await f.run('reset --auto');await until(()=>f.activity.length===2);
 assert.ok(f.notices.every(notice=>notice.level==='info'));assert.deepEqual(f.requests.map(req=>req.type),['session','stream']);
 assert.ok(f.requests.every(req=>req.reset));
 assert.equal(f.requests[0].catalog.session.title_mode,'automatic');
 assert.equal(f.requests[1].catalog.stream.title_mode,'automatic');assert.equal(f.requests[1].catalog.stream.goal_mode,'automatic');
 assert.deepEqual(f.writes.slice(0,2).map(req=>({kind:req.kind,actor:req.actor,changes:req.changes})),[
  {kind:'session',actor:'user',changes:{title_mode:'automatic'}},
  {kind:'stream',actor:'user',changes:{title_mode:'automatic',goal_mode:'automatic'}},
 ]);
 assert.equal(catalog.stream.status,'active');assert.equal(catalog.stream.archived,false);
});
test('reporting settings have explicit stable knobs and reject invalid cadence',()=>{
 const root=mkdtempSync(join(tmpdir(),'card-settings-'));
 try{assert.deepEqual(loadSettings(root),defaults);const custom={reporting:'manual',enrollment:'manual',frequency:3,context:'full',model:{provider:'custom',id:'helper'}};saveSettings(root,custom);assert.deepEqual(loadSettings(root),custom);for(const frequency of [0,-1,1.5,NaN,'2'])assert.throws(()=>validateSettings({...defaults,frequency}),/positive integer/);}finally{rmSync(root,{recursive:true,force:true});}
});
test('model inference uses actual supported registry API, typed calls, no tools beyond record_card',async()=>{
 const requests=[];const ctx={model:{id:'current',contextWindow:100000},modelRegistry:{complete:async(model,payload,options)=>{requests.push({model,payload,options});return {stopReason:'toolUse',content:[{type:'toolCall',name:'record_card',arguments:{changes:{state:'Recorded'}}}]};},find:()=>undefined}};
 const value=await infer(ctx,{type:'session',catalog:context(),messages:[]},new AbortController().signal);
 assert.deepEqual(value,{changes:{state:'Recorded'}});assert.equal(requests[0].payload.tools.length,1);assert.equal(requests[0].payload.tools[0].name,'record_card');assert.doesNotMatch(JSON.stringify(requests[0].payload.messages),/source-a|\"token\"/);
 ctx.modelRegistry.find=(provider,id)=>({provider,id,contextWindow:100000});await infer(ctx,{type:'session',catalog:context(),messages:[]},new AbortController().signal,{provider:'custom',id:'chosen'});assert.equal(requests[1].model.id,'chosen');ctx.modelRegistry.find=()=>undefined;
 const schema=changesSchema('stream',context(),'automatic');assert.equal(schema.properties.title,undefined);assert.equal(schema.properties.status,undefined);assert.equal(schema.additionalProperties,false);
 await assert.rejects(infer(ctx,{type:'session',catalog:context(),messages:[]},new AbortController().signal,{provider:'missing',id:'missing'}),/unavailable/);
 ctx.modelRegistry.complete=async()=>({stopReason:'stop',content:[{type:'text',text:'{"changes":{"state":"not parsed"}}'}]});
 await assert.rejects(infer(ctx,{type:'session',catalog:context(),messages:[]},new AbortController().signal),/structured/);
 ctx.modelRegistry.complete=async()=>({stopReason:'toolUse',content:[{type:'toolCall',name:'record_card',arguments:{changes:{title_mode:'manual'}}}]});
 await assert.rejects(infer(ctx,{type:'session',catalog:context(),messages:[]},new AbortController().signal),/invalid structured/);
});
for(const failure of ['throw','response'])test(`inference retries transient gateway ${failure} and succeeds`,async()=>{
 let attempts=0;
 const ctx={model:{id:'current'},modelRegistry:{complete:async()=>{
  if(++attempts===1) {
   const errorMessage='ai-gw-openai API error (503): 503 upstream connect error or disconnect/reset before headers. reset reason: connection termination';
   if(failure==='throw')throw Error(errorMessage);
   return {stopReason:'error',errorMessage};
  }
  return {stopReason:'toolUse',content:[{type:'toolCall',name:'record_card',arguments:{changes:{state:'Recovered'}}}]};
 }}};
 assert.deepEqual(await infer(ctx,{type:'session',catalog:context(),messages:[]},new AbortController().signal),{changes:{state:'Recovered'}});
 assert.equal(attempts,2);
});
for(const failure of ['throw','response'])for(const transient of [false,true])test(`inference bounds ${transient?'transient':'permanent'} ${failure} failures`,async()=>{
 let attempts=0;const errorMessage=transient?'503 upstream connect error':'401 unauthorized';
 const ctx={model:{id:'current'},modelRegistry:{complete:async()=>{
  attempts++;if(failure==='throw')throw Error(errorMessage);
  return {stopReason:'error',errorMessage};
 }}};
 await assert.rejects(infer(ctx,{type:'session',catalog:context(),messages:[]},new AbortController().signal),new RegExp(errorMessage));
 assert.equal(attempts,transient?3:1);
});
for(const when of ['before','during','delay','response'])test(`inference aborts ${when} without retrying`,async()=>{
 let attempts=0;const controller=new AbortController();
 if(when==='before')controller.abort();
 const ctx={model:{id:'current'},modelRegistry:{complete:async()=>{
  attempts++;
  if(when==='during')controller.abort();
  if(when==='delay')setImmediate(()=>controller.abort());
  if(when==='response')return {stopReason:'aborted'};
  throw Error('503 upstream connect error');
 }}};
 await assert.rejects(infer(ctx,{type:'session',catalog:context(),messages:[]},controller.signal),/cancelled|aborted/i);
 assert.equal(attempts,when==='before'?0:1);
});
test('session progress uses the agreed brevity prompt without hard limits or changes to other helper tasks',async()=>{
 const guidance='Write state as a brief orientation for someone returning to the work: what meaningfully changed and where things stand. Use one or two short sentences, normally under 35 words; shorter is better. Do not enumerate completed steps, test counts, commit hashes, rollout inventories, or other specific details unless essential to understanding the result. Put unfinished scope and blockers in remaining. This is to provide the user a quick update/reminder of the current status, not a completion report.';
 const state=Array(40).fill('context').join(' ');const prompts={};
 for(const type of ['session','stream','enroll','discussion']){
  const answer=type==='session'?{changes:{state,remaining:'Resolve the blocker'}}:type==='enroll'?{meaningful:false}:type==='discussion'?{reply:'Discuss only'}:{changes:{}};
  const ctx={model:{contextWindow:100000},modelRegistry:{complete:async(_model,payload)=>{prompts[type]=payload.systemPrompt;return {stopReason:'toolUse',content:[{type:'toolCall',name:'record_card',arguments:answer}]};}}};
  assert.deepEqual(await infer(ctx,{type,catalog:context(),messages:[]},new AbortController().signal),answer);
 }
 assert.equal(prompts.session,`${prompts.stream}\n\n${guidance}`);
 assert.equal(prompts.enroll,prompts.stream);assert.equal(prompts.discussion,prompts.stream);
 assert.ok(!prompts.stream.includes(guidance));
});
test('inference prompt requires short handles rather than work descriptions',async()=>{
 let prompt;
 const ctx={model:{contextWindow:100000},modelRegistry:{complete:async(_model,payload)=>{prompt=payload.systemPrompt;return {stopReason:'toolUse',content:[{type:'toolCall',name:'record_card',arguments:{changes:{}}}]};}}};
 await infer(ctx,{type:'session',catalog:context(),messages:[]},new AbortController().signal);
 for(const rule of [/short, recognizable handles for the sidebar and tab bar/,/descriptions belong in goal and state/,/stream.title.*1–3 words \(target <= 20 chars\)/,/subject, project, or system/,/Never name the objective or use a sentence/,/session.title.*1–2 words, 3 max \(target <= 18 chars\)/,/specific lane, topic, or role/,/never repeat words from the effort title in the session title/,/Investigate, Implement, Explore, Disable, Fix, Organize/,/Integration, Implementation, Work, Task/,/X & Y compound lists/,/update session\.status \(active, waiting, blocked, parked, done\) from conversation evidence/,/I think we're done here/])assert.match(prompt,rule);
});
test('reset inference scrubs only target automatic text without mutating captured catalog or field modes',async()=>{
 for(const type of ['session','stream'])for(const sessionMode of ['automatic','manual'])for(const titleMode of ['automatic','manual'])for(const goalMode of ['automatic','manual']) {
  const catalog=context();Object.assign(catalog.session,{title_mode:sessionMode,state:'Prior state',state_short:'Prior short state',remaining:'Prior remaining'});
  Object.assign(catalog.stream,{title_mode:titleMode,goal_mode:goalMode,goal_short:'Prior short goal'});
  catalog.sessions=[structuredClone(catalog.session),{...catalog.session,host_id:'other-host'},{...catalog.session,id:'sibling'}];
  const before=structuredClone(catalog);let payload;
  const ctx={model:{contextWindow:100000},modelRegistry:{complete:async(_model,value)=>{payload=value;return {stopReason:'toolUse',content:[{type:'toolCall',name:'record_card',arguments:{changes:{}}}]};}}};
  await infer(ctx,{type,catalog,messages:[],reset:true},new AbortController().signal);
  const sent=JSON.parse(payload.messages.at(-1).content),visible=sent.catalog;
  assert.match(sent.reset,/all allowed automatic fields fresh from conversation evidence/);assert.match(sent.reset,/fresh short-handle title/);assert.match(sent.reset,/empty no-change delta/);
  for(const record of [visible.session,visible.sessions[0]]) {
   assert.equal(record.title,sessionMode==='automatic'?'':before.session.title);assert.equal(record.title_mode,sessionMode);
   assert.equal(record.state,'');assert.equal(record.state_short,null);assert.equal(record.remaining,'');
  }
  assert.deepEqual(visible.sessions.slice(1),before.sessions.slice(1));
  assert.deepEqual(visible.stream,{...before.stream,...(type==='stream'?{...(titleMode==='automatic'?{title:''}:{}),...(goalMode==='automatic'?{goal:null,goal_short:null}:{})}:{})});
  const properties=payload.tools[0].parameters.properties.changes.properties;
  assert.equal(Object.hasOwn(properties,'title'),(type==='session'?sessionMode:titleMode)==='automatic');
  if(type==='stream')assert.equal(Object.hasOwn(properties,'goal'),goalMode==='automatic');
  assert.equal(Object.hasOwn(properties,'title_mode'),false);assert.equal(Object.hasOwn(properties,'goal_mode'),false);
  assert.deepEqual(catalog,before);
 }
});
test('ordinary inference retains the visible generated text and has no reset instruction',async()=>{
 const catalog=context();catalog.session.state='Prior state';catalog.session.state_short='Prior short';let sent;
 const ctx={model:{contextWindow:100000},modelRegistry:{complete:async(_model,payload)=>{sent=JSON.parse(payload.messages.at(-1).content);return {stopReason:'toolUse',content:[{type:'toolCall',name:'record_card',arguments:{changes:{}}}]};}}};
 await infer(ctx,{type:'session',catalog,messages:[]},new AbortController().signal);
 assert.deepEqual(sent.catalog.session,catalog.session);assert.deepEqual(sent.catalog.sessions,catalog.sessions);assert.equal(sent.reset,undefined);
});
test('actual local Pi runtime registry and projected compaction work without live credentials',async()=>{
 const root=mkdtempSync(join(tmpdir(),'pi-runtime-card-'));
 try {
  const runtime=await ModelRuntime.create({authPath:join(root,'auth.json'),modelsPath:null,refreshOnCreate:false});
  const registry=new ModelRegistry(runtime);const model=registry.getAll()[0];assert.ok(model);
  runtime.stream=(selected,payload)=>{assert.equal(payload.tools[0].name,'record_card');const events=createAssistantMessageEventStream();const message={role:'assistant',content:[{type:'toolCall',id:'record',name:'record_card',arguments:{changes:{state:'SDK verified'}}}],api:selected.api,provider:selected.provider,model:selected.id,stopReason:'toolUse',timestamp:Date.now(),usage:{input:1,output:1,cacheRead:0,cacheWrite:0,totalTokens:2,cost:{input:0,output:0,cacheRead:0,cacheWrite:0,total:0}}};events.push({type:'done',reason:'toolUse',message});events.end();return events;};
  const manager=SessionManager.inMemory(root);const user=manager.appendMessage({role:'user',content:'Original old content',timestamp:1});manager.appendCompaction('Compacted authoritative work',user,100);manager.appendContextEdit(user,{content:'Changed by active context projection'});
  const evidence=checkpointContext(manager,'trimmed');assert.match(JSON.stringify(evidence.messages),/Compacted authoritative work/);assert.match(JSON.stringify(evidence.messages),/Changed by active context projection/);assert.doesNotMatch(JSON.stringify(evidence.messages),/Original old content/);
  const response=await infer({model,modelRegistry:registry},{type:'session',catalog:context(),messages:evidence.messages},new AbortController().signal);assert.equal(response.changes.state,'SDK verified');
 } finally {rmSync(root,{recursive:true,force:true});}
});
test('reference validation rejects unknown repos, absolute/traversal paths, malformed URLs, duplicate or arbitrary fields',()=>{
 for(const path of ['/absolute','../escape','safe/../escape','C:/absolute','back\\\\slash'])assert.throws(()=>validateReferences('documents',[{repository:'repo',ref:'main',path,description:''}],stream),/relative/);
 assert.throws(()=>validateReferences('documents',[{repository:'unknown',ref:'main',path:'doc.md',description:''}],stream),/repository/);
 for(const url of ['not URL','file:///private','javascript:alert(1)'])assert.throws(()=>validateReferences('issues',[{url,description:''}],stream),/URL|HTTP/);
 assert.throws(()=>validateReferences('prs',[{url:'https://example.com/pr',description:'',extra:'no'}],stream),/labelled/);
 assert.throws(()=>validateReferences('issues',[{url:'https://example.com/1',description:''},{url:'https://example.com/1',description:'duplicate'}],stream),/Duplicate/);
 validateReferences('documents',[{repository:'repo',ref:'main',path:'docs/plan.md',description:'Plan',added_at:'commit:known'}],stream);
 for(const added_at of ['', ' ', 42, false])assert.throws(()=>validateReferences('documents',[{repository:'repo',ref:'main',path:'docs/plan.md',description:'Plan',added_at}],stream),/added_at/);
 assert.throws(()=>validateReferences('documents',[{repository:'repo',ref:'main',path:'docs/plan.md',description:'Plan'},{repository:'repo',ref:'other',path:'docs/plan.md',description:'Other',added_at:'commit:other'}],stream),/Duplicate/);
});
test('discussion composer preserves unsent text across close/reopen without model requests',async()=>{
 const state={input:'',messages:[{role:'assistant',text:'Previous discussion'}]};const renders=[];let first=true;
 const ctx={ui:{custom:async factory=>new Promise(resolve=>{const component=factory({terminal:{rows:30},requestRender(){}},{fg:(_color,text)=>text},{},resolve);component.focused=true;if(first){component.handleInput('unsent question');first=false;}renders.push(component.render(80).join('\n'));component.handleInput('\x1b');})}};
 await compose(ctx,state);assert.equal(state.input,'unsent question');await compose(ctx,state);assert.match(renders[1],/unsent question/);assert.match(renders[1],/Previous discussion/);
});
test('background enrollment waits for meaningful work and reuses associated stream before session exists',async()=>{
 for(const meaningful of [false,true]){
  const initial=context();initial.session=null;initial.sessions=[];const requests=[],receipts=[],steps=[];
  const f=reporterFixture();const cli={context:async()=>structuredClone(initial),enroll:async request=>{requests.push(request);initial.session=structuredClone(session);initial.sessions=[initial.session];return {stream:initial.stream,session:initial.session,saves:[{operation_id:request.operation_id,save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}}}],completion_error:'Publication offline'};},apply:()=>assert.fail('partial enrollment must stop before reports')};
  const reporter=new Reporter({cli:()=>cli,settings:()=>({...defaults}),infer:async(_ctx,req)=>{steps.push(req.type);return {meaningful,session:{title:'Contribution'},stream:{title:'Must not replace associated effort'}};},notice:text=>assert.fail(text),receipt:value=>receipts.push(value)});
  reporter.response(f.ctx);await reporter.idle();assert.equal(requests.length,meaningful?1:0);
  if(meaningful){assert.equal(requests[0].stream_id,'effort');assert.equal(requests[0].stream,undefined);assert.equal(receipts[0].completion_error,'Publication offline');assert.deepEqual(steps,['enroll']);}
 }
});
test('catalog source replacement with copied logical IDs invalidates in-flight report',async()=>{
 const f=reporterFixture();f.reporter.response(f.ctx);await until(()=>f.inferences.length===1);f.catalog.catalog_source='source-b';f.release();await f.reporter.idle();assert.equal(f.saves.length,0);
 const req=editRequest(context(),'session',{state:'Old source intent'},'automatic');assert.equal(req.catalog_source,'source-a');
});
test('background inference never rebases changed catalog inputs and serializes discussion helper',async()=>{
 const f=reporterFixture();f.reporter.response(f.ctx);await until(()=>f.inferences.length===1);f.catalog.stream.goal='Edited during inference';
 let entered=false;const discussion=f.reporter.exclusive(async()=>{entered=true;assert.equal(f.saves.length,2);return 'talk';});
 assert.equal(entered,false);f.release();await discussion;
 assert.deepEqual(f.saves[0].inputs,[]);assert.equal(f.saves[1].before.goal,'Edited during inference');
});

test('bound chooser never lists or switches, and live card offers no switch/create actions',async()=>{
 const calls=[];const cli={context:async()=>context(),list:()=>assert.fail('bound chooser must not list')};
 const ctx={hasUI:true,sessionManager:{getSessionId:()=> 'native'},ui:{select:async(title,rows)=>{calls.push({title,rows});return 'Close';}}};
 assert.match((await chooseInitial(ctx,cli,{title:'Another'},()=>assert.fail())).status,/already-bound/);
 await openCard(ctx,cli,{discussion:{input:'',messages:[]},edits:new Map()},{receipt:()=>{},discuss:()=>assert.fail(),settings:()=>assert.fail(),report:()=>assert.fail()});
 assert.ok(calls[0].rows.includes('Finish this session'));assert.ok(calls[0].rows.includes('Finish whole effort'));
 assert.ok(calls[0].rows.every(row=>! /switch|create|attach/i.test(row)));assert.match(cardSummary(context()),/owner: host/);
});
for(const intent of ['reply','automatic','user'])test(`card discussion ${intent} has typed intent and no redundant confirmation`,async()=>{
 const handlers=new Map(),receipts=[],writes=[],history=[];let openPanel,submitted=false;const menu=['Discuss live card','Close'];
 const visible=context();visible.sessions.push({...session,id:'sibling',host_id:'other-host'});
 const cli={sync:localSync,context:async()=>structuredClone(visible),apply:async req=>{writes.push(req);return {operation_id:req.operation_id,save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}}};}};
 const edit=intent==='reply'?undefined:{kind:'stream',actor:intent,changes:intent==='user'?{status:'parked'}:{goal:'Current descriptive goal'}};
 registerWorkCard({on:(name,fn)=>handlers.set(name,fn),registerCommand:(_name,options)=>{openPanel=ctx=>options.handler('card',ctx);},registerTool:()=>{},registerShortcut:()=>assert.fail('no shortcuts'),appendEntry:(type,value)=>receipts.push({type,value})},{settings:{...defaults,reporting:'manual'},cli,infer:async(_ctx,req)=>{assert.equal(req.type,'discussion');assert.match(req.discussion.at(-1).text,/card question/);return {reply:'Discussed without assuming extra intent',...(edit?{edit}:{})};}});
 const ctx={mode:'tui',hasUI:true,cwd:'/repo',sessionManager:{getSessionId:()=> 'native',getLeafId:()=> 'leaf',getBranch:()=>[{id:'leaf'}],buildSessionProjection:()=>({messages:[]})},ui:{select:async()=>menu.shift(),confirm:()=>assert.fail('explicit direction needs no second confirmation'),notify:()=>{},setStatus:()=>{},custom:async factory=>new Promise(resolve=>{const component=factory({terminal:{rows:30},requestRender(){}},{fg:(_color,text)=>text,bold:text=>text},{},resolve);if(component.render(80).join("\n").includes("Whole effort")){const action=menu.shift();if(action==="Discuss live card"){for(let i=0;i<3;i++)component.handleInput("\x1b[B");component.handleInput("\r");}else component.handleInput("\x1b");return;}history.push(component.render(80).join('\n'));if(!submitted){submitted=true;component.handleInput('card question');component.handleInput('\r');}else component.handleInput('\x1b');})}};
 await handlers.get('session_start')({},ctx);await openPanel(ctx);
 assert.equal(writes.length,intent==='reply'?0:1);assert.match(history.at(-1),/Discussed without assuming extra intent/);
 if(intent!=='reply'){assert.equal(writes[0].actor,intent);assert.equal(writes[0].catalog_source,'source-a');assert.equal(writes[0].ownership_token,intent==='user'?null:'owner');assert.equal(receipts.length,1);assert.equal(writes[0].before.status,'active');assert.deepEqual(writes[0].inputs.map(input=>input.record.id),['session','sibling']);}
});
for(const failure of ['throw','abort','invalid','branch','cwd','source','attachment','shutdown','conflict','superseded','repair','completion'])test(`discussion ${failure} retains composer/history and isolates replacement source`,async()=>{
 const handlers=new Map(),renders=[],notices=[],receipts=[];let openPanel,submitted=false,menuCount=0,leaf='leaf';const catalog=context();
 const saveFailure=['conflict','superseded','repair','completion'].includes(failure);
 const cli={sync:localSync,context:async()=>structuredClone(catalog),apply:async req=>{
  assert.equal(saveFailure,true,'cancelled discussion must not write');
  return {operation_id:req.operation_id,save:{local:'persisted',sharing:'local-only',domain:{outcome:['conflict','superseded'].includes(failure)?failure:'changes'}},...(failure==='repair'?{publication:{local:'repair-required',sharing:'confirmed',domain:null}}:{}),...(failure==='completion'?{completion_error:{message:'Incomplete'}}:{})};
 }};
 registerWorkCard({on:(name,fn)=>handlers.set(name,fn),registerCommand:(_name,options)=>{openPanel=ctx=>options.handler('card',ctx);},registerTool:()=>{},registerShortcut:()=>assert.fail('no shortcuts'),appendEntry:(type,value)=>{assert.equal(saveFailure,true,'no receipt');receipts.push({type,value});}},{settings:{...defaults,reporting:'manual'},cli,infer:async(_ctx,req,signal)=>{
  assert.deepEqual(req.discussion,[{role:'user',text:'retry question'}]);
  if(failure==='throw')throw Error('Model unavailable');
  if(failure==='abort') {handlers.get('session_tree')({}, {...ctx});assert.equal(signal.aborted,true);throw Error('Aborted discussion');}
  if(failure==='branch')leaf='other-branch';
  if(failure==='cwd')ctx.cwd='/other-repo';
  if(failure==='source')catalog.catalog_source='changed-during-inference';
  if(failure==='attachment')catalog.stream.id='other-effort';
  if(failure==='shutdown') {await handlers.get('session_shutdown')({}, {...ctx});assert.equal(signal.aborted,true);throw Error('Aborted on shutdown');}
  return failure==='invalid'?{reply:42}:{reply:'Unexpected committed reply',edit:{kind:'session',actor:'automatic',changes:{state:'Must not write'}}};
 }});
 const ctx={mode:'tui',hasUI:true,cwd:'/repo',sessionManager:{getSessionId:()=> 'native',getLeafId:()=>leaf,getBranch:()=>[{id:leaf}],buildSessionProjection:()=>({messages:[]})},ui:{select:async()=>menuCount++%2===0?'Discuss live card':'Close',notify:text=>notices.push(text),setStatus:()=>{},custom:async factory=>new Promise(resolve=>{const component=factory({terminal:{rows:30},requestRender(){}},{fg:(_color,text)=>text,bold:text=>text},{},resolve);if(component.render(100).join("\n").includes("Whole effort")){if(menuCount++%2===0){for(let i=0;i<3;i++)component.handleInput("\x1b[B");component.handleInput("\r");}else component.handleInput("\x1b");return;}renders.push(component.render(100).join('\n'));if(!submitted){submitted=true;component.handleInput('retry question');component.handleInput('\r');}else component.handleInput('\x1b');})}};
 await handlers.get('session_start')({},ctx);await openPanel(ctx);
 assert.equal(receipts.length,saveFailure?1:0);
 if(saveFailure)assert.ok(notices.some(text=>text.includes('input and save receipt retained')));
 else if(!['throw','invalid'].includes(failure))assert.deepEqual(notices,[]);
 if(failure==='shutdown') {assert.equal(renders.length,1);await handlers.get('session_start')({}, {...ctx});}
 // Reopen the original attachment after a cancelled source/attachment change.
 catalog.catalog_source='source-a';catalog.stream.id='effort';
 await openPanel(ctx);assert.match(renders.at(-1),/retry question/);assert.doesNotMatch(renders.at(-1),/user: retry question|Unexpected committed reply/);
 catalog.catalog_source='replacement-source';await openPanel(ctx);assert.doesNotMatch(renders.at(-1),/retry question|Model unavailable|cancelled|typed reply/);
});
test('extension startup and card commands do not enroll or run a model; report uses completed response event',async()=>{
 const handlers=new Map(),commands=new Map(),tools=new Map();const contexts=[];
 registerWorkCard({on:(name,fn)=>handlers.set(name,fn),registerCommand:(name,options)=>commands.set(name,options),registerTool:tool=>tools.set(tool.name,tool),registerShortcut:()=>assert.fail('no shortcuts'),appendEntry:()=>{}},{settings:{...defaults,reporting:'manual'},cli:{sync:localSync,context:async()=>{contexts.push(1);return context();}},infer:()=>assert.fail('no model on view')});
 const ctx={mode:'tui',hasUI:true,sessionManager:{getSessionId:()=> 'native'},ui:{select:async()=> 'Close',notify:()=>{},setStatus:()=>{}}};
 await handlers.get('session_start')({},ctx);assert.equal(contexts.length,0);await commands.get('streams').handler('',ctx);await commands.get('streams').handler('card',ctx);assert.equal(contexts.length,4);
 assert.ok(handlers.has('agent_end'));assert.equal(handlers.has('before_agent_start'),false);
 assert.match(tools.get('checkpoint').description,/durable/);assert.match(tools.get('stream_read').description,/typed durable/);
});
function commandFixture(options={}) {
 const commands=new Map(),tools=new Map(),handlers=new Map(),notices=[],requests=[],writes=[],receipts=[],activity=[],dialogs=[];
 const catalog=options.catalog??context();let reads=0;
 const cli={context:async()=>{reads++;return structuredClone(catalog);},list:async()=>[stream],apply:async req=>{writes.push(req);Object.assign(catalog[req.kind],req.changes);return {save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}}};},enroll:async req=>{writes.push(req);catalog.stream=stream;catalog.session=session;return {stream,session,saves:[]};}};
 const ctx={mode:'tui',hasUI:true,cwd:'/repo',sessionManager:{getSessionId:()=> 'native',getLeafId:()=> 'leaf',getBranch:()=>[{id:'leaf'}],buildSessionProjection:()=>({messages:[]})},ui:{notify:(text,level)=>notices.push({text,level}),setStatus:(_key,text)=>activity.push(text),select:async(title,rows)=>{dialogs.push({title,rows});return options.select?.(title,rows)??'Close';},editor:async()=> 'Contribution',confirm:async()=>true}};
 registerWorkCard({on:(name,fn)=>handlers.set(name,fn),registerCommand:(name,value)=>commands.set(name,value),registerTool:value=>tools.set(value.name,value),registerShortcut:()=>assert.fail('no shortcuts'),appendEntry:(type,value)=>receipts.push({type,value})},{settings:{...defaults,reporting:'manual',enrollment:'manual'},cli,infer:async(_ctx,req)=>{requests.push(req);return options.infer?await options.infer(req):{changes:{}};}});
 return {commands,tools,handlers,ctx,notices,requests,writes,receipts,activity,dialogs,reads:()=>reads,run:args=>commands.get('streams').handler(args,ctx)};
}
test('only streams slash command registers; model checkpoint tool retains its interface',()=>{
 const f=commandFixture();assert.deepEqual([...f.commands.keys()],['streams']);
 for(const old of ['checkpoint','checkpoint:settings','checkpoint:attach','streams:settings','streams:attach'])assert.equal(f.commands.has(old),false);
 assert.deepEqual([...f.tools.keys()],['stream_read','checkpoint','select_stream','create_stream']);
 assert.ok(f.tools.get('checkpoint').parameters.properties.instructions);
});
test('streams completes subcommand prefixes with short descriptions but never report instructions or extra arguments',async()=>{
 const complete=commandFixture().commands.get('streams').getArgumentCompletions;
 const items=[
  {value:'card',label:'card',description:'Open the work card panel'},
  {value:'report',label:'report',description:'Save a progress update'},
  {value:'reset',label:'reset',description:'Regenerate automatic fields'},
   {value:'settings',label:'settings',description:'Configure reporting'},
  {value:'attach',label:'attach',description:'Attach an initial work card'},
  {value:'pane',label:'pane',description:"Open native pane; 'pane close' hides it"},
 ];
 assert.deepEqual(await complete(''),items);
 for(const item of items){assert.ok(item.description.length<=40);assert.doesNotMatch(item.description,/[\r\n]/);}
 for(const [prefix,value] of [['ca','card'],['rep','report'],['se','settings'],['att','attach'],['p','pane'],['report','report']])assert.deepEqual(await complete(prefix),items.filter(item=>item.value===value));
 assert.deepEqual(await complete('reset --a'),[{value:'reset --auto',label:'--auto',description:'Re-enable automatic naming and reset'}]);
 assert.equal((await complete('reset ')).length,7);
 assert.deepEqual(await complete('reset session '),[{value:'reset session --auto',label:'session --auto',description:'Re-enable session auto-naming & reset'}]);
 for(const prefix of ['unknown','report ','report settings','report\nattach','settings ','attach effort','reset extra'])assert.equal(await complete(prefix),null);
});
for(const [args,instructions] of [['report',undefined],['  report  Keep the caveat\nand settings detail  ','Keep the caveat\nand settings detail']])test(`streams reports explicitly with ${JSON.stringify(args)}`,async()=>{
 const f=commandFixture();await f.run(args);await until(()=>f.activity.length===2);
 assert.deepEqual(f.requests.map(req=>req.type),['session','stream']);
 assert.ok(f.requests.every(req=>req.instructions===instructions));assert.equal(f.dialogs.length,0);assert.equal(f.notices.length,0);
});
for(const args of ['', '  \t ', 'card', ' card '])for(const attached of [false,true])test(`streams ${JSON.stringify(args)} only views the ${attached?'attached':'unattached'} card`,async()=>{
 const catalog=context();if(!attached){catalog.stream=null;catalog.session=null;catalog.sessions=[];}
 const f=commandFixture({catalog});await f.run(args);
 assert.deepEqual(f.requests,[]);assert.deepEqual(f.writes,[]);assert.deepEqual(f.receipts,[]);assert.deepEqual(f.activity,[]);
 assert.equal(f.dialogs.length,attached?1:0);
 if(attached)assert.equal(f.dialogs[0].title,'Work card');else assert.match(f.notices[0].text,/\/streams attach/);
});
test('streams reset dispatches fresh inference for both targets without changing manual modes',async()=>{
 const catalog=context();catalog.session.title_mode='manual';catalog.stream.goal_mode='manual';
 const f=commandFixture({catalog});await f.run(' reset ');await until(()=>f.activity.length===2);
 assert.deepEqual(f.requests.map(req=>req.type),['session']);
 assert.equal(f.requests[0].reset,true);assert.equal(f.requests[0].catalog.session.title_mode,'manual');
 assert.equal(f.requests[0].catalog.stream.title_mode,'manual');assert.equal(f.requests[0].catalog.stream.goal_mode,'manual');
 assert.deepEqual(f.writes,[]);assert.deepEqual(f.notices,[]);
 const automatic=context();automatic.stream.title_mode='automatic';
 const fresh=commandFixture({catalog:automatic});await fresh.run('reset');await until(()=>fresh.activity.length===2);
 assert.deepEqual(fresh.requests.map(req=>req.type),['session','stream']);assert.ok(fresh.requests.every(req=>req.reset===true));
});
test('streams reset on an unattached conversation warns without inference, writes or enrollment',async()=>{
 for(const associated of [true,false]) {
  const catalog=context();catalog.session=null;catalog.sessions=[];if(!associated)catalog.stream=null;
  const f=commandFixture({catalog});await f.run('reset');await until(()=>f.activity.length===2);
  assert.deepEqual(f.notices,[{text:'Work card: Reset requires an attached work card. Use /streams attach first.',level:'warning'}]);
  assert.deepEqual(f.requests,[]);assert.deepEqual(f.writes,[]);assert.deepEqual(f.receipts,[]);assert.deepEqual(f.dialogs,[]);
 }
});
test('streams reset clears omitted short state and automatic short goal but preserves fresh or manual text',async()=>{
 for(const reset of [false,true])for(const goalMode of ['automatic','manual'])for(const freshShort of [false,true])for(const updateGoal of [false,true]) {
  const catalog=context();catalog.session.state_short='Stale status';catalog.stream.goal_short='Stale goal';catalog.stream.goal_mode=goalMode;catalog.stream.title_mode='automatic';
  const f=commandFixture({catalog,infer:async req=>({changes:req.type==='session'?{state:'Fresh state',...(freshShort?{state_short:'Fresh status'}:{})}:{...(goalMode==='automatic'?{...(updateGoal?{goal:'Fresh goal'}:{}),...(freshShort?{goal_short:'Fresh goal summary'}:{})}:{})}})});
  await f.run(reset?'reset':'report');await until(()=>f.activity.length===2);
  const progress=f.writes.find(req=>req.kind==='session'),metadata=f.writes.find(req=>req.kind==='stream');
  assert.equal(progress.before.state_short,'Stale status');assert.equal(progress.changes.state_short,freshShort?'Fresh status':reset?null:undefined);
  assert.equal(metadata?.changes.goal_short,goalMode==='manual'?undefined:freshShort?'Fresh goal summary':reset?null:undefined);
  for(const write of f.writes){assert.equal(write.actor,'automatic');assert.equal(Object.hasOwn(write.changes,'title_mode'),false);assert.equal(Object.hasOwn(write.changes,'goal_mode'),false);}
  assert.ok(f.notices.every(notice=>notice.level==='info'));
 }
});
test('streams reset extra rejects arguments before reading the catalog or running a model',async()=>{
 const f=commandFixture();await f.run('reset extra');await turn();
 assert.deepEqual(f.notices,[{text:"Work card: Usage: /streams [card]; /streams report [instructions]; /streams reset [session|stream|all] [--auto]; /streams settings; /streams attach; /streams pane [close]. Put report instructions after 'report'.",level:'warning'}]);
 assert.equal(f.reads(),0);for(const effects of [f.requests,f.writes,f.receipts,f.dialogs,f.activity])assert.deepEqual(effects,[]);
});
test('streams reset session and stream scopes unlock and infer only the targeted scope',async()=>{
 const sessionCatalog=context();sessionCatalog.session.title_mode='manual';sessionCatalog.stream.title_mode='manual';sessionCatalog.stream.goal_mode='manual';
 const s=commandFixture({catalog:sessionCatalog,infer:async req=>({changes:{title:'Fresh session',state:'Fresh state'}})});
 await s.run('reset session --auto');await until(()=>s.activity.length===2);
 assert.deepEqual(s.requests.map(req=>req.type),['session']);
 assert.deepEqual(s.writes.map(req=>({kind:req.kind,actor:req.actor,changes:req.changes})),[
  {kind:'session',actor:'user',changes:{title_mode:'automatic'}},
  {kind:'session',actor:'automatic',changes:{title:'Fresh session',state:'Fresh state'}},
 ]);
 assert.equal(sessionCatalog.stream.title_mode,'manual');assert.equal(sessionCatalog.stream.goal_mode,'manual');

 const streamCatalog=context();streamCatalog.session.title_mode='manual';streamCatalog.stream.title_mode='manual';streamCatalog.stream.goal_mode='manual';
 const st=commandFixture({catalog:streamCatalog,infer:async req=>({changes:{title:'Fresh effort',goal:'Fresh goal'}})});
 await st.run('reset stream --auto');await until(()=>st.activity.length===2);
 assert.deepEqual(st.requests.map(req=>req.type),['stream']);
 assert.deepEqual(st.writes.map(req=>({kind:req.kind,actor:req.actor,changes:req.changes})),[
  {kind:'stream',actor:'user',changes:{title_mode:'automatic',goal_mode:'automatic'}},
  {kind:'stream',actor:'automatic',changes:{title:'Fresh effort',goal:'Fresh goal'}},
 ]);
 assert.equal(streamCatalog.session.title_mode,'manual');
});
test('streams reset stays pending when reports coalesce while the helper is busy',async()=>{
 let release;const f=commandFixture({infer:async()=>{if(f.requests.length===1)await new Promise(resolve=>{release=resolve;});return {changes:{}};}});
 await f.run('report');await until(()=>f.requests.length===1);
 await f.run('reset');await f.run('report Keep the caveat');await f.run('');
 release();await until(()=>f.activity.length===4);
 assert.deepEqual(f.requests.map(req=>req.reset),[false,false,true,true]);
 assert.deepEqual(f.requests.slice(2).map(req=>req.instructions),['Keep the caveat','Keep the caveat']);
});
test('streams report preserves pending instructions while helper is busy',async()=>{
 let release;const f=commandFixture({infer:async()=>{if(f.requests.length===1)await new Promise(resolve=>{release=resolve;});return {changes:{}};}});
 await f.run('report');await until(()=>f.requests.length===1);
 await f.run('report Keep the deployment caveat');await f.run('');await f.run('report   ');
 release();await until(()=>f.activity.length===4);
 assert.deepEqual(f.requests.map(req=>req.instructions),[undefined,undefined,'Keep the deployment caveat','Keep the deployment caveat']);
});
test('invalid streams arguments show actionable usage without catalog, UI dialog, or model effects',async()=>{
 const f=commandFixture();
 for(const args of ['unknown','Keep the caveat','settings extra','attach effort','settings\nreport','report:now','--help','card extra']){
  await f.run(args);const notice=f.notices.at(-1);assert.equal(notice.level,'warning');
  assert.match(notice.text,/\/streams report \[instructions\]/);assert.match(notice.text,/\/streams settings/);assert.match(notice.text,/\/streams attach/);
 }
 await turn();assert.equal(f.notices.length,8);assert.equal(f.reads(),0);
 for(const effects of [f.requests,f.writes,f.receipts,f.dialogs,f.activity])assert.deepEqual(effects,[]);
});
test('streams settings dispatches to the existing reporting dialog without reporting',async()=>{
 const f=commandFixture();await f.run(' settings ');
 assert.equal(f.dialogs.length,1);assert.match(f.dialogs[0].title,/Work card & Herdr settings/);assert.ok(f.dialogs[0].rows.includes('Response frequency: 1'));
 assert.equal(f.reads(),0);assert.deepEqual(f.requests,[]);assert.deepEqual(f.writes,[]);
});
test('streams attach enrolls an initial selection but never switches a bound conversation',async()=>{
 const catalog=context();catalog.stream=null;catalog.session=null;catalog.sessions=[];
 const f=commandFixture({catalog,select:(_title,rows)=>rows.find(row=>row.startsWith('effort ·'))});
 await f.run('attach');assert.equal(f.writes.length,1);assert.equal(f.writes[0].stream_id,'effort');assert.equal(f.writes[0].actor,'user');assert.equal(f.receipts.length,1);
 const dialogs=f.dialogs.length;await f.run('attach');assert.equal(f.dialogs.length,dialogs);assert.equal(f.writes.length,1);assert.deepEqual(f.requests,[]);
});
test('unattached card directs users to streams attach',async()=>{
 const catalog=context();catalog.stream=null;catalog.session=null;
 const f=commandFixture({catalog});await f.run('card');assert.match(f.notices[0].text,/\/streams attach/);assert.doesNotMatch(f.notices[0].text,/\/checkpoint/);
 assert.deepEqual(f.requests,[]);assert.deepEqual(f.writes,[]);
});
for(const inside of [true,false])test(`real checkout CLI enroll/context/guarded apply ${inside?'inside':'outside'} repository`,async()=>{
 const root=mkdtempSync(join(tmpdir(),'streams-pi-catalog-'));const checkout=fileURLToPath(new URL('..',import.meta.url));
 const cwd=inside?checkout:join(root,'outside');mkdirSync(cwd,{recursive:true});
 const bin=join(root,'bin');mkdirSync(bin);const shim=join(bin,'streams');
 writeFileSync(shim,`#!/bin/sh\nexec uv run --project '${checkout.replaceAll("'", "'\\''")}' streams "$@"\n`);chmodSync(shim,0o755);
 const env={...process.env,HOME:root,XDG_CONFIG_HOME:join(root,'config'),XDG_DATA_HOME:join(root,'data'),XDG_STATE_HOME:join(root,'state')};
 for(const key of Object.keys(env))if(key.startsWith('STREAMS_'))delete env[key];
 const run=(args,input)=>new Promise((resolve,reject)=>{const child=spawn(shim,args,{cwd,env,stdio:['pipe','pipe','pipe']});let stdout='',stderr='';child.stdout.on('data',chunk=>stdout+=chunk);child.stderr.on('data',chunk=>stderr+=chunk);child.on('error',reject);child.on('close',code=>resolve({code,stdout,stderr}));child.stdin.on('error',error=>{if(error.code!=='EPIPE')reject(error);});child.stdin.end(input);});
 try {
  const init=await run(['init','--sync','none','--json']);assert.equal(init.code,0,init.stderr);
  assert.equal((await run(['config','set','frontend.mode','none','--json'])).code,0);
  const cli=new StreamCli(cwd,run);const empty=await cli.context('native');assert.equal(empty.session,null);assert.equal(empty.stream,null);
  const enrolled=await cli.enroll({catalog_source:empty.catalog_source,operation_id:randomUUID(),harness:'pi',harness_session_id:'native',stream:{title:'Catalog port',goal:'Safe durable reports'},session:{title:'Implement reporting'},actor:'automatic',cwd});
  assert.ok(enrolled.saves.length>=2);const captured=await cli.context('native');assert.equal(captured.stream.id,enrolled.stream.id);assert.equal(captured.session.harness_session_id,'native');assert.ok(captured.authority.token);assert.ok(captured.catalog_source);
  const again=await cli.enroll({catalog_source:empty.catalog_source,operation_id:randomUUID(),harness:'pi',harness_session_id:'native',stream_id:captured.stream.id,session:{title:'Ignored existing'},actor:'automatic',cwd});assert.deepEqual(again.saves,[]);
  const wrongSource=editRequest(captured,'session',{state:'Never write cloned source'},'automatic');wrongSource.catalog_source='different-source';await assert.rejects(cli.apply(wrongSource),/source/i);
  const human=await cli.apply(editRequest(captured,'session',{state:'Human progress'},'user'));assert.equal(human.save.domain.outcome,'changes');
  const stale=await cli.apply(editRequest(captured,'session',{state:'Stale generated progress'},'automatic'));assert.equal(stale.save.domain.outcome,'superseded');assert.equal((await cli.context('native')).session.state,'Human progress');
  const protectedTitle=await cli.apply(editRequest(await cli.context('native'),'stream',{title:'Human title'},'user'));assert.equal(protectedTitle.stream.title_mode,'manual');
  const fresh=await cli.context('native');assert.ok(!automaticFields('stream',fresh.stream).includes('title'));
  const goal=await cli.apply(editRequest(fresh,'stream',{goal:'New automatic goal'},'automatic'));assert.equal(goal.stream.title,'Human title');assert.equal(goal.stream.goal_mode,'automatic');
  await cli.enroll({catalog_source:empty.catalog_source,operation_id:randomUUID(),harness:'pi',harness_session_id:'sibling-native',stream_id:captured.stream.id,session:{title:'Sibling contribution'},actor:'user',cwd});
  const withSibling=await cli.context('native');const guardedSession=editRequest(withSibling,'session',{state:'Own progress unaffected by sibling'},'automatic');
  const guardedStream=editRequest(withSibling,'stream',{goal:'Based on all contributions'},'automatic');
  const siblingContext=await cli.context('sibling-native');await cli.apply(editRequest(siblingContext,'session',{state:'Changed sibling evidence'},'user'));
  const sessionSaved=await cli.apply(guardedSession);assert.equal(sessionSaved.save.domain.outcome,'changes');
  const superseded=await cli.apply(guardedStream);assert.equal(superseded.save.domain.outcome,'superseded');
  const independent=await cli.apply(editRequest(withSibling,'session',{title:'Independent manual edit'},'user'));assert.equal(independent.save.domain.outcome,'changes');
  const blocked=await cli.apply(editRequest(await cli.context('native'),'session',{status:'blocked'},'automatic'));assert.equal(blocked.session.status,'blocked');
  const invalid=await run(['agent','apply','--json'],'{"operation_id":"bad","kind":"stream","changes":{"repositories":{}}}');assert.equal(invalid.code,2);
  const other=await cli.enroll({catalog_source:empty.catalog_source,operation_id:randomUUID(),harness:'pi',harness_session_id:'other-native',stream:{title:'Other effort'},session:{title:'Other'},actor:'user',cwd});
  await assert.rejects(cli.context('native',other.stream.id),/move|fork|different|mismatch/i);
 } finally {rmSync(root,{recursive:true,force:true});}
});

test('structured automatic no-op remains valid but discussion empty edits cannot write',async()=>{
 let answer={changes:{}};
 const ctx={model:{contextWindow:100000},modelRegistry:{complete:async()=>({stopReason:'toolUse',content:[{type:'toolCall',name:'record_card',arguments:answer}]})}};
 assert.deepEqual(await infer(ctx,{type:'session',catalog:context(),messages:[]},new AbortController().signal),{changes:{}});
 answer={reply:'No changes',edit:{kind:'stream',actor:'user',changes:{}}};
 await assert.rejects(infer(ctx,{type:'discussion',catalog:context(),messages:[]},new AbortController().signal),/invalid structured/);
 answer={reply:'Discuss only'};assert.deepEqual(await infer(ctx,{type:'discussion',catalog:context(),messages:[]},new AbortController().signal),answer);
 const f=reporterFixture();const writes=[],steps=[];
 const reporter=new Reporter({cli:()=>({context:async()=>context(),apply:async req=>{writes.push(req);return {save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}}};}}),settings:()=>defaults,infer:async(_ctx,req)=>{steps.push(req.type);return {changes:req.type==='session'?{}:{goal:'Metadata after no-op'}};},notice:text=>assert.fail(text),receipt:()=>{}});
 reporter.request(f.ctx);await reporter.idle();assert.deepEqual(steps,['session','stream']);assert.deepEqual(writes.map(req=>req.kind),['stream']);
});
test('bare or whitespace checkpoint preserves queued explicit report instructions',async()=>{
 const f=reporterFixture();f.reporter.request(f.ctx);await until(()=>f.inferences.length===1);
 f.reporter.request(f.ctx,true,'  Keep the deployment caveat  ');f.reporter.request(f.ctx,true,'');f.reporter.request(f.ctx,true,'  ');
 f.release();await f.reporter.idle();assert.equal(f.inferences[2].req.instructions,'Keep the deployment caveat');
});
test('generated stream guards include every visible sibling while session guards only its own target',()=>{
 const catalog=context();const sibling={...session,id:'sibling',host_id:'other-host',state:'Original sibling'};
 catalog.sessions.push(structuredClone(session),sibling);
 const progress=editRequest(catalog,'session',{state:'Progress'},'automatic');
 assert.deepEqual(progress.inputs,[]);
 const metadata=editRequest(catalog,'stream',{goal:'Goal'},'user',randomUUID(),true);
 assert.deepEqual(metadata.inputs.map(input=>[input.kind,input.record.id]),[['session','session'],['session','sibling']]);
 sibling.state='Changed after capture';assert.equal(metadata.inputs[1].record.state,'Original sibling');
 assert.deepEqual(editRequest(catalog,'session',{title:'Human title'},'user',randomUUID(),false).inputs,[]);
});
test('automatic Session status is reportable; automatic Stream lifecycle remains forbidden',()=>{
 assert.ok(automaticFields('session',session).includes('status'));
 assert.doesNotThrow(()=>editRequest(context(),'session',{status:'blocked'},'automatic'));
 for(const changes of [{status:'parked'},{status:'done'},{archived:true}])assert.throws(()=>editRequest(context(),'stream',changes,'automatic'),/allowed/);
});
test('compact goal requires effective goal only when a nonnull short is submitted; empty writes rejected',()=>{
 const catalog=context();catalog.stream.goal=null;
 assert.throws(()=>editRequest(catalog,'stream',{goal_short:'Orphan'},'user'),/goal/i);
 assert.doesNotThrow(()=>editRequest(context(),'stream',{goal:null},'user'));
 assert.throws(()=>editRequest(context(),'stream',{goal:null,goal_short:'Orphan'},'user'),/goal/i);
 assert.doesNotThrow(()=>editRequest(catalog,'stream',{goal:'New goal',goal_short:'Compact'},'user'));
 assert.throws(()=>editRequest(context(),'stream',{},'user'),/empty|field|change/i);
});
test('cancelled clean editor leaves no retained badge; failed edit reopens action menu first',async()=>{
 const state={discussion:{input:'',messages:[]},edits:new Map()};const notifications=[];const cli={apply:()=>assert.fail('must not write')};
 await editField({ui:{editor:async()=>undefined}},cli,context(),'stream','title',state,()=>{});
 assert.equal(state.edits.size,0);
 const edit=new FieldEdit('session',context(),'status');edit.value='blocked';edit.error='Stale status; compare live value';state.edits.set('session:status',edit);
 let menus=0;await editField({ui:{editor:()=>assert.fail('retain before edit'),select:async(title,rows)=>{menus++;assert.match(title,/Stale status/);assert.ok(rows.includes('Discard input'));return 'Keep input and close';},notify:text=>notifications.push(text)}},cli,context(),'session','status',state,()=>{});
 assert.equal(menus,1);assert.equal(state.edits.get('session:status').value,'blocked');
});
test('retained invalid short goal is blocked before CLI and archive has one confirmation',async()=>{
 const catalog=context();catalog.stream.goal=null;const state={discussion:{input:'',messages:[]},edits:new Map()};
 const edit=new FieldEdit('stream',catalog,'goal_short');edit.value='No source goal';state.edits.set('stream:goal_short',edit);let menus=0;
 await editField({ui:{select:async(title)=>{if(menus++===0)return 'Save field';assert.match(title,/goal/i);return 'Keep input and close';},notify:()=>{}}},{apply:()=>assert.fail('invalid short must be local validation')},catalog,'stream','goal_short',state,()=>{});
 assert.equal(edit.value,'No source goal');assert.match(edit.error,/goal/);
 const actions=['Whole effort — edit fields',undefined,'Archive effort','Close'];let confirmations=0,writes=0;
 await openCard({sessionManager:{getSessionId:()=> 'native'},ui:{select:async(title,rows)=>{if(title.includes('IDs, owner'))assert.ok(rows.every(row=>!row.startsWith('goal_short:')));return actions.shift();},confirm:async()=>{confirmations++;return true;},notify:()=>{}}},{context:async()=>catalog,apply:async req=>{writes++;assert.deepEqual(req.changes,{archived:true});return {save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}}};}},state,{receipt:()=>{},discuss:()=>{},settings:()=>{},report:()=>{}});
 assert.equal(confirmations,1);assert.equal(writes,1);
});
test('invalid reference rows survive validation and failed Save without forcing the editor again',async()=>{
 const state={discussion:{input:'',messages:[]},edits:new Map()};const choices=['Add row','Done','Save field','Keep input and close'];const text=['bad URL','Reference note'];const headers=[];
 const ctx={ui:{select:async(title)=>{headers.push(title);return choices.shift();},editor:async()=>{assert.ok(text.length,'must not force editor after failed Save');return text.shift();},notify:()=>{}}};
 await editField(ctx,{apply:()=>assert.fail('invalid references must never reach CLI')},context(),'stream','issues',state,()=>{});
 const retained=state.edits.get('stream:issues');assert.deepEqual(retained.value,[{url:'bad URL',description:'Reference note'}]);assert.match(retained.error,/URL/);assert.match(headers[1],/URL/);
 await editField({ui:{select:async(title,rows)=>{assert.match(title,/URL/);assert.ok(rows.includes('Discard input'));return 'Keep input and close';}}},{apply:()=>assert.fail()},context(),'stream','issues',state,()=>{});
 assert.deepEqual(retained.value,[{url:'bad URL',description:'Reference note'}]);
});
test('document editing without repositories reports guidance instead of an empty alias selector',async()=>{
 const catalog=context();catalog.stream.repositories={};const state={discussion:{input:'',messages:[]},edits:new Map()};const choices=['Add row','Done','Keep input and close'];const notices=[];
 await editField({ui:{select:async(title,rows)=>{assert.ok(rows.length>0);return choices.shift();},notify:text=>notices.push(text)}},{apply:()=>assert.fail()},catalog,'stream','documents',state,()=>{});
 assert.match(notices[0],/repository.*CLI/i);assert.equal(state.edits.size,0);
});
test('settings validate object roots/model types and preserve unknown on-disk keys across saves',()=>{
 const root=mkdtempSync(join(tmpdir(),'card-settings-invalid-'));const path=join(root,'stream-reporting.json');
 try{
  for(const value of [null,[],false,42,'text']){writeFileSync(path,JSON.stringify(value));assert.throws(()=>loadSettings(root),/object/i);}
  for(const model of [null,[],42,{provider:42,id:'model'},{provider:'provider',id:false}]){writeFileSync(path,JSON.stringify({...defaults,model}));assert.throws(()=>loadSettings(root),/model|provider/i);}
  writeFileSync(path,JSON.stringify({...defaults,future:{keep:true}}));const loaded=loadSettings(root);
  writeFileSync(path,JSON.stringify({...defaults,future:{keep:'newer'},concurrent:'preserved'}));
  saveSettings(root,{...loaded,frequency:4});const saved=JSON.parse(readFileSync(path,'utf8'));assert.deepEqual(saved.future,{keep:'newer'});assert.equal(saved.concurrent,'preserved');assert.equal(saved.frequency,4);assert.equal(statSync(path).mode&0o777,0o600);
 }finally{rmSync(root,{recursive:true,force:true});}
});

test('trimmed projection excludes tools/thinking; full retains only complete valid batches',()=>{
 const messages=[{role:'compactionSummary',summary:'Authoritative compacted context'},{role:'user',content:'Work'},
 {role:'assistant',stopReason:'toolUse',content:[{type:'thinking',thinking:'secret'},{type:'toolCall',id:'ok'}]},
 {role:'toolResult',toolCallId:'ok',content:[{type:'text',text:'result'}]},
 {role:'assistant',stopReason:'toolUse',content:[{type:'toolCall',id:'missing'}]},
 {role:'toolResult',toolCallId:'orphan',content:[]},
 {role:'assistant',stopReason:'stop',content:[{type:'text',text:'Finished'}]}];
 const manager={buildSessionProjection:()=>({messages}),getLeafId:()=> 'leaf'};
 const trimmed=checkpointContext(manager,'trimmed');const full=checkpointContext(manager,'full');
 assert.doesNotMatch(JSON.stringify(trimmed.messages),/secret|toolCall|toolResult/);
 assert.match(JSON.stringify(trimmed.messages),/Authoritative compacted context/);
 assert.equal(full.messages.filter(m=>m.role==='toolResult').length,1);
 messages[1].content='mutation';assert.equal(trimmed.messages[1].content,'Work');
});
test('infer normalizes null remaining/state, empty nullable fields, overlong short hints, and echoed non-automatic fields',async()=>{
 const ctx=changes=>({model:{contextWindow:100000},modelRegistry:{complete:async()=>({stopReason:'toolUse',content:[{type:'toolCall',name:'record_card',arguments:{changes}}]})}});
 const sessionResult=await infer(ctx({title:'  ',state:null,state_short:`${'x'.repeat(90)}\nline`,remaining:null}),{type:'session',catalog:context(),messages:[],reset:true},new AbortController().signal);
 assert.deepEqual(sessionResult,{changes:{state:'',state_short:'x'.repeat(80),remaining:''}});
 const streamCatalog=context();streamCatalog.stream.goal=null;
 const streamResult=await infer(ctx({title:'Echoed manual',status:'active',goal:'   ',goal_short:'Orphan short'}),{type:'stream',catalog:streamCatalog,messages:[],reset:true},new AbortController().signal);
 assert.deepEqual(streamResult,{changes:{goal:null,goal_short:null}});
});
test('concurrent sibling updates that supersede an in-flight report retry against the fresh catalog and coalesce native updates once',async()=>{
 const catalog=context(),sibling={...session,id:'sibling',harness_session_id:'sibling-native',state:'Before'};
 catalog.sessions.push(sibling);
 const receipts=[],updates=[],reclaims=[];let sessionAttempts=0;
 const f=reporterFixture();
 const cli={context:async()=>structuredClone(catalog),apply:async req=>{
  if(req.kind==='session'&&++sessionAttempts===1){
   sibling.state='Concurrent sibling progress';
   return {operation_id:req.operation_id,save:{local:'unchanged',sharing:'local-only',domain:{outcome:'superseded',conflicts:['inputs'],reason:'captured catalog input changed'}}};
  }
  Object.assign(catalog[req.kind],req.changes);
  return {operation_id:req.operation_id,save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}}};
 }};
 const reporter=new Reporter({cli:()=>cli,settings:()=>defaults,infer:async(_ctx,req)=>({changes:req.type==='session'?{state:'Recovered progress'}:{goal:'Updated goal'}}),notice:text=>assert.fail(text),receipt:(value,defer)=>receipts.push({value,defer}),onUpdateNative:()=>updates.push('update'),onReclaimNames:()=>reclaims.push('reclaim')});
 reporter.request(f.ctx);await reporter.idle();
 assert.equal(sessionAttempts,2);
 assert.equal(receipts.length,2);assert.ok(receipts.every(item=>item.defer===true));
 assert.deepEqual(updates,['update']);assert.deepEqual(reclaims,[]);
 reporter.reset(f.ctx,true);await reporter.idle();
 assert.deepEqual(reclaims,['reclaim']);assert.deepEqual(updates,['update']);
});
test('failed manual field edit can rebase onto updated live catalog and save retained input',async()=>{
 const state={discussion:{input:'',messages:[]},edits:new Map()},live=context();
 const edit=new FieldEdit('session',context(),'state');edit.value='My retained edit';edit.error='Edit conflict';
 state.edits.set('session:state',edit);
 live.session.state='Concurrent update';
 let savedReq;
 await editField({ui:{editor:()=>assert.fail('must not reopen editor'),select:async(_title,rows)=>{assert.ok(rows.includes('Rebase on live field & save'));return 'Rebase on live field & save';},notify:assert.fail}},{apply:async req=>{savedReq=req;return {operation_id:req.operation_id,save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}}};}},live,'session','state',state,()=>{});
 assert.equal(savedReq.before.state,'Concurrent update');
 assert.equal(savedReq.changes.state,'My retained edit');
 assert.equal(state.edits.size,0);
});
test('receiptText formats human-readable summaries on success and background reports show Updated in the status bar without chat spam',async()=>{
 assert.equal(receiptText({operation_id:'op',save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}},session:{...session,title:'Card UX',state:'Detailed state',state_short:'UX fixes deployed'}}),'Work card updated (Card UX): UX fixes deployed');
 assert.equal(receiptText({operation_id:'op',save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}},stream:{...stream,title:'Streams',goal:'Full goal',goal_short:'Compact goal'}}),'Work card effort updated (Streams): Compact goal');
 assert.equal(receiptText({operation_id:'op',save:{local:'unchanged',sharing:'local-only',domain:{outcome:'no-change'}}}),'Work card up to date (no changes)');
 const handlers=new Map(),notices=[],statuses=[];const catalog=context();
 registerWorkCard({on:(name,fn)=>handlers.set(name,fn),registerCommand:()=>{},registerTool:()=>{},registerShortcut:()=>{},appendEntry:()=>{}},{
  settings:{...defaults,reporting:'automatic',frequency:1},
  cli:{sync:localSync,context:async()=>structuredClone(catalog),apply:async req=>{Object.assign(catalog[req.kind],req.changes);return {operation_id:req.operation_id,save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}},[req.kind]:structuredClone(catalog[req.kind])};}},
  infer:async(_ctx,req)=>({changes:req.type==='session'?{state:'Auto progress',state_short:'Auto short'}:{goal:'Auto goal'}}),
 });
 const ctx={mode:'tui',hasUI:true,cwd:'/repo',sessionManager:{getSessionId:()=> 'native',getLeafId:()=> 'leaf',getBranch:()=>[{id:'leaf'}],buildSessionProjection:()=>({messages:[{role:'user',content:'Work'},{role:'assistant',content:[{type:'text',text:'Done'}],stopReason:'stop'}]})},ui:{notify:(text,level)=>notices.push({text,level}),setStatus:(_k,text)=>statuses.push(text),theme:{fg:(_c,t)=>`<green>${t}</green>`}}};
 await handlers.get('session_start')({},ctx);
 handlers.get('agent_end')({messages:[{role:'assistant',stopReason:'stop'}]},ctx);
 await until(()=>statuses.some(s=>s&&s.includes('Updated')));
 assert.equal(notices.length,0);
 assert.equal(statuses.at(-1),'Work card · <green>✓</green> Updated');
 await handlers.get('session_shutdown')({},ctx);
});
