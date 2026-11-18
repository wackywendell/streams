import assert from 'node:assert/strict';
import test from 'node:test';
import { spawn } from 'node:child_process';
import { mkdtempSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync, chmodSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';
import { createJiti } from 'jiti';
import { childCli } from './helpers/stream-cli.mjs';
import { sessionId } from './helpers/session-id.mjs';
import { patchSchemas, withPatchSchemas } from './helpers/patch-schemas.mjs';
const jiti = createJiti(import.meta.url);
const load = (name) => jiti.import(new URL(`../extensions/stream-checkpoint/${name}.ts`, import.meta.url).pathname);
const { StreamCli, StreamsCliError, receiptText, receiptCanContinue } = await load('streams');
const { automaticReferenceRequest, referenceRequest, referenceSelector, ReferenceEdit, editRequest, FieldEdit } = await load('core');
const { isReferenceValue } = await load('wire');
let previewCli;
/** Rust's real preview, for fakes whose panel adopts retained automatic reference input. */
const rustPreview=input=>(previewCli??=childCli(mkdtempSync(join(tmpdir(),'streams-preview-')),'preview')).preview(input);
test.after(async()=>{await previewCli?.stop();});
/** A fake apply that refuses malformed references, as Rust does, and records what it accepts. */
const validatingApply=writes=>async request=>{
 for(const row of request.changes.reference_changes??[])if(row.desired!==null&&!isReferenceValue(row.desired))throw Error('invalid captured edit: invalid reference');
 writes.push(request);return {save:{local:'persisted',domain:{outcome:'changes'}}};
};
const { runReport } = await load('report-workflow');
const { checkpointContext, infer, changesSchema, buildInferencePayload, parseInferenceResponse } = await load('agent');
const { chooseInitial } = await load('chooser');
const { openCard, compose, editField } = await load('panel');
const { ModelRuntime, ModelRegistry, SessionManager, initTheme } = await import('@earendil-works/pi-coding-agent');
initTheme('dark');
const { createAssistantMessageEventStream } = await import('@earendil-works/pi-ai');
const { default: registerWorkCard } = await load('index');
const { Reporter } = await load('reporter');
const { defaults, validateSettings, defaultConfigSnapshot, reportingSettingsFromSnapshot, settingsDialog } = await load('settings');
function makeSnapshotActions(initial = defaultConfigSnapshot(), hook) {
 const snap = structuredClone(initial);
 const apply = (key, value) => {
  if (key === 'naming.stream') snap.naming.stream = value;
  else if (key === 'naming.session') snap.naming.session = value;
  else if (key === 'pi.naming.session') snap.pi.naming.session = value === 'true';
  else if (key === 'pi.naming.import_edits') snap.pi.naming.import_edits = value === 'true';
  else if (key === 'pi.reporting.mode') snap.pi.reporting.mode = value;
  else if (key === 'pi.reporting.enrollment') snap.pi.reporting.enrollment = value;
  else if (key === 'pi.reporting.frequency') snap.pi.reporting.frequency = Number(value);
  else if (key === 'pi.reporting.context') snap.pi.reporting.context = value;
  else if (key === 'pi.reporting.model') {
   const slash = value.indexOf('/');
   snap.pi.reporting.model = {provider: value.slice(0, slash), id: value.slice(slash + 1)};
  } else if (key === 'pi.reporting.session_logging') snap.pi.reporting.session_logging = value === 'true';
  else if (key === 'pi.reporting.debug_commands') snap.pi.reporting.debug_commands = value === 'true';
 };
 return {
  snap,
  setConfig: async (key, value) => {if (hook) await hook('set', key, value); apply(key, value); return structuredClone(snap);},
  unsetConfig: async key => {if (hook) await hook('unset', key); if (key === 'pi.reporting.model') snap.pi.reporting.model = null; return structuredClone(snap);},
 };
}
const stream = { id:'effort', title:'Work', title_mode:'manual', goal:'Goal', goal_mode:'automatic', goal_short:null, status:'active', owner:{host_id:'host'},revision:1,updated_at_ms:0, repositories:{repo:{url:'https://example.com/repo'}}, references:[] };
const session = { id:sessionId('host','pi','native'), host_id:'host',revision:1,updated_at_ms:0,membership_epoch:0,stream_id:'effort', harness:'pi', harness_session_id:'native', title:'Session', title_mode:'automatic', status:'active', state:'', state_short:null, remaining:'', checkout:null };
const context = () => withPatchSchemas(structuredClone({catalog_source:'source-a',host_id:'host',stream,session,sessions:[session],title_defaults:{stream:'automatic',session:'automatic'}}));
const localSync = async () => ({local:'unchanged',sharing:'local-only',domain:null});

test('typed catalog apply retains accepted warning receipts without retry or target rewriting', async () => {
 const calls=[],warnings=['Saved; projection deferred'];const receipt={save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}},session,completion_error:'offline'};
 const cli=new StreamCli('/tmp',async request=>{calls.push(request);return {status:'saved_with_warnings',result:receipt,warnings};});
 const request={};assert.deepEqual(await cli.apply(request),{...receipt,warnings});
 assert.equal(calls.length,1);assert.deepEqual(calls[0],{kind:'apply',request});assert.equal(calls[0].request,request);assert.match(receiptText({...receipt,warnings}),/Saved; projection deferred/);
});
test('idempotent enrollment with no new saves is a valid receipt, including publication',async()=>{
 const value={stream,session,saves:[],publication:{local:'unchanged',sharing:'local-only',domain:null}};
 const cli=new StreamCli('/tmp',async()=>({status:'success',result:value}));
 assert.deepEqual(await cli.enroll({}),value);assert.match(receiptText(value),/Already enrolled; no new local saves/);
});
test('a save without a domain outcome is refused and keeps dirty input without retry',async()=>{
 const value={save:{local:'repair-required',sharing:'local-only',domain:null,detail:'interrupted save marker'}};let calls=0;const receipts=[];
 const cli=new StreamCli('/tmp',async()=>{calls++;return {status:'success',result:value};});
 const edit=new FieldEdit('session',context(),'state');edit.value='Keep this input';
 await assert.rejects(edit.save(cli,receipt=>receipts.push(receipt)),/Invalid catalog receipt/);
 assert.equal(calls,1);assert.deepEqual(receipts,[]);assert.equal(edit.value,'Keep this input');
});
test('typed catalog rejects nonreceipts and preserves structured failures',async()=>{
 for(const outcome of [{status:'success',result:{}},{status:'error',error:{kind:'invalid',message:'Fix input'}}]){
  const cli=new StreamCli('/tmp',async()=>outcome);
  await assert.rejects(cli.apply({}),StreamsCliError);
 }
});
test('StreamCli validates strict catalog fields and nullable document provenance',async()=>{
 const initial=context(), id=randomUUID();initial.session.harness_session_id=id;
 initial.stream.references=[{name:'Design',purpose:'Guide design',role:'brief',state:'active',target:{kind:'document',repository_url:'https://example.com/repo',ref:'main',path:'design.md'}}];
 initial.native={frontend:'herdr',pane_id:'pane',tab_id:'tab',workspace_id:'workspace'};
 initial.checkouts=[{repository_url:'https://example.com/repo',checkout_id:'checkout',root:'/checkout'}];
 initial.document_locations=[{kind:'local',repository_url:'https://example.com/repo',path:'design.md',checkout_id:'checkout',root:'/checkout',file:'/checkout/design.md',error:null}];
 const read=value=>new StreamCli('/tmp',async()=>({status:'success',result:value})).context(id);
 for(const provenance of [{},{commit:'abc',source_session_id:'origin'},{commit:null,source_session_id:null}]) {
  const value=structuredClone(initial);Object.assign(value.stream.references[0].target,provenance);
  const app=await read(value);assert.deepEqual(app.stream,value.stream);assert.deepEqual(app.session,value.session);
 }
 for(const ref of [undefined,null,'topic']) {
  const value=structuredClone(initial);if(ref===undefined)delete value.stream.references[0].target.ref;else value.stream.references[0].target.ref=ref;
  assert.deepEqual((await read(value)).stream.references,value.stream.references);
 }
 const pinned=structuredClone(initial);pinned.session.checkout={repository:'old-app',repository_url:'https://example.test/original.git',ref:'topic',directory_within_checkout:'services/api'};
 assert.deepEqual((await read(pinned)).session,pinned.session);
 for(const membership_epoch of [0,1,5]) {const value=structuredClone(initial);value.session.membership_epoch=membership_epoch;assert.deepEqual((await read(value)).session,value.session);}
 for(const change of [
  value=>{value.session.title_mode='other';},
  ...[null,-1,1.5,'1',[1]].map(epoch=>value=>{value.session.membership_epoch=epoch;}),
  value=>{value.session.checkout={repository:'app',repository_url:42,ref:'topic',directory_within_checkout:'.'};},
  value=>{value.session.checkout={repository:'app',repository_url:'',ref:'topic',directory_within_checkout:'.'};},
  value=>{value.stream.goal_mode='other';},value=>{value.stream.status=['active'];},value=>{value.host_id='';},
  ...['',' ',42,false].map(ref=>value=>{value.stream.references[0].target.ref=ref;}),
  value=>{value.stream.references[0].target.commit='';},value=>{value.stream.references[0].target.source_session_id=42;},
  value=>{value.stream.references.push({...value.stream.references[0],target:{...value.stream.references[0].target,ref:'other'}});},
  value=>{value.stream.documents=[];},value=>{delete value.stream.references;},
  value=>{delete value.session.checkout;},value=>{delete value.stream.owner;},
 ]) {const value=structuredClone(initial);change(value);await assert.rejects(read(value),StreamsCliError);}
});
test('catalog list accepts only the current object contract',async()=>{
 const unreadable=[{kind:'stream',host:'host',path:'hosts/host/streams/broken.yaml',cause:'unknown field `archived`',repair:'Delete or fix the file.'}];
 for(const value of [[],[stream],null,{},{streams:[stream]},{streams:[{...stream,title_mode:'other'}],unreadable:[]},{streams:[stream],unreadable:[{...unreadable[0],kind:'other'}]},{streams:[stream],unreadable:[{kind:'stream',path:'p'}]},{streams:[stream],unreadable:[{...unreadable[0],repair:undefined}]},{streams:[stream],unreadable:[{...unreadable[0],host:42}]}]) {
  const cli=new StreamCli('/tmp',async()=>({status:'success',result:value}));
  await assert.rejects(cli.list(),StreamsCliError);
 }
 const cli=new StreamCli('/tmp',async()=>({status:'success',result:{streams:[stream],unreadable}}));
 assert.deepEqual(await cli.list(),{streams:[stream],unreadable});
});
test('the chooser offers readable Streams and names this host\'s records it could not read',async()=>{
 const repair='Run /streams reset in a conversation on that Stream, or delete or fix the file.';
 const unreadable=[{kind:'stream',host:'host',path:'hosts/host/streams/broken.yaml',cause:'unknown field `archived`',repair},{kind:'host',host:'other',path:'hosts/other/host.yaml',cause:'newer format',repair:'Delete or fix the file.'}];
 const calls=[];const cli={context:async()=>({...context(),stream:null,session:null,sessions:[]}),list:async()=>({streams:[stream],unreadable})};
 const ctx={hasUI:true,sessionManager:{getSessionId:()=> 'native'},ui:{select:async(title,rows)=>{calls.push({title,rows});return 'Cancel';}}};
 assert.equal((await chooseInitial(ctx,cli,{},()=>assert.fail())).status,'cancelled');
 assert.deepEqual(calls[0].rows,['effort · Work','Create…','Cancel']);
 assert.equal(calls[0].title,`Initial work card attachment\n1 unreadable record is not listed:\nhosts/host/streams/broken.yaml: unknown field \`archived\`\n ${repair}`);
 await assert.rejects(chooseInitial(ctx,cli,{streamId:'broken'},()=>assert.fail()),/exact catalog stream ID[\s\S]*broken\.yaml: unknown field/);
});
test('context is bound to the child while the caller still admits only its own Session',async()=>{
 const calls=[];const cli=new StreamCli('/tmp',async request=>{calls.push(request);return {status:'success',result:context()};});
 await cli.context('native','effort');assert.deepEqual(calls,[{kind:'context',stream_id:'effort'}]);
 await assert.rejects(cli.context('different'),/does not match this Pi session/);
});
test('title defaults use typed config intent and validate the returned policy',async()=>{
 for(const target of ['stream','session'])for(const mode of ['automatic','manual']) {
  const calls=[],title_defaults={stream:'manual',session:'automatic'};
  const cli=new StreamCli('/tmp',async request=>{calls.push(request);return {status:'success',result:{title_defaults}};});
  assert.deepEqual(await cli.setTitleDefault(target,mode),title_defaults);assert.deepEqual(calls,[{kind:'set_title_default',target,mode}]);
 }
 for(const title_defaults of [null,{}, {stream:'inherit',session:'manual'},{stream:'automatic',session:42}]) {
  const cli=new StreamCli('/tmp',async()=>({status:'success',result:{title_defaults}}));await assert.rejects(cli.setTitleDefault('session','manual'),/Invalid title defaults/);
 }
});
test('card settings use typed child requests and reject inconsistent effective modes',async()=>{
 const calls=[],value={global:'automatic',session:'inherit',effective:'automatic'};
 const cli=new StreamCli('/tmp',async request=>{calls.push(request);return {status:'success',result:{...value}};});
 assert.deepEqual(await cli.cardSettings(),value);
 assert.deepEqual(await cli.setCardMode('session','inherit'),value);
 assert.deepEqual(calls,[{kind:'card_settings'},{kind:'set_card_mode',target:'session',mode:'inherit'}]);
 for(const result of [{},{...value,global:'inherit'},{...value,session:'other'},{...value,effective:'manual'}]) {
  const invalid=new StreamCli('/tmp',async()=>({status:'success',result}));
  await assert.rejects(invalid.cardSettings(),/Invalid card settings/);
 }
});
const proposedDocument={name:'Design',purpose:'Guide the work',role:'brief',state:'active',target:{kind:'document',repository_url:'https://example.com/repo',path:'docs/design.md',ref:'main'}};
test('live-shaped reference proposals save without a portable checkout or a ref',async()=>{
 const cwd=mkdtempSync(join(tmpdir(),'streams-docs-no-ref-')),cli=childCli(cwd,'native');
 try {
  let catalog=context();catalog.session.id=sessionId('host','pi','native');catalog.sessions=[structuredClone(catalog.session)];catalog.stream.goal_mode='manual';
  catalog.checkouts=[{repository_url:'https://example.com/repo',checkout_id:'checkout',root:'/repo'}];catalog.document_locations=[];
  const initial=structuredClone(catalog),requests=[],candidate=structuredClone(proposedDocument);delete candidate.target.ref;let generations=0;
  const saved=await runReport({kind:'report',force:true},{
   signal:new AbortController().signal,automaticEnrollment:true,
   capture:async()=>({native:'native',cwd:'/repo',evidence:{messages:[{role:'user',content:'Use the design document',timestamp:0}],leafId:null},catalog:structuredClone(catalog),valid:()=>true,attached:async()=>true}),
   generate:async request=>{generations++;const payload=buildInferencePayload(request),input=JSON.parse(payload.messages.at(-1).content);
    assert.equal(input.catalog.session.checkout,null);assert.deepEqual(input.catalog.checkouts,initial.checkouts);
    assert.ok(payload.tools[0].parameters.properties.reference_proposals);assert.match(payload.systemPrompt,/without requiring a Stream alias or known branch/);
    return parseInferenceResponse(request,{content:[{type:'toolCall',name:'record_card',arguments:{changes:{state:'Explaining card settings'},reference_proposals:[candidate]}}]},payload);
   },
   apply:async request=>{requests.push(request);const projected=await cli.preview({catalog,action:'apply',request});catalog=projected.catalog;return projected.receipt;},
   enroll:assert.fail,receipt:()=>{},notice:()=>{},
  });
  assert.equal(saved,true);assert.equal(generations,1);assert.equal(requests.length,2);assert.equal(catalog.session.state,'Explaining card settings');
  assert.deepEqual(catalog.stream.references,[{...candidate,target:{...candidate.target,source_session_id:initial.session.id}}]);
  assert.deepEqual(catalog.checkouts,initial.checkouts);
 } finally {await cli.stop();rmSync(cwd,{recursive:true,force:true});}
});
test('Session envelopes defer malformed reference proposals while enforcing progress and root shape',()=>{
 const request={type:'session',catalog:context(),messages:[]}, response=args=>({content:[{type:'toolCall',name:'record_card',arguments:args}]});
 const answer={changes:{state:'Saved'},reference_proposals:[proposedDocument]};assert.deepEqual(parseInferenceResponse(request,response(answer)),answer);
 assert.equal(Object.hasOwn(buildInferencePayload(request).tools[0].parameters.properties.changes.properties,'references'),false);
 assert.equal(Object.hasOwn(buildInferencePayload({...request,type:'stream'}).tools[0].parameters.properties,'reference_proposals'),false);
 assert.throws(()=>parseInferenceResponse({...request,type:'stream'},response(answer)),/invalid structured/);
 for(const proposals of [null,{},[null],[{...proposedDocument,target:{...proposedDocument.target,commit:null}}], [{...proposedDocument,extra:true}]]) {
  const value={changes:{state:'Saved'},reference_proposals:proposals};assert.deepEqual(parseInferenceResponse(request,response(value)),value);
  if(!Array.isArray(proposals))assert.throws(()=>automaticReferenceRequest(request.catalog,proposals,'op'));
 }
 assert.throws(()=>parseInferenceResponse(request,response({...answer,extra:true})),/invalid structured/);
 assert.throws(()=>parseInferenceResponse(request,response({changes:{status:'invalid'},reference_proposals:null})),/invalid structured/);
 const sent=automaticReferenceRequest(request.catalog,[proposedDocument],'op');request.catalog.session.state='Later';
 assert.equal(sent.kind,'stream');assert.equal(sent.actor,'automatic');assert.equal(sent.inputs[0].record.state,'');
 assert.deepEqual(sent.changes.reference_changes,[{key:{kind:'document',repository_url:'https://example.com/repo',path:'docs/design.md'},desired:proposedDocument}]);
});
test('trimmed evidence retains completed file paths, including nested codemode metadata, but no bodies or results',()=>{
 const calls=[{type:'toolCall',id:'read',name:'read',arguments:{path:'docs/read.md',content:'Secret argument'}},{type:'toolCall',id:'write',name:'functions.write',arguments:{path:'docs/write.md',content:'Secret body'}}];
 const messages=[
  {role:'assistant',stopReason:'toolUse',content:[{type:'thinking',thinking:'Secret thought'},...calls]},
  ...calls.map(call=>({role:'toolResult',toolCallId:call.id,toolName:call.name,content:[{type:'text',text:'Secret result'}]})),
  {role:'assistant',stopReason:'toolUse',content:[{type:'toolCall',id:'nested',name:'codemode',arguments:{code:'Secret code'}}]},
  {role:'toolResult',toolCallId:'nested',toolName:'codemode',content:[{type:'text',text:'Secret nested result'}],nestedCalls:{calls:[{name:'edit',arguments:{path:'docs/nested.md',oldText:'Secret original',newText:'Secret replacement'}},{name:'read',arguments:{path:'bad\npath'}}]}},
  {role:'assistant',stopReason:'pending',content:[{type:'toolCall',id:'pending',name:'read',arguments:{path:'docs/pending.md'}}]},
  {role:'toolResult',toolCallId:'orphan',toolName:'codemode',content:[],nestedCalls:{calls:[{name:'read',arguments:{path:'docs/orphan.md'}}]}},
 ];
 const evidence=checkpointContext({buildSessionProjection:()=>({messages}),getLeafId:()=>null},'trimmed');
 const text=JSON.stringify(evidence);
 for(const path of ['docs/read.md','docs/write.md','docs/nested.md']) assert.ok(text.includes(path));
 assert.doesNotMatch(text,/Secret|pending.md|orphan.md|bad|toolCall|toolResult/);
 assert.match(text,/not proof of success/);
});
test('Session reporting saves progress and reference proposals separately from the same captured base with one inference',async()=>{
 for(const progress of [false,true]) {
  const catalog=context();catalog.stream.goal_mode='manual';const initial=structuredClone(catalog),requests=[],generated=[],receipts=[];
  const saved=await runReport({kind:'report',force:true},{
   signal:new AbortController().signal,automaticEnrollment:true,
   capture:async()=>({native:'native',cwd:'/repo',evidence:{messages:[],leafId:null},catalog:structuredClone(catalog),valid:()=>true,attached:async()=>true}),
   generate:async request=>{generated.push(request);return {changes:progress?{state:'Working'}:{},reference_proposals:[proposedDocument]};},
   apply:async request=>{requests.push(request);if(request.kind==='session')catalog.session.state=request.changes.state;return {save:{local:'persisted',domain:{outcome:'changes'}}};},
   enroll:assert.fail,receipt:value=>receipts.push(value),notice:assert.fail,
  });
  assert.equal(saved,true);assert.equal(generated.length,1);assert.equal(requests.length,progress?2:1);assert.equal(receipts.length,requests.length);
  const docs=requests.at(-1);assert.deepEqual(docs.before,initial.stream);assert.deepEqual(docs.inputs,[{kind:'session',record:initial.session}]);
  assert.deepEqual(docs.changes.reference_changes,[{key:{kind:'document',repository_url:'https://example.com/repo',path:'docs/design.md'},desired:proposedDocument}]);
 }
});
test('a refused reference save keeps the progress receipt and does not retry or run Stream inference',async()=>{
 const requests=[],receipts=[],catalog=context();let generated=0;
 assert.equal(await runReport({kind:'report',force:true},{
  signal:new AbortController().signal,automaticEnrollment:true,
  capture:async()=>({native:'native',cwd:'/repo',evidence:{messages:[],leafId:null},catalog,valid:()=>true,attached:async()=>true}),
  generate:async()=>{generated++;return {changes:{state:'Working'},reference_proposals:[proposedDocument]};},
  apply:async request=>{requests.push(request);return {save:{local:'persisted',domain:{outcome:requests.length===1?'changes':'superseded'}}};},
  enroll:assert.fail,receipt:value=>receipts.push(value),notice:assert.fail,
 }),true);
 assert.equal(generated,1);assert.equal(requests.length,2);assert.equal(receipts.length,2);
 assert.equal(receipts[0].save.domain.outcome,'changes');assert.equal(receipts[1].save.domain.outcome,'superseded');
});

test('a reset discards an unreadable card before capturing it and enrolls again when the Session was discarded',async()=>{
 const reason='Session record hosts/host/conversations/s.yaml is unreadable: missing field `title`';
 const attached=context(),catalog={...context(),stream:null,session:null,sessions:[]},events=[],notices=[];
 const saved=await runReport({kind:'reset',target:'all',auto:false},{signal:new AbortController().signal,automaticEnrollment:false,
  discardUnreadable:async()=>{events.push('discard');return {session:reason,stream:null};},
  capture:async()=>{events.push('capture');return {native:'native',cwd:'/repo',evidence:{messages:[],leafId:null},catalog:structuredClone(catalog),valid:()=>true,attached:async()=>true};},
  generate:async request=>{events.push(request.type);return request.type==='enroll'?{meaningful:false,stream:{title:'Work'},session:{title:'Lane'}}:{changes:{}};},
  enroll:async request=>{events.push('enroll');assert.equal(request.harness_session_id,'native');Object.assign(catalog,structuredClone(attached));return {saves:[{target:'session:native',save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}}}]};},
  apply:assert.fail,receipt:()=>{},notice:(text,level)=>notices.push({text,level}),
 });
 assert.equal(saved,true);
 assert.deepEqual(events,['discard','capture','enroll','enroll','capture','session','capture','stream']);
 assert.deepEqual(notices,[{text:`${reason}. Reset discarded it.`,level:'warning'}]);
});
test('a Stream reset keeps an automatic goal the model omitted and clears its short form',async()=>{
 const catalog=context();Object.assign(catalog.stream,{goal:'Goal',goal_mode:'automatic',goal_short:'Short'});const writes=[];
 await runReport({kind:'reset',target:'stream',auto:false},{signal:new AbortController().signal,automaticEnrollment:true,
  discardUnreadable:async()=>({session:null,stream:null}),
  capture:async()=>({native:'native',cwd:'/repo',evidence:{messages:[],leafId:null},catalog:structuredClone(catalog),valid:()=>true,attached:async()=>true}),
  generate:async()=>({changes:{}}),enroll:assert.fail,
  apply:async request=>{writes.push(request);return {save:{local:'persisted',domain:{outcome:'changes'}}};},receipt:()=>{},notice:()=>{},
 });
 assert.deepEqual(writes.map(request=>[request.kind,request.changes]),[['stream',{goal_short:null}]]);
});
test('a reset of a readable card discards nothing and says nothing',async()=>{
 const notices=[];let discards=0;
 await runReport({kind:'reset',target:'session',auto:false},{signal:new AbortController().signal,automaticEnrollment:true,
  discardUnreadable:async()=>{discards++;return {session:null,stream:null};},
  capture:async()=>({native:'native',cwd:'/repo',evidence:{messages:[],leafId:null},catalog:context(),valid:()=>true,attached:async()=>true}),
  generate:async()=>({changes:{}}),enroll:assert.fail,apply:assert.fail,receipt:()=>{},notice:(text,level)=>notices.push({text,level}),
 });
 assert.equal(discards,1);assert.deepEqual(notices,[]);
});
test('malformed deferred references leave saved progress and its receipt intact',async()=>{
 for(const proposals of [null,{},[null],[{...proposedDocument,target:{...proposedDocument.target,commit:null}}]])for(const progress of [false,true]) {
  const catalog=context(),receipts=[],notices=[],requests=[],tasks=[];let captures=0;
  const saved=await runReport({kind:'report',force:true},{signal:new AbortController().signal,automaticEnrollment:true,
   capture:async()=>{captures++;return {native:'native',cwd:'/repo',evidence:{messages:[],leafId:null},catalog:structuredClone(catalog),valid:()=>true,attached:async()=>true};},
   generate:async request=>{tasks.push(request.type);return {changes:progress?{state:'Saved'}:{},reference_proposals:proposals};},
   // Rust's preview refuses the malformed references; accepted requests save.
   apply:async request=>{await rustPreview({catalog,action:'apply',request});requests.push(request);return {save:{local:'persisted',domain:{outcome:'changes'}}};},enroll:assert.fail,
   receipt:value=>receipts.push(value),notice:(text,level)=>notices.push({text,level}),
  });
  assert.equal(saved,progress);assert.deepEqual(tasks,['session']);assert.equal(captures,1);assert.equal(requests.length,progress?1:0);assert.equal(receipts.length,progress?1:0);
  assert.equal(notices.length,1);assert.equal(notices[0].level,'warning');assert.match(notices[0].text,/Reference update failed/);
 }
});
test('partial reference receipt warns and stops after accepted independent rows',async()=>{
 for(const local of ['persisted','unchanged']) {
  const catalog=context(),notices=[],receipts=[],tasks=[];let calls=0,captures=0;
  const addition={name:'New\u001bname',purpose:'New resource',role:'deliverable',state:'active',target:{kind:'pr',url:'https://example.com/new'}};
  const dropped={...proposedDocument,state:'active'};
  catalog.stream.references=[{...proposedDocument,state:'dropped'}];
  const saved=await runReport({kind:'report',force:true},{signal:new AbortController().signal,automaticEnrollment:true,
   capture:async()=>{captures++;return {native:'native',cwd:'/repo',evidence:{messages:[],leafId:null},catalog:structuredClone(catalog),valid:()=>true,attached:async()=>true};},
   generate:async request=>{tasks.push(request.type);return {changes:{},reference_proposals:[dropped,addition]};},
   apply:async request=>{calls++;const rows=request.changes.reference_changes;return {save:{local,domain:{outcome:'changes',reference_results:[{key:rows[0].key,desired:rows[0].desired,action:'restore',outcome:'superseded',reason:'reference is Dropped; explicit restoration is required'},{key:rows[1].key,desired:rows[1].desired,action:'add',outcome:'changes',reason:''}]}}};},
   enroll:assert.fail,receipt:value=>receipts.push(value),notice:(text,level)=>notices.push({text,level}),
  });
  assert.equal(saved,true);assert.equal(calls,1);assert.equal(captures,1);assert.deepEqual(tasks,['session']);assert.equal(receipts.length,1);
  assert.deepEqual(notices,local==='persisted'?[{text:'Reference added: "New\\u001bname"',level:'info'}]:[]);
 }
});
test('reference construction preserves exact captured values and actor boundaries',async()=>{
 const catalog=context(),value={name:'Status',purpose:'Guide rollout',role:'background',state:'active',target:{kind:'link',url:'https://EXAMPLE.com/x'}};
 const key={kind:'url',url:value.target.url};
 assert.equal(automaticReferenceRequest(catalog,undefined,'unused'),undefined);assert.equal(automaticReferenceRequest(catalog,[],'unused'),undefined);
 const auto=automaticReferenceRequest(catalog,[value],'replay:capture:0');assert.deepEqual(auto.inputs,[{kind:'session',record:catalog.session}]);
 catalog.session.state='Changed';catalog.stream.title='Changed';assert.equal(auto.inputs[0].record.state,'');assert.equal(auto.before.title,'Work');
 const noSession=context();noSession.session=null;noSession.sessions=[];
 assert.deepEqual(referenceRequest(noSession,'user',[{key,desired:value}],'id').inputs,[]);
 const edit=new ReferenceEdit(context(),key,'user',value),receipts=[];let calls=0;
 const refused={save:{local:'unchanged',domain:{outcome:'conflict',reference_results:[{key,action:'add',outcome:'conflict',reason:'reference already exists'}]}}};
 await assert.rejects(edit.save({apply:async()=>{calls++;return refused;}},value=>receipts.push(value)),/not applied/);
 assert.equal(calls,1);assert.equal(edit.dirty,true);assert.deepEqual(edit.desired,value);assert.deepEqual(receipts,[refused]);
});
test('reference application exceptions warn once without losing progress',async()=>{
 const catalog=context(),receipts=[],notices=[],tasks=[];let calls=0,captures=0;
 const saved=await runReport({kind:'report',force:true},{signal:new AbortController().signal,automaticEnrollment:true,
  capture:async()=>{captures++;return {native:'native',cwd:'/repo',evidence:{messages:[],leafId:null},catalog:structuredClone(catalog),valid:()=>true,attached:async()=>true};},
  generate:async request=>{tasks.push(request.type);return {changes:{state:'Saved'},reference_proposals:[proposedDocument]};},
  apply:async request=>{if(++calls===2)throw Error('reference save failed');return {save:{local:'persisted',domain:{outcome:'changes'}}};},
  enroll:assert.fail,receipt:value=>receipts.push(value),notice:(text,level)=>notices.push({text,level}),
 });
 assert.equal(saved,true);assert.equal(calls,2);assert.equal(captures,1);assert.equal(receipts.length,1);assert.deepEqual(tasks,['session']);assert.deepEqual(notices,[{text:'Reference update failed: reference save failed',level:'warning'}]);
});
test('request freezes original target and stream dependencies; session progress does not guard siblings or stream',()=>{
 const captured=context(); const req=editRequest(captured,'session',{state:'Done'},'automatic');
 const streamReq=editRequest(captured,'stream',{goal:'Updated'},'automatic');
 captured.session.state='later';captured.stream.goal='later';
 assert.equal(req.before.state,''); assert.deepEqual(req.inputs,[]);assert.equal(req.title_default,'automatic');assert.equal(req.version,2);assert.equal(Object.hasOwn(req,'ownership_token'),false);
 assert.equal(streamReq.inputs[0].kind,'session');assert.equal(streamReq.inputs[0].record.state,'');assert.equal(req.actor,'automatic');
});
test('dirty field retains base and failed input while clean values refresh',async()=>{
 const edit=new FieldEdit('stream',context(),'title');edit.value='';
 await assert.rejects(edit.save({apply:async()=>{throw Error('invalid captured edit: stream.title must be nonempty');}}),/nonempty/);
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
 return {reporter,ctx,cli,saves,inferences,notices,catalog,append(){leaf='two';branch.push(leaf);},rewind(){leaf='other';branch.splice(0,branch.length,'other');},release:()=>release?.()};
}
test('Reporter forwards reference notice levels only through a live fence',async()=>{
 for(const invalidate of [false,true]) {
  const f=reporterFixture(),catalog=context(),notices=[],receipts=[];catalog.stream.goal_mode='manual';let release,started=false;
  const proposal={name:'New',purpose:'Guide work',role:'background',state:'active',target:{kind:'link',url:'https://example.com/new'}};
  const reporter=new Reporter({cli:()=>({context:async()=>structuredClone(catalog),apply:async request=>{
   started=true;await new Promise(resolve=>release=resolve);const row=request.changes.reference_changes[0];
   return {save:{local:'persisted',domain:{outcome:'changes',reference_results:[{key:row.key,desired:row.desired,action:'add',outcome:'changes',reason:''}]}}};
  }}),settings:()=>defaults,infer:async()=>({changes:{},reference_proposals:[proposal]}),notice:(text,level)=>notices.push({text,level}),receipt:value=>receipts.push(value)});
  reporter.request(f.ctx);await until(()=>started);if(invalidate)reporter.scope.reset(f.ctx,true);release();await reporter.idle();
  assert.deepEqual(notices,invalidate?[]:[{text:'Reference added: New',level:'info'}]);assert.equal(receipts.length,invalidate?0:1);
 }
});
test('response cadence coalesces and accepts old prefixes then follows up; captures metadata after session save',async()=>{
 const f=reporterFixture({frequency:2});f.reporter.response(f.ctx);assert.equal(f.inferences.length,0);
 f.reporter.response(f.ctx);await until(()=>f.inferences.length===1);f.append();
 f.reporter.response(f.ctx);f.reporter.response(f.ctx);f.reporter.response(f.ctx);f.reporter.response(f.ctx);
 assert.equal(f.inferences[0].signal.aborted,false);f.release();await f.reporter.idle();
 assert.equal(f.saves[0].kind,'session');assert.equal(f.saves[1].kind,'stream');
 assert.equal(f.saves[1].inputs[0].record.state,'Reported');assert.equal(f.inferences.length,4);
});
test('reporting reads an outside attachment without restarting the Pi extension',async()=>{
 const f=reporterFixture({enrollment:'manual'}),bound=context();
 Object.assign(f.catalog,{stream:null,session:null,sessions:[]});
 f.reporter.response(f.ctx);await f.reporter.idle();
 assert.deepEqual(f.inferences,[]);assert.deepEqual(f.saves,[]);
 Object.assign(f.catalog,bound);
 f.reporter.response(f.ctx);await until(()=>f.inferences.length===1);
 assert.equal(f.inferences[0].req.type,'session');assert.deepEqual(f.inferences[0].req.catalog.session,bound.session);
 f.release();await f.reporter.idle();
 assert.deepEqual(f.saves.map(request=>request.kind),['session','stream']);assert.deepEqual(f.notices,[]);
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
test('membership moves submit the original Session result to the CLI and never start destination metadata',async()=>{
 const previous=process.env.STREAMS_STREAM_ID;process.env.STREAMS_STREAM_ID='effort';
 try {
  for(const outcome of ['changes','conflict','superseded']) {
   const f=reporterFixture(),captured=context(),reads=[];
   f.cli.context=async(...args)=>{reads.push(args);return structuredClone(f.catalog);};
   f.cli.apply=async req=>{f.saves.push(req);return {save:{local:outcome==='changes'?'persisted':'unchanged',sharing:'local-only',domain:{outcome}}};};
   f.reporter.request(f.ctx);await until(()=>f.inferences.length===1);
   f.catalog.stream.id='destination';f.catalog.session.stream_id='destination';f.catalog.session.membership_epoch=1;
   f.release();await f.reporter.idle();
   assert.equal(f.saves.length,1);const request=f.saves[0];
   assert.deepEqual(request,editRequest(captured,'session',{state:'Reported'},'automatic'));
   assert.deepEqual(f.inferences.map(item=>item.req.type),['session']);
   assert.ok(reads.length>=2);assert.ok(reads.every(args=>args.length===1&&args[0]==='native'));
   assert.equal(f.catalog.stream.goal,'Goal');
  }
 } finally {if(previous===undefined)delete process.env.STREAMS_STREAM_ID;else process.env.STREAMS_STREAM_ID=previous;}
});
test('membership moves during shared inference retain the old Stream target and dependencies for CLI refusal',async()=>{
 for(const outcome of ['conflict','superseded'])for(const destination of ['destination','effort']) {
  const f=reporterFixture(),catalog=context(),captured=context(),requests=[],writes=[],receipts=[];
  const reporter=new Reporter({cli:()=>({context:async()=>structuredClone(catalog),apply:async req=>{writes.push(req);return {save:{local:'unchanged',sharing:'local-only',domain:{outcome}}};}}),settings:()=>defaults,infer:async(_ctx,req)=>{
   requests.push(req);if(req.type==='session')return {changes:{}};
   catalog.stream.id=destination;catalog.session.stream_id=destination;catalog.session.membership_epoch=destination==='effort'?2:1;
   return {changes:{goal:'Old effort result'}};
  },notice:assert.fail,receipt:value=>receipts.push(value)});
  reporter.request(f.ctx);await reporter.idle();
  assert.deepEqual(requests.map(req=>req.type),['session','stream']);assert.equal(writes.length,1);
  assert.deepEqual(writes[0],editRequest(captured,'stream',{goal:'Old effort result'},'automatic'));
  assert.equal(receipts.length,1);assert.equal(catalog.stream.goal,'Goal');
 }
});
test('membership moves do not weaken catalog source, conversation or branch fences',async()=>{
 for(const change of [f=>{f.catalog.catalog_source='replacement-source';},f=>{f.catalog.session.id='different-session';},f=>f.rewind(),f=>f.reporter.invalidate()]) {
  const f=reporterFixture();f.reporter.request(f.ctx);await until(()=>f.inferences.length===1);
  f.catalog.stream.id='destination';f.catalog.session.stream_id='destination';change(f);
  f.release();await f.reporter.idle();assert.equal(f.saves.length,0);
 }
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
test('settings change in place, pick searchable models, and mutate via child config actions',async()=>{
 let factories=0;const saved=[];
 const actions=makeSnapshotActions(defaultConfigSnapshot(),(op,key,value)=>saved.push([op,key,value]));
 const ctx={modelRegistry:{getAvailable:()=>[{provider:'z',id:'last'},{provider:'a',id:'first'}]},ui:{notify:text=>assert.fail(text),custom:async factory=>{
  factories++;
  let done;const closed=new Promise(resolve=>{done=resolve;});
  const component=factory({terminal:{rows:30},requestRender(){}},{fg:(_color,text)=>text,bold:text=>text},{},done);
  const press=data=>component.handleInput(data);
  press('\r');await turn();press('\r');await turn();
  // Move to pi.reporting.model submenu and select a/first
  for(let i=0;i<4;i++)press('\x1b[B');press('\r');press('a/first');press('\r');await turn();
  press('\x1b[B');press('\r'); // Debug subsection
  press('\x1b[B');press('\r');await turn(); // pi.reporting.session_logging -> true
  press('\x1b');
  press('\x1b[B');press('\r'); // Naming subsection
  press('\x1b[B');press('\x1b[B');press('\x1b[B');press('\r');await turn(); // pi.naming.session -> false
  assert.match(component.render(100).join('\n'),/pi\.naming\.session/);
  press('\x1b');press('\x1b');
  await closed;
 }}};
 await settingsDialog(ctx,defaultConfigSnapshot(),actions);
 await turn();
 assert.equal(factories,1);
 assert.deepEqual(saved,[
  ['set','pi.reporting.mode','manual'],
  ['set','pi.reporting.mode','automatic'],
  ['set','pi.reporting.model','a/first'],
  ['set','pi.reporting.session_logging','true'],
  ['set','pi.naming.session','false'],
 ]);
 assert.deepEqual(actions.snap.pi.reporting.model,{provider:'a',id:'first'});
 assert.equal(actions.snap.pi.reporting.session_logging,true);
 assert.equal(actions.snap.pi.naming.session,false);
});
test('all settings explain their effects and leave description room on short terminals',async()=>{
 const actions=makeSnapshotActions();
 await settingsDialog({modelRegistry:{getAvailable:()=>[]},ui:{custom:async factory=>{
  const component=factory({terminal:{rows:16},requestRender(){}},{fg:(_color,text)=>text,bold:text=>text},{},()=>{});
  for(const description of [
   'Automatically summarize your progress into the work card',
   'Automatically attach new Pi chats to a work card',
   'How many completed agent responses to wait',
   'What conversation history the summary model reads',
   'Model used to summarize progress and generate short titles.',
   'Configure debug commands and optional helper session logging.',
   'Configure title generation, incoming Pi name edits and outgoing Pi synchronization.',
   'Default for Stream titles in inherit mode.',
   'Default for Session titles in inherit mode.',
   'Save changed Pi /name edits as manual Session titles.',
   'Mirror the saved Session title in Pi.',
  ]) {
   const lines=component.render(100);assert.ok(lines.length<=16,lines.join('\n'));
   assert.doesNotMatch(lines.join(' '),/herdr\./);
   assert.ok(lines.join(' ').includes(description),lines.join('\n'));
   component.handleInput(description.startsWith('Configure title generation')?'\r':'\x1b[B');
  }
  component.handleInput('\x1b');component.handleInput('\x1b');
 }}},defaultConfigSnapshot(),actions);
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
 assert.equal(catalog.stream.status,'active');
});
test('reporting settings have explicit stable knobs and reject invalid cadence',()=>{
 const custom={...defaults,reporting:'manual',enrollment:'manual',frequency:3,context:'full',model:{provider:'custom',id:'helper'},session_logging:true};
 validateSettings(custom);
 for(const frequency of [0,-1,1.5,NaN,'2'])assert.throws(()=>validateSettings({...defaults,frequency}),/positive integer/);
});
test('helper transport hooks are absent unless inference observation is requested',async()=>{
 const ctx={model:{api:'anthropic-messages'},modelRegistry:{complete:async(_model,_payload,options)=>{
  for(const name of ['onPayload','onResponse','onProviderStreamEvent'])assert.equal(Object.hasOwn(options,name),false);
  return {stopReason:'toolUse',content:[{type:'toolCall',name:'record_card',arguments:{changes:{}}}]};
 }}};
 assert.deepEqual(await infer(ctx,{type:'session',catalog:context(),messages:[]},new AbortController().signal),{changes:{}});
});
test('helper transport logs copy converted requests and allowlisted headers without mutating provider data',async()=>{
 const root=mkdtempSync(join(tmpdir(),'card-helper-transport-')),created=[];
 try {
  const wire={model:'provider-model',system:[{type:'text',text:'Exact system'}],messages:[{role:'user',content:'Full conversation'},{role:'assistant',content:[{type:'thinking',thinking:'hidden reasoning',signature:'opaque secret'}]}],tools:[{name:'record_card',input_schema:{type:'object'}}],max_tokens:1024,api_key:'credential secret',metadata:{Authorization:'nested credential'},headers:{'x-api-key':'header secret'},thinking:{type:'adaptive'}};
  const before=structuredClone(wire),headers={'X-Request-ID':'request-123','x-amzn-requestid':'bedrock-456','Content-Type':'text/event-stream','Retry-After':'2','Authorization':'header credential','set-cookie':'cookie secret','x-debug-token':'token secret'};
  const model={api:'anthropic-messages',provider:'mock',id:'helper'};
  const ctx={cwd:root,sessionManager:SessionManager.inMemory(root),model,modelRegistry:{complete:async(_model,_payload,options)=>{
   assert.equal(await options.onPayload(wire,model),undefined);
   await options.onResponse({status:200,headers},model);
   assert.deepEqual(wire,before);assert.equal(headers.Authorization,'header credential');
   return {role:'assistant',stopReason:'toolUse',content:[{type:'toolCall',name:'record_card',arguments:{changes:{}}}],timestamp:1};
  }}};
  assert.deepEqual(await infer(ctx,{type:'session',catalog:context(),messages:[]},new AbortController().signal,undefined,{directory:root,onCreate:value=>created.push(value)}),{changes:{}});
  const entries=SessionManager.open(created[0].session_file).getEntries(),request=entries.find(e=>e.customType==='stream-helper-provider-request').data,response=entries.find(e=>e.customType==='stream-helper-provider-response').data;
  assert.deepEqual(request.model,model);assert.equal(request.payload.messages[0].content,wire.messages[0].content);assert.deepEqual(request.payload.system,wire.system);assert.deepEqual(request.payload.tools,wire.tools);assert.deepEqual(request.payload.thinking,{type:'adaptive'});
  assert.deepEqual(request.payload.messages[1].content,[{type:'thinking',omitted:true}]);
  assert.doesNotMatch(JSON.stringify([request,response]),/credential secret|nested credential|header secret|header credential|cookie secret|token secret|hidden reasoning|opaque secret/);
  assert.deepEqual(response.headers,{'x-request-id':'request-123','x-amzn-requestid':'bedrock-456','content-type':'text/event-stream','retry-after':'2'});
  assert.equal(response.status,200);assert.equal(statSync(created[0].session_file).mode&0o777,0o600);
  assert.ok(entries.findIndex(e=>e.customType==='stream-helper-attempt')<entries.findIndex(e=>e.customType==='stream-helper-provider-request'));
  assert.ok(entries.findIndex(e=>e.customType==='stream-helper-provider-response')<entries.findIndex(e=>e.type==='message'&&e.message.role==='assistant'));
 } finally {rmSync(root,{recursive:true,force:true});}
});
test('helper transport logs retain tool and completion events but omit text and reasoning for both supported stream formats',async()=>{
 const {providerLogOptions}=await load('provider-log'),seen=[],model={provider:'mock',id:'helper',api:'openai-responses'};
 const hooks=providerLogOptions(event=>seen.push(event));
 const events=[
  {type:'message_start',message:{id:'msg',model:'haiku',usage:{input_tokens:10},content:[{type:'thinking',thinking:'hidden reasoning'}]}},
  {type:'content_block_start',index:0,content_block:{type:'thinking',thinking:'hidden reasoning',signature:'opaque secret'}},
  {type:'content_block_delta',index:0,delta:{type:'thinking_delta',thinking:'hidden reasoning'}},
  {type:'content_block_delta',index:0,delta:{type:'signature_delta',signature:'opaque secret'}},
  {type:'content_block_start',index:1,content_block:{type:'text',text:'streamed text'}},
  {type:'content_block_delta',index:1,delta:{type:'text_delta',text:'streamed text'}},
  {type:'content_block_start',index:2,content_block:{type:'tool_use',id:'call',name:'record_card',input:{}}},
  {type:'content_block_delta',index:2,delta:{type:'input_json_delta',partial_json:'{"changes":{}}'}},
  {type:'content_block_stop',index:2},
  {type:'message_delta',delta:{stop_reason:'tool_use'},usage:{output_tokens:50}},
  {type:'message_stop'},
  {type:'response.created',response:{id:'resp',model:'sol',status:'in_progress',output:[{type:'reasoning',encrypted_content:'opaque secret'}]}},
  {type:'response.output_item.added',output_index:0,item:{type:'reasoning',id:'reason',summary:[{text:'hidden reasoning'}],encrypted_content:'opaque secret'}},
  {type:'response.reasoning_summary_text.delta',delta:'hidden reasoning'},
  {type:'response.output_text.delta',delta:'streamed text'},
  {type:'response.output_item.added',output_index:1,item:{type:'function_call',id:'item',call_id:'call',name:'record_card',arguments:''}},
  {type:'response.function_call_arguments.delta',item_id:'item',output_index:1,delta:'{"changes":{}}'},
  {type:'response.function_call_arguments.done',item_id:'item',output_index:1,arguments:'{"changes":{}}'},
  {type:'response.output_item.done',output_index:1,item:{type:'function_call',name:'record_card',arguments:'{"changes":{}}'}},
  {type:'response.completed',response:{id:'resp',status:'completed',usage:{output_tokens:50},output:[{type:'reasoning',summary:[{text:'hidden reasoning'}]}]}},
  {type:'response.failed',response:{id:'failed',status:'failed',error:{code:'server_error',message:'Failure'}}},
  {type:'error',error:{type:'api_error',message:'Failure',headers:{authorization:'credential secret'}}},
 ];
 const before=structuredClone(events);
 for(const event of events)await hooks.onProviderStreamEvent(event,model);
 assert.deepEqual(events,before);
 assert.doesNotMatch(JSON.stringify(seen),/hidden reasoning|opaque secret|streamed text|credential secret/);
 const raw=seen.map(e=>e.event);
 assert.equal(raw.find(e=>e.type==='content_block_delta').delta.partial_json,'{"changes":{}}');
 assert.equal(raw.find(e=>e.type==='response.function_call_arguments.delta').delta,'{"changes":{}}');
 for(const type of ['content_block_stop','message_delta','message_stop','response.output_item.done','response.function_call_arguments.done','response.completed','response.failed','error'])assert.ok(raw.some(e=>e.type===type),type);
 assert.equal(raw.find(e=>e.type==='message_start').message.id,'msg');assert.equal(raw.find(e=>e.type==='response.completed').response.id,'resp');
});
for(const finish of ['normal','truncated','unclosed-tool','cancelled'])test(`actual Anthropic provider hooks preserve ${finish} evidence and reporting outcome`,async()=>{
 const {stream:providerStream}=await import(new URL('api/anthropic-messages.js',import.meta.resolve('@earendil-works/pi-ai')));
 const root=mkdtempSync(join(tmpdir(),'card-helper-sse-')),created=[],controller=new AbortController();let sent=0;
 try {
  const model={api:'anthropic-messages',provider:'mock',id:'claude-haiku-5-5',name:'Helper',baseUrl:'https://example.test',contextWindow:200000,maxTokens:1024,input:['text'],reasoning:false,cost:{input:0,output:0,cacheRead:0,cacheWrite:0}};
  const raw=[{type:'message_start',message:{id:'original-msg',model:'claude-haiku-5-5',role:'assistant',content:[],usage:{input_tokens:10,output_tokens:0}}},
   {type:'content_block_start',index:0,content_block:{type:'tool_use',id:'call',name:'record_card',input:{}}},
   {type:'content_block_delta',index:0,delta:{type:'input_json_delta',partial_json:finish==='unclosed-tool'?'{"changes":{':'{"changes":{}}'}},
   ...(finish==='unclosed-tool'?[]:[{type:'content_block_stop',index:0}]),
   {type:'message_delta',delta:{stop_reason:'tool_use'},usage:{output_tokens:50}},
   ...(finish==='truncated'?[]:[{type:'message_stop'}])];
  const client={beta:{messages:{create:()=>{sent++;return {asResponse:async()=>new Response(raw.map(event=>`event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`).join(''),{headers:{'content-type':'text/event-stream','x-request-id':'http-request'},status:200})};}}}};
  const ctx={cwd:root,sessionManager:SessionManager.inMemory(root),model,modelRegistry:{complete:async(selected,payload,options)=>{
   const originalHook=options.onProviderStreamEvent;
   return providerStream(selected,payload,{...options,client,onProviderStreamEvent:async(event,m)=>{await originalHook(event,m);if(finish==='cancelled'&&event.type==='content_block_delta')controller.abort();}}).result();
  }}};
  const run=infer(ctx,{type:'session',catalog:context(),messages:[]},controller.signal,undefined,{directory:root,onCreate:value=>created.push(value)});
  if(['truncated','cancelled'].includes(finish))await assert.rejects(run,/message_stop|cancelled/i);
  else assert.deepEqual(await run,{changes:{}});
  assert.equal(sent,1);
  const entries=SessionManager.open(created[0].session_file).getEntries(),rawEvents=entries.filter(e=>e.customType==='stream-helper-provider-event').map(e=>e.data.event);
  assert.equal(entries.filter(e=>e.customType==='stream-helper-provider-request').length,1);
  assert.equal(entries.find(e=>e.customType==='stream-helper-provider-response').data.headers['x-request-id'],'http-request');
  assert.equal(rawEvents[0].message.id,'original-msg');
  // Cancellation may leave already-buffered events to inspect, but must never save a result.
  assert.equal(rawEvents.some(e=>e.type==='content_block_stop'),finish!=='unclosed-tool');
  assert.equal(rawEvents.some(e=>e.type==='message_stop'),finish!=='truncated');
  assert.equal(entries.at(-1).customType,['normal','unclosed-tool'].includes(finish)?'stream-helper-result':'stream-helper-error');
 } finally {rmSync(root,{recursive:true,force:true});}
});
for(const type of ['session','stream','enroll','discussion'])test(`helper logging persists exact ${type} input and raw response as a linked Pi session`,async()=>{
 const root=mkdtempSync(join(tmpdir(),'card-helper-log-')),directory=join(root,'logs'),created=[];
 try {
  const parent=SessionManager.create(root,join(root,'parent'));parent.appendMessage({role:'user',content:'Original conversation',timestamp:1});
  const answer=type==='enroll'?{meaningful:false}:type==='discussion'?{reply:'Discuss only'}:{changes:{}};
  const response={role:'assistant',api:'openai-responses',provider:'mock',model:'helper',stopReason:'toolUse',timestamp:2,usage:{input:10,output:5,cacheRead:0,cacheWrite:0,totalTokens:15,cost:{input:0,output:0,cacheRead:0,cacheWrite:0,total:0}},content:[{type:'thinking',thinking:'Exposed reasoning'},{type:'toolCall',id:'record',name:'record_card',arguments:answer}]};
  let payload;
  const ctx={cwd:root,sessionManager:parent,model:{provider:'mock',id:'helper',contextWindow:100000},modelRegistry:{complete:async(_model,value)=>{
   payload=structuredClone(value);
   assert.equal(created.length,1);
   const before=SessionManager.open(created[0].session_file).getEntries().filter(entry=>entry.type==='message');
   assert.deepEqual(before.slice(1).map(entry=>entry.message),payload.messages,'input is on disk before the model starts');
   return response;
  }}};
  const request={type,catalog:context(),messages:[{role:'user',content:'Current work',timestamp:1}],...(type==='session'?{reset:true}:{}),...(type==='discussion'?{discussion:[{role:'user',text:'Question'}]}:{})};
  assert.deepEqual(await infer(ctx,request,new AbortController().signal,undefined,{directory,onCreate:value=>created.push(value)}),answer);
  assert.equal(readdirSync(directory).length,1);
  const log=SessionManager.open(created[0].session_file),entries=log.getEntries(),messages=entries.filter(entry=>entry.type==='message').map(entry=>entry.message);
  assert.equal(log.getSessionId(),created[0].session_id);assert.equal(log.getHeader().parentSession,parent.getSessionFile());
  assert.match(log.getSessionName(),new RegExp(`Streams helper.*${type}`));
  assert.equal(messages[0].role,'system');assert.equal(messages[0].content,payload.systemPrompt);assert.deepEqual(messages[0].toolsAdded,payload.tools);
  assert.deepEqual(messages.slice(1,-1),payload.messages);assert.deepEqual(messages.at(-1),response);
  const metadata=entries.find(entry=>entry.customType==='stream-helper-request').data;
  assert.deepEqual(metadata,{parent_session_id:parent.getSessionId(),parent_leaf_id:parent.getLeafId(),task:type,reset:type==='session'});
  assert.deepEqual(entries.find(entry=>entry.customType==='stream-helper-result').data,answer);
  assert.doesNotMatch(readFileSync(created[0].session_file,'utf8'),/source-a|"token"/);
  assert.equal(statSync(directory).mode&0o777,0o700);assert.equal(statSync(created[0].session_file).mode&0o777,0o600);
 } finally {rmSync(root,{recursive:true,force:true});}
});
for(const failure of ['throw','response'])test(`helper logging retains every transient ${failure} attempt before retrying`,async()=>{
 const root=mkdtempSync(join(tmpdir(),'card-helper-retry-'));let attempts=0;
 try {
  const ctx={cwd:root,sessionManager:SessionManager.inMemory(root),model:{provider:'mock',id:'helper'},modelRegistry:{complete:async()=>{
   if(++attempts===1){if(failure==='throw')throw Error('503 upstream connect error');return {role:'assistant',stopReason:'error',errorMessage:'503 upstream connect error',content:[],timestamp:1};}
   return {role:'assistant',stopReason:'toolUse',content:[{type:'toolCall',name:'record_card',arguments:{changes:{state:'Recovered'}}}],timestamp:2};
  }}};
  await infer(ctx,{type:'session',catalog:context(),messages:[]},new AbortController().signal,undefined,{directory:root});
  const entries=SessionManager.open(join(root,readdirSync(root)[0])).getEntries();
  assert.deepEqual(entries.filter(entry=>entry.customType==='stream-helper-attempt').map(entry=>entry.data.attempt),[1,2]);
  if(failure==='throw')assert.match(entries.find(entry=>entry.customType==='stream-helper-attempt-error').data.error,/503/);
  else assert.equal(entries.filter(entry=>entry.type==='message'&&entry.message.role==='assistant')[0].message.stopReason,'error');
  assert.equal(entries.at(-1).customType,'stream-helper-result');assert.equal(attempts,2);
 } finally {rmSync(root,{recursive:true,force:true});}
});
for(const failure of ['throw','aborted','invalid','budget'])test(`helper logging retains input and ${failure} failure without saving a result`,async()=>{
 const root=mkdtempSync(join(tmpdir(),'card-helper-failure-'));let attempts=0;
 try {
  const controller=new AbortController();
  const ctx={cwd:root,sessionManager:SessionManager.inMemory(root),model:{provider:'mock',id:'helper',contextWindow:failure==='budget'?1:100000},modelRegistry:{complete:async()=>{
   attempts++;
   if(failure==='throw')throw Error('401 unauthorized');
   if(failure==='aborted'){controller.abort();return {role:'assistant',stopReason:'aborted',content:[],timestamp:1};}
   return {role:'assistant',stopReason:'toolUse',content:[{type:'toolCall',name:'record_card',arguments:{changes:{title_mode:'manual'}}}],timestamp:1};
  }}};
  await assert.rejects(infer(ctx,{type:'session',catalog:context(),messages:[]},controller.signal,undefined,{directory:root}));
  const entries=SessionManager.open(join(root,readdirSync(root)[0])).getEntries();
  assert.equal(entries.at(-1).customType,'stream-helper-error');assert.equal(entries.at(-1).data.cancelled,failure==='aborted');
  assert.ok(entries.some(entry=>entry.type==='message'&&entry.message.role==='user'));assert.ok(!entries.some(entry=>entry.customType==='stream-helper-result'));
  assert.equal(attempts,failure==='budget'?0:1);
 } finally {rmSync(root,{recursive:true,force:true});}
});
test('helper logging captures raw tool arguments before normalization and fails closed if input cannot be logged',async()=>{
 const root=mkdtempSync(join(tmpdir(),'card-helper-raw-'));let calls=0;
 try {
  const ctx={cwd:root,sessionManager:SessionManager.inMemory(root),model:{provider:'mock',id:'helper'},modelRegistry:{complete:async()=>{calls++;return {role:'assistant',stopReason:'toolUse',content:[{type:'toolCall',name:'record_card',arguments:{changes:{title:' ',state:null}}}],timestamp:1};}}};
  assert.deepEqual(await infer(ctx,{type:'session',catalog:context(),messages:[]},new AbortController().signal,undefined,{directory:root}),{changes:{state:''}});
  const entries=SessionManager.open(join(root,readdirSync(root)[0])).getEntries();
  assert.deepEqual(entries.find(entry=>entry.type==='message'&&entry.message.role==='assistant').message.content[0].arguments,{changes:{title:' ',state:null}});
  const blocked=join(root,'not-a-directory');writeFileSync(blocked,'file');
  await assert.rejects(infer(ctx,{type:'session',catalog:context(),messages:[]},new AbortController().signal,undefined,{directory:blocked}));assert.equal(calls,1);
 } finally {rmSync(root,{recursive:true,force:true});}
});
test('helper response persistence errors never retry a successful model call',async()=>{
 const root=mkdtempSync(join(tmpdir(),'card-helper-write-failure-'));let calls=0;
 try {
  let file;
  const ctx={cwd:root,sessionManager:SessionManager.inMemory(root),model:{provider:'mock',id:'helper'},modelRegistry:{complete:async()=>{calls++;chmodSync(file,0o644);return {role:'assistant',stopReason:'toolUse',content:[{type:'toolCall',name:'record_card',arguments:{changes:{}}}],timestamp:1};}}};
  await assert.rejects(infer(ctx,{type:'session',catalog:context(),messages:[]},new AbortController().signal,undefined,{directory:root,onCreate:link=>{file=link.session_file;}}),/identity or permissions changed/);
  assert.equal(calls,1);
 } finally {rmSync(root,{recursive:true,force:true});}
});
test('helper logging refuses an existing permissive directory before exposing inputs',async()=>{
 const root=mkdtempSync(join(tmpdir(),'card-helper-private-'));let calls=0;
 chmodSync(root,0o755);
 try {
  const ctx={cwd:root,sessionManager:SessionManager.inMemory(root),model:{provider:'mock',id:'helper'},modelRegistry:{complete:async()=>{calls++;assert.fail('must not infer without a logged input');}}};
  await assert.rejects(infer(ctx,{type:'session',catalog:context(),messages:[]},new AbortController().signal,undefined,{directory:root}),/0700/);
  assert.equal(calls,0);assert.equal(readdirSync(root).length,0);assert.equal(statSync(root).mode&0o777,0o755);
 } finally {rmSync(root,{recursive:true,force:true});}
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
test('discussion reference edits preserve optional provenance and validate actor authority',async()=>{
 for(const provenance of [{},{commit:'abcdef012345',source_session_id:'session'},{commit:null,source_session_id:null}]) {
  const catalog=context(),saved={...structuredClone(proposedDocument)};Object.assign(saved.target,provenance);catalog.stream.references=[saved];
  const before=structuredClone(catalog),{revision,...desired}=saved;desired.purpose='Updated';
  const key={kind:'document',repository_url:saved.target.repository_url,path:saved.target.path};
  const answer={reply:'Updated the purpose.',edit:{kind:'references',actor:'user',changes:[{key,desired}]}};
  const ctx={model:{contextWindow:100000},modelRegistry:{complete:async()=>({stopReason:'toolUse',content:[{type:'toolCall',name:'record_card',arguments:answer}]})}};
  const request={type:'discussion',catalog,messages:[],instructions:'Update the document purpose.'};
  assert.deepEqual(await infer(ctx,request,new AbortController().signal),answer);assert.deepEqual(catalog,before);
  answer.edit.actor='automatic';if(Object.keys(provenance).length) await assert.rejects(infer(ctx,request,new AbortController().signal),/invalid structured/);
  answer.edit.actor='user';answer.edit.changes[0].desired.unknown='unsupported';await assert.rejects(infer(ctx,request,new AbortController().signal),/invalid structured/);
 }
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
test('report prompts separate contribution state from whole-effort purpose without hard text limits',async()=>{
 const state=Array(40).fill('context').join(' ');const prompts={};
 for(const type of ['session','stream','enroll','discussion']){
  const answer=type==='session'?{changes:{state,remaining:'Resolve the blocker'}}:type==='enroll'?{meaningful:false}:type==='discussion'?{reply:'Discuss only'}:{changes:{}};
  const ctx={model:{contextWindow:100000},modelRegistry:{complete:async(_model,payload)=>{prompts[type]=payload.systemPrompt;return {stopReason:'toolUse',content:[{type:'toolCall',name:'record_card',arguments:answer}]};}}};
  assert.deepEqual(await infer(ctx,{type,catalog:context(),messages:[]},new AbortController().signal),answer);
 }
 assert.match(prompts.session,/in one short sentence/);assert.match(prompts.session,/empty string if Status already covers the next step or no work remains/);
 assert.match(prompts.session,/work inherited from or performed by other sessions/);
 assert.match(prompts.stream,/whole effort's enduring user purpose/);assert.match(prompts.stream,/saved goal is the baseline/);assert.match(prompts.stream,/supplied Sessions are not a complete roster/);
 assert.doesNotMatch(prompts.stream,/actor user|update session.status|10–18 words/);
 assert.doesNotMatch(prompts.session,/actor user|In discussion answer/);
 assert.doesNotMatch(prompts.enroll,/session\.status|stream\.status|For session reports|In discussion|actor user/);
 assert.match(prompts.enroll,/session contains only title/);assert.match(prompts.enroll,/stream contains only title, goal and goal_short/);
 assert.match(prompts.enroll,/Greetings alone are not meaningful work/);assert.match(prompts.enroll,/short, recognizable handles/);
 assert.notEqual(prompts.enroll,prompts.discussion);assert.notEqual(prompts.stream,prompts.enroll);
 for(const type of ['session','stream'])assert.match(prompts[type],/Use unprefixed field names directly inside changes/);
});
test('inference prompt requires short handles rather than work descriptions',async()=>{
 let prompt;
 const ctx={model:{contextWindow:100000},modelRegistry:{complete:async(_model,payload)=>{prompt=payload.systemPrompt;return {stopReason:'toolUse',content:[{type:'toolCall',name:'record_card',arguments:{changes:{}}}]};}}};
 await infer(ctx,{type:'session',catalog:context(),messages:[]},new AbortController().signal);
 for(const rule of [/short, recognizable handles for the sidebar and tab bar/,/descriptions belong in goal and recap/,/session.title.*1–2 words, 3 max/,/specific lane, topic, or role/,/not a task sentence or a repetition of the effort title/,/Infer session\.status \(active, waiting, blocked, parked, done\) from the evidence/])assert.match(prompt,rule);
});
test('session reset omits existing descriptions; stream reset retains contribution evidence and protects manual fields',async()=>{
 for(const type of ['session','stream'])for(const sessionMode of ['automatic','manual'])for(const titleMode of ['automatic','manual'])for(const goalMode of ['automatic','manual']) {
  const catalog=context();Object.assign(catalog.session,{title_mode:sessionMode,state:'Prior state',state_short:'Prior short state',remaining:'Prior remaining'});
  Object.assign(catalog.stream,{title_mode:titleMode,goal_mode:goalMode,goal_short:'Prior short goal'});
  catalog.sessions=[structuredClone(catalog.session),{...catalog.session,host_id:'other-host'},{...catalog.session,id:'sibling'}];
  const before=structuredClone(catalog);let payload;
  const ctx={model:{contextWindow:100000},modelRegistry:{complete:async(_model,value)=>{payload=value;return {stopReason:'toolUse',content:[{type:'toolCall',name:'record_card',arguments:{changes:{}}}]};}}};
  await infer(ctx,{type,catalog,messages:[],reset:true},new AbortController().signal);
  const sent=JSON.parse(payload.messages.at(-1).content),visible=sent.catalog;
  assert.match(sent.reset,/all allowed automatic fields fresh from provided evidence/);assert.match(sent.reset,/fresh short-handle title/);assert.match(sent.reset,/empty no-change delta/);
  if(type==='session') {
   assert.deepEqual(visible.sessions,[]);
   for(const field of ['title','state','state_short','remaining','status'])assert.equal(Object.hasOwn(visible.session,field),false);
   for(const field of ['title','goal','goal_short'])assert.equal(Object.hasOwn(visible.stream,field),false);
   assert.equal(visible.session.id,before.session.id);assert.equal(visible.session.harness_session_id,before.session.harness_session_id);
   assert.deepEqual(visible.stream.repositories,before.stream.repositories);
  } else {
   assert.equal(Object.hasOwn(visible,'session'),false);assert.deepEqual(visible.sessions,[before.session]);
   assert.deepEqual(visible.stream,{...before.stream,...(titleMode==='automatic'?{title:''}:{}),...(goalMode==='automatic'?{goal:null,goal_short:null}:{})});
  }
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
  const logs=[];
  const response=await infer({cwd:root,sessionManager:manager,model,modelRegistry:registry},{type:'session',catalog:context(),messages:evidence.messages},new AbortController().signal,undefined,{directory:join(root,'logs'),onCreate:value=>logs.push(value)});assert.equal(response.changes.state,'SDK verified');
  assert.equal(SessionManager.open(logs[0].session_file).getEntries().find(entry=>entry.type==='message'&&entry.message.role==='assistant').message.model,model.id);
 } finally {rmSync(root,{recursive:true,force:true});}
});
test('reference requests carry captured intent and leave its validation to Rust',()=>{
 const catalog=context(),key={kind:'document',repository_url:'https://example.com/repo',path:'docs/design.md'};
 const saved={...structuredClone(proposedDocument)};Object.assign(saved.target,{commit:'known',source_session_id:'origin'});catalog.stream.references=[saved];
 const auto=structuredClone(proposedDocument);delete auto.target.ref;
 const req=automaticReferenceRequest(catalog,[auto],'op');assert.equal(req.inputs[0].record.id,catalog.session.id);assert.deepEqual(req.changes.reference_changes,[{key,desired:auto}]);
 const user=referenceRequest({...catalog,session:null,sessions:[]},'user',[{key,desired:null}],'delete');assert.deepEqual(user.inputs,[]);assert.deepEqual(user.changes.reference_changes,[{key,desired:null}]);
 const edit=new ReferenceEdit(catalog,key,'user',{...saved,purpose:'retained'});assert.equal(edit.dirty,true);assert.equal(referenceSelector(key),JSON.stringify(['document',key.repository_url,key.path]));
});
test('discussion composer preserves unsent text across close/reopen without model requests',async()=>{
 const state={input:'',messages:[{role:'assistant',text:'Previous discussion'}]};const renders=[];let first=true;
 const ctx={ui:{custom:async factory=>new Promise(resolve=>{const component=factory({terminal:{rows:30},requestRender(){}},{fg:(_color,text)=>text},{},resolve);component.focused=true;if(first){component.handleInput('unsent question');first=false;}renders.push(component.render(80).join('\n'));component.handleInput('\x1b');})}};
 await compose(ctx,state);assert.equal(state.input,'unsent question');await compose(ctx,state);assert.match(renders[1],/unsent question/);assert.match(renders[1],/Previous discussion/);
});
test('enrollment rejects the recorded session.status response before any catalog write',async()=>{
 const catalog=context();catalog.session=null;catalog.sessions=[];
 const raw={content:[{type:'toolCall',name:'record_card',arguments:{meaningful:true,session:{title:'EGM Docs',status:'active'}}}]};
 const capture={native:'native',cwd:'/fixture',catalog,evidence:{messages:[],leafId:null},valid:()=>true,attached:async()=>true};
 await assert.rejects(runReport({kind:'report',force:false},{
  signal:new AbortController().signal,automaticEnrollment:true,capture:async()=>capture,
  generate:async request=>parseInferenceResponse(request,raw),
  enroll:()=>assert.fail('invalid enrollment must not reach Rust'),apply:()=>assert.fail('invalid enrollment must not write reports'),
  receipt:()=>assert.fail('invalid enrollment has no receipt'),notice:assert.fail,
 }),/invalid structured card fields; nothing was saved/);
});
test('background enrollment waits for meaningful work and reuses associated stream before session exists',async()=>{
 for(const meaningful of [false,true]){
  const initial=context();initial.session=null;initial.sessions=[];const requests=[],receipts=[],steps=[];
  const f=reporterFixture();const cli={context:async()=>structuredClone(initial),enroll:async request=>{requests.push(request);initial.session=structuredClone(session);initial.sessions=[initial.session];return {stream:initial.stream,session:initial.session,saves:[{target:'session:native',save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}}}],completion_error:'Publication offline'};},apply:()=>assert.fail('partial enrollment must stop before reports')};
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
 assert.ok(calls[0].rows.includes('Session · Finish…'));assert.ok(calls[0].rows.includes('Stream · Finish…'));
 assert.ok(calls[0].rows.every(row=>! /switch|create|attach/i.test(row)));
});
for(const intent of ['reply','automatic','user'])test(`card discussion ${intent} has typed intent and no redundant confirmation`,async()=>{
 const handlers=new Map(),receipts=[],writes=[],history=[];let openPanel,submitted=false;const menu=['Discuss…','Close'];
 const visible=context();visible.sessions.push({...session,id:'sibling',host_id:'other-host'});
 const cli={sync:localSync,context:async()=>structuredClone(visible),apply:async req=>{writes.push(req);return {save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}}};}};
 const edit=intent==='reply'?undefined:{kind:'stream',actor:intent,changes:intent==='user'?{status:'parked'}:{goal:'Current descriptive goal'}};
 registerWorkCard({on:(name,fn)=>handlers.set(name,fn),registerCommand:(_name,options)=>{openPanel=ctx=>options.handler('card',ctx);},registerTool:()=>{},registerShortcut:()=>assert.fail('no shortcuts'),appendEntry:(type,value)=>receipts.push({type,value})},{settings:{...defaults,reporting:'manual'},cli,infer:async(_ctx,req)=>{assert.equal(req.type,'discussion');assert.match(req.discussion.at(-1).text,/card question/);return {reply:'Discussed without assuming extra intent',...(edit?{edit}:{})};}});
 const ctx={mode:'tui',hasUI:true,cwd:'/repo',sessionManager:{getSessionId:()=> 'native',getLeafId:()=> 'leaf',getBranch:()=>[{id:'leaf'}],buildSessionProjection:()=>({messages:[]})},ui:{select:async()=>menu.shift(),confirm:()=>assert.fail('explicit direction needs no second confirmation'),notify:()=>{},setStatus:()=>{},custom:async factory=>new Promise(resolve=>{const component=factory({terminal:{rows:30},requestRender(){}},{fg:(_color,text)=>text,bold:text=>text},{},resolve);if(component.render(80).join("\n").includes("1: Actions")){const action=menu.shift();if(action==="Discuss…"){component.handleInput("\x1b[B");component.handleInput("\r");}else component.handleInput("\x1b");return;}history.push(component.render(80).join('\n'));if(!submitted){submitted=true;component.handleInput('card question');component.handleInput('\r');}else component.handleInput('\x1b');})}};
 await handlers.get('session_start')({},ctx);await openPanel(ctx);
 assert.equal(writes.length,intent==='reply'?0:1);assert.match(history.at(-1),/Discussed without assuming extra intent/);
 if(intent!=='reply'){assert.equal(writes[0].actor,intent);assert.equal(writes[0].catalog_source,'source-a');assert.equal(Object.hasOwn(writes[0],'ownership_token'),false);assert.equal(receipts.length,1);assert.equal(writes[0].before.status,'active');assert.deepEqual(writes[0].inputs.map(input=>input.record.id),intent==='automatic'?[session.id]:[]);}
});
test('discussion partial receipt retains every refused source and blocks another reference write',async()=>{
 const handlers=new Map(),receipts=[],writes=[],notices=[],history=[];let openPanel,main=0,composer=0,models=0,managerScreen='';
 const live=context(),a={name:'Alpha',purpose:'Original',role:'brief',state:'active',target:{kind:'link',url:'https://example.org/a'}},b={name:'Beta',purpose:'Original',role:'dependency',state:'active',target:{kind:'issue',url:'https://example.org/b'}},c={name:'Gamma',purpose:'New',role:'deliverable',state:'active',target:{kind:'pr',url:'https://example.org/c'}};
 live.stream.references=[a,b];const keys=[a,b,c].map(ref=>({kind:'url',url:ref.target.url}));
 const changes=[{key:keys[0],desired:{...a,purpose:'Updated'}},{key:keys[1],desired:null},{key:keys[2],desired:c}];delete changes[0].desired.revision;
 const cli={sync:localSync,context:async()=>structuredClone(live),apply:async req=>{writes.push(req);live.stream.references=[{...c}];return {save:{local:'persisted',domain:{outcome:'changes',reference_results:[{key:keys[0],outcome:'conflict',reason:'conflict'},{key:keys[1],outcome:'superseded',reason:'superseded'},{key:keys[2],outcome:'changes',reason:'saved'}]}}};}};
 registerWorkCard({on:(name,fn)=>handlers.set(name,fn),registerCommand:(_name,options)=>{openPanel=ctx=>options.handler('card',ctx);},registerTool:()=>{},appendEntry:(type,value)=>receipts.push({type,value})},{settings:{...defaults,reporting:'manual'},cli,infer:async()=>{models++;if(models===1)live.stream.references=[];return {reply:models===1?'Partial update saved':'Must not appear',edit:{kind:'references',actor:'user',changes:models===1?changes:[{key:keys[0],desired:{...changes[0].desired,purpose:'Second edit A'}},{key:keys[2],desired:{...c,purpose:'Second edit C'}}]}};}});
 const ctx={mode:'tui',hasUI:true,cwd:'/repo',sessionManager:{getSessionId:()=> 'native',getLeafId:()=> 'leaf',getBranch:()=>[{id:'leaf'}],buildSessionProjection:()=>({messages:[]})},ui:{select:assert.fail,notify:(text,level)=>notices.push({text,level}),setStatus:()=>{},custom:async factory=>new Promise(resolve=>{
  const component=factory({terminal:{rows:36},requestRender(){}},{fg:(_c,t)=>t,bold:t=>t},{},resolve),screen=component.render(200).join('\n');
  if(screen.includes('1: Actions')) {const n=main++;if(n===0||n===2)selectCardRow(component,'1','Discuss…');else if(n===1)selectCardRow(component,'2','References…');else component.handleInput('\x1b');}
  else if(screen.includes('References · Esc back')) {managerScreen=screen;component.handleInput('\x1b');}
  else {history.push(screen);const n=composer++;if(n===0||n===2){component.handleInput(n===0?'first question':'second question');component.handleInput('\r');}else component.handleInput('\x1b');}
 })}};
 await handlers.get('session_start')({},ctx);await openPanel(ctx);
 assert.equal(models,2);assert.equal(writes.length,1);assert.equal(receipts.length,1);assert.ok(notices.some(row=>row.level==='warning'&&/conflict|superseded/i.test(row.text)));
 assert.match(managerScreen,/Alpha.*Retained edit · Source absent/);assert.match(managerScreen,/Beta.*Retained deletion · Source absent/);assert.match(managerScreen,/Gamma/);
 assert.match(history[1],/Partial update saved/);assert.match(history.at(-1),/second question/);assert.match(history.at(-1),/Finish or discard retained reference edits/);assert.doesNotMatch(history.at(-1),/Must not appear/);
});
test('retained reference input does not block ordinary discussion edits',async()=>{
 const handlers=new Map(),writes=[],history=[];let openPanel,main=0,manager=0,composer=0,retained='';const live=context();
 const cli={sync:localSync,context:async()=>structuredClone(live),apply:async request=>{writes.push(request);return {save:{local:'persisted',domain:{outcome:'changes'}}};}};
 registerWorkCard({on:(name,fn)=>handlers.set(name,fn),registerCommand:(_name,options)=>{openPanel=ctx=>options.handler('card',ctx);},registerTool:()=>{},appendEntry:()=>{}},{settings:{...defaults,reporting:'manual'},cli,infer:async()=>({reply:'Session updated',edit:{kind:'session',actor:'user',changes:{state:'Progress'}}})});
 const actions=['Name…','Keep input and close'];
 const ctx={mode:'tui',hasUI:true,cwd:'/repo',sessionManager:{getSessionId:()=> 'native',getLeafId:()=> 'leaf',getBranch:()=>[{id:'leaf'}],buildSessionProjection:()=>({messages:[]})},ui:{select:async()=>actions.shift(),editor:async()=> 'Draft',notify:()=>{},setStatus:()=>{},custom:async factory=>new Promise(resolve=>{
  const component=factory({terminal:{rows:36},requestRender(){}},{fg:(_c,t)=>t,bold:t=>t},{},resolve),screen=component.render(180).join('\n');
  if(screen.includes('1: Actions')) {const n=main++;if(n===0||n===2)selectCardRow(component,'2','References…');else if(n===1)selectCardRow(component,'1','Discuss…');else component.handleInput('\x1b');}
  else if(screen.includes('References · Esc back')) {const n=manager++;if(n===0)component.handleInput('\r');else {if(n===2)retained=screen;component.handleInput('\x1b');}}
  else {history.push(screen);if(composer++===0){component.handleInput('Update session');component.handleInput('\r');}else component.handleInput('\x1b');}
 })}};
 await handlers.get('session_start')({},ctx);await openPanel(ctx);
 assert.equal(writes.length,1);assert.equal(writes[0].kind,'session');assert.deepEqual(writes[0].changes,{state:'Progress'});assert.match(history.at(-1),/Session updated/);assert.match(retained,/Add reference….*Retained input/);assert.deepEqual(actions,[]);
});
test('wholly refused discussion retains additions and deletion before preserving composer',async()=>{
 const handlers=new Map(),receipts=[],history=[],notices=[];let openPanel,main=0,composer=0,managerScreen='',writes=0;
 const live=context(),old={name:'Forget',purpose:'Old',role:'brief',state:'active',target:{kind:'link',url:'https://example.org/old'}};live.stream.references=[old];
 const one={name:'First',purpose:'One',role:'dependency',state:'active',target:{kind:'link',url:'https://example.org/one'}},two={name:'Second',purpose:'Two',role:'deliverable',state:'active',target:{kind:'pr',url:'https://example.org/two'}};
 const changes=[{key:{kind:'url',url:old.target.url},desired:null},...([one,two].map(desired=>({key:{kind:'url',url:desired.target.url},desired})))];
 const cli={sync:localSync,context:async()=>structuredClone(live),apply:async request=>{writes++;return {save:{local:'persisted',domain:{outcome:'conflict',reference_results:changes.map(({key})=>({key,outcome:'conflict',reason:'concurrent change'}))}}};}};
 registerWorkCard({on:(name,fn)=>handlers.set(name,fn),registerCommand:(_name,options)=>{openPanel=ctx=>options.handler('card',ctx);},registerTool:()=>{},appendEntry:(type,value)=>receipts.push({type,value})},{settings:{...defaults,reporting:'manual'},cli,infer:async()=>{live.stream.references=[];return {reply:'Must not appear',edit:{kind:'references',actor:'user',changes}};}});
 const ctx={mode:'tui',hasUI:true,cwd:'/repo',sessionManager:{getSessionId:()=> 'native',getLeafId:()=> 'leaf',getBranch:()=>[{id:'leaf'}],buildSessionProjection:()=>({messages:[]})},ui:{select:assert.fail,notify:(text,level)=>notices.push({text,level}),setStatus:()=>{},custom:async factory=>new Promise(resolve=>{
  const component=factory({terminal:{rows:36},requestRender(){}},{fg:(_c,t)=>t,bold:t=>t},{},resolve),screen=component.render(200).join('\n');
  if(screen.includes('1: Actions')) {const n=main++;if(n===0)selectCardRow(component,'1','Discuss…');else if(n===1)selectCardRow(component,'2','References…');else component.handleInput('\x1b');}
  else if(screen.includes('References · Esc back')) {managerScreen=screen;component.handleInput('\x1b');}
  else {history.push(screen);if(composer++===0){component.handleInput('Question');component.handleInput('\r');}else component.handleInput('\x1b');}
 })}};
 await handlers.get('session_start')({},ctx);await openPanel(ctx);
 assert.equal(writes,1);assert.equal(receipts.length,1);assert.match(history.at(-1),/Question/);assert.match(history.at(-1),/input kept/);assert.doesNotMatch(history.at(-1),/Must not appear|user: Question/);
 assert.match(managerScreen,/Forget.*Retained deletion · Source absent/);assert.match(managerScreen,/First.*Retained addition · Source absent/);assert.match(managerScreen,/Second.*Retained addition · Source absent/);assert.ok(notices.some(row=>row.level==='warning'));
});
for(const failure of ['throw','abort','invalid','branch','cwd','source','attachment','shutdown','conflict','superseded','repair','completion'])test(`discussion ${failure} retains composer/history and isolates replacement source`,async()=>{
 const handlers=new Map(),renders=[],notices=[],receipts=[];let openPanel,submitted=false,menuCount=0,leaf='leaf';const catalog=context();
 const saveFailure=['conflict','superseded','repair','completion'].includes(failure);
 const cli={sync:localSync,context:async()=>structuredClone(catalog),apply:async req=>{
  assert.equal(saveFailure,true,'cancelled discussion must not write');
  return {save:{local:'persisted',sharing:'local-only',domain:{outcome:['conflict','superseded'].includes(failure)?failure:'changes'}},...(failure==='repair'?{publication:{local:'repair-required',sharing:'confirmed',domain:null}}:{}),...(failure==='completion'?{completion_error:{message:'Incomplete'}}:{})};
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
 const ctx={mode:'tui',hasUI:true,cwd:'/repo',sessionManager:{getSessionId:()=> 'native',getLeafId:()=>leaf,getBranch:()=>[{id:leaf}],buildSessionProjection:()=>({messages:[]})},ui:{select:async()=>menuCount++%2===0?'Discuss…':'Close',notify:text=>notices.push(text),setStatus:()=>{},custom:async factory=>new Promise(resolve=>{const component=factory({terminal:{rows:30},requestRender(){}},{fg:(_color,text)=>text,bold:text=>text},{},resolve);if(component.render(100).join("\n").includes("1: Actions")){if(menuCount++%2===0){component.handleInput("\x1b[B");component.handleInput("\r");}else component.handleInput("\x1b");return;}renders.push(component.render(100).join('\n'));if(!submitted){submitted=true;component.handleInput('retry question');component.handleInput('\r');}else component.handleInput('\x1b');})}};
 await handlers.get('session_start')({},ctx);await openPanel(ctx);
 assert.equal(receipts.length,saveFailure?1:0);
 if(saveFailure)assert.ok(notices.some(text=>text.includes('Discussion edit did not complete; input kept')));
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
 const actions=makeSnapshotActions(defaultConfigSnapshot());
 const cli={context:async()=>{reads++;return structuredClone(catalog);},discardUnreadableCard:async()=>({session:null,stream:null}),getConfig:async()=>structuredClone(actions.snap),setConfig:actions.setConfig,unsetConfig:actions.unsetConfig,cardSettings:async()=>({global:'automatic',session:'inherit',effective:'automatic'}),list:async()=>({streams:[stream],unreadable:[]}),apply:async req=>{writes.push(req);Object.assign(catalog[req.kind],req.changes);return {save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}}};},enroll:async req=>{writes.push(req);catalog.stream=stream;catalog.session=session;return {stream,session,saves:[]};}};
 const ctx={mode:'tui',hasUI:true,cwd:'/repo',sessionManager:{getSessionId:()=> 'native',getLeafId:()=> 'leaf',getBranch:()=>[{id:'leaf'}],buildSessionProjection:()=>({messages:[{role:'user',content:'Build work cards',timestamp:0}]})},ui:{notify:(text,level)=>notices.push({text,level}),setStatus:(_key,text)=>activity.push(text),select:async(title,rows)=>{dialogs.push({title,rows});return options.select?.(title,rows)??'Close';},editor:async()=> 'Contribution',confirm:async()=>true}};
 registerWorkCard({on:(name,fn)=>handlers.set(name,fn),registerCommand:(name,value)=>commands.set(name,value),registerTool:value=>tools.set(value.name,value),registerShortcut:()=>assert.fail('no shortcuts'),appendEntry:(type,value)=>receipts.push({type,value})},{agentDir:options.agentDir,settings:{...defaults,reporting:'manual',enrollment:'manual',...options.settings},cli,infer:async(_ctx,req,_signal,_model,logging)=>{requests.push(req);return options.infer?await options.infer(req,logging):{changes:{}};}});
 return {commands,tools,handlers,ctx,notices,requests,writes,receipts,activity,dialogs,reads:()=>reads,run:args=>commands.get('streams').handler(args,ctx)};
}
test('only streams slash command registers; model checkpoint tool retains its interface',()=>{
 const f=commandFixture();assert.deepEqual([...f.commands.keys()],['streams']);
 for(const old of ['checkpoint','checkpoint:settings','checkpoint:attach','streams:settings','streams:attach'])assert.equal(f.commands.has(old),false);
 assert.deepEqual([...f.tools.keys()],['artifact','stream_read','checkpoint','select_stream','create_stream']);
 assert.ok(f.tools.get('checkpoint').parameters.properties.instructions);
});
test('bare debug gives gated informational help without model, CLI or dialog calls',async()=>{
 for(const enabled of [true,false]) {
  const f=commandFixture({settings:{debug_commands:enabled}});
  f.ctx.modelRegistry={getAvailable:()=>assert.fail('help must not list models'),complete:()=>assert.fail('help must not infer')};
  await f.run('  debug  ');
  assert.equal(f.notices.length,1);assert.equal(f.notices[0].level,enabled?'info':'warning');
  assert.match(f.notices[0].text,/^Streams — /);assert.doesNotMatch(f.notices[0].text,/Error:|Usage:/);
  assert.match(f.notices[0].text,enabled?/debug capture[\s\S]*debug replay/:/Debug commands are disabled/);
  assert.equal(f.reads(),0);
  for(const effects of [f.requests,f.writes,f.receipts,f.dialogs,f.activity])assert.deepEqual(effects,[]);
 }
});
test('Streams notifications use Error messages without stacked error branding',async()=>{
 const f=commandFixture();await f.run('pane');
 assert.deepEqual(f.notices,[{text:'Streams — Native work cards require session startup in an interactive Herdr pane.',level:'warning'}]);
});
test('extension forwards accepted automatic reference notices as info',async()=>{
 const handlers=new Map(),commands=new Map(),notices=[],live=context(),proposal={name:'New',purpose:'Context',role:'brief',state:'active',target:{kind:'link',url:'https://example.org/new'}};
 const cli={sync:localSync,context:async()=>structuredClone(live),apply:async request=>{live.stream.references.push({...proposal});return {save:{local:'persisted',domain:{outcome:'changes',reference_results:[{key:{kind:'url',url:proposal.target.url},desired:proposal,outcome:'changes',reason:'added',action:'add'}]}}};}};
 registerWorkCard({on:(name,fn)=>handlers.set(name,fn),registerCommand:(name,options)=>commands.set(name,options),registerTool:()=>{},appendEntry:()=>{}},{settings:{...defaults,reporting:'manual'},cli,infer:async(_ctx,req)=>req.type==='session'?{changes:{},reference_proposals:[proposal]}:{changes:{}}});
 const ctx={mode:'tui',hasUI:true,cwd:'/repo',sessionManager:{getSessionId:()=> 'native',getLeafId:()=> 'leaf',getBranch:()=>[{id:'leaf'}],buildSessionProjection:()=>({messages:[{role:'user',content:'Current work',timestamp:0}]})},ui:{notify:(text,level)=>notices.push({text,level}),setStatus:()=>{}}};
 await handlers.get('session_start')({},ctx);await commands.get('streams').handler('report',ctx);await until(()=>notices.some(row=>row.text.includes('Reference added: New')));
 assert.ok(notices.some(row=>row.text==='Streams — Reference added: New'&&row.level==='info'));
});
test('report helper failures use Error messages without stacked branding',async()=>{
 const f=commandFixture({infer:async()=>{throw Error('Synthetic helper failure');}});
 await f.run('report');await until(()=>f.activity.length===2);
 assert.deepEqual(f.notices,[{text:'Streams — Synthetic helper failure',level:'warning'}]);
});
test('catalog sync keeps full Error diagnostics internally but not in Streams notifications',async()=>{
 const handlers=new Map(),entries=[],notices=[];
 registerWorkCard({on:(name,fn)=>handlers.set(name,fn),registerCommand:()=>{},registerTool:()=>{},appendEntry:(type,value)=>entries.push({type,value})},{settings:{...defaults},cli:{sync:async()=>{throw Error('offline');}}});
 const ctx={mode:'print',hasUI:false,cwd:'/repo',sessionManager:{getSessionId:()=> 'native'},ui:{notify:(text,level)=>notices.push({text,level}),setStatus:()=>{}}};
 await handlers.get('session_start')({},ctx);
 assert.equal(entries.find(entry=>entry.type==='stream-catalog-sync').value.error,'Error: offline');
 assert.equal(notices.length,1);assert.match(notices[0].text,/^Streams — Catalog sync \(start\): offline\./);assert.doesNotMatch(notices[0].text,/Error:/);
});
test('explicit report receipt notifications use the Streams prefix',async()=>{
 const f=commandFixture({infer:async req=>({changes:req.type==='session'?{state:'Fresh progress'}:{}})});
 await f.run('report');await until(()=>f.activity.length===2);
 assert.equal(f.notices.length,1);assert.match(f.notices[0].text,/^Streams — Work card/);
});
test('debug completion descends into children and replay flags without treating positional input as flags',async()=>{
 const f=commandFixture({settings:{debug_commands:true}}),complete=f.commands.get('streams').getArgumentCompletions;
 const labels=prefix=>(complete(prefix)??[]).map(item=>item.label);
 assert.ok(labels('').includes('debug'));
 for(const prefix of ['debug','debug '])assert.deepEqual(labels(prefix),['capture','replay']);
 assert.deepEqual(labels('debug c'),['capture']);assert.deepEqual(labels('debug re'),['replay']);
 assert.deepEqual(labels('debug capture '),['stream','workspace']);assert.deepEqual(labels('debug capture w'),['workspace']);
 assert.deepEqual(labels('debug replay '),['--model','--reset']);
 assert.deepEqual(labels('debug replay frozen-id --m'),['--model']);
 assert.deepEqual(labels('debug replay --reset '),['session','stream','all']);
 assert.deepEqual(labels('debug replay --reset s'),['session','stream']);
 assert.deepEqual(labels('debug replay --reset session --a'),['--auto']);
 assert.deepEqual(labels('debug replay --reset --a'),['--auto']);
 assert.deepEqual(labels('debug replay frozen-id --reset all --auto '),['--model']);
 for(const prefix of ['report --m','debug capture stream ','debug capture stream --m','debug capture workspace target ','debug replay frozen','debug replay --reset invalid ','debug replay --model ','debug replay --model --r','debug unknown '])assert.equal(complete(prefix),null,prefix);
 for(const prefix of ['debug ','debug re','debug replay --m','debug replay --reset s'])for(const item of complete(prefix)) {
  assert.ok(item.description);assert.ok(item.value.startsWith('debug '));assert.doesNotMatch(item.value,/PROVIDER|CAPTURE_ID/);
 }
 f.ctx.modelRegistry={getAvailable:()=>[{provider:'registered',id:'actual-model'}]};
 await f.run('debug');
 assert.deepEqual(complete('debug replay --model reg'),[{value:'debug replay --model registered/actual-model',label:'registered/actual-model',description:'Replay with this available model'}]);
 assert.deepEqual(labels('debug replay --model registered/actual-model --r'),['--reset']);
 const disabled=commandFixture().commands.get('streams').getArgumentCompletions;
 for(const prefix of ['debug','debug ','debug c','debug replay ','debug replay --reset s'])assert.equal(disabled(prefix),null);
 assert.ok(disabled('').every(item=>item.value!=='debug'));
});
test('streams command hint reaches Pi slash completion and fits beside its label',async()=>{
 const {CombinedAutocompleteProvider,SelectList}=await import('@earendil-works/pi-tui');
 const {getSelectListTheme}=await import('@earendil-works/pi-coding-agent');
 const command=commandFixture().commands.get('streams');
 assert.equal(command.description,'Open the Stream and Session panel');
 const provider=new CombinedAutocompleteProvider([{name:'streams',...command}],'/tmp');
 const suggestions=await provider.getSuggestions(['/strea'],0,6,{signal:new AbortController().signal});
 assert.equal(suggestions.items.length,1);
 assert.equal(suggestions.items[0].description,command.description);
 const rendered=new SelectList(suggestions.items,5,getSelectListTheme()).render(70).join('\n').replace(/\x1b\[[0-9;]*m/g,'');
 assert.match(rendered,/streams.*Open the Stream and Session panel/);
});
test('streams completes subcommand prefixes with short descriptions but never report instructions or extra arguments',async()=>{
 const complete=commandFixture().commands.get('streams').getArgumentCompletions;
 const items=[
  {value:'card',label:'card',description:'Open the Stream and Session panel'},
  {value:'report',label:'report',description:'Save a progress update'},
  {value:'name',label:'name',description:'Set and protect a saved title'},
  {value:'title-mode',label:'title-mode',description:'Change title generation policy'},
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
 for(const prefix of ['pane ','pane c','pane close'])assert.deepEqual(await complete(prefix),[{value:'pane close',label:'close',description:'Close the native pane'}]);
 for(const prefix of ['reset ','reset stream ','reset all ','name ','title-mode '])for(const item of await complete(prefix)){assert.ok(item.description);assert.doesNotMatch(item.description,/effort/i);}
 for(const prefix of ['unknown','report ','report settings','report\nattach','settings ','attach effort','reset extra','pane other','pane close '])assert.equal(await complete(prefix),null);
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
  assert.deepEqual(f.notices,[{text:'Streams — Reset requires an attached work card. Use /streams attach first.',level:'warning'}]);
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
 assert.deepEqual(f.notices,[{text:"Streams — Usage: /streams [card]; /streams report [instructions]; /streams name [session|stream] <name>; /streams title-mode [session|stream] automatic|manual|inherit; /streams reset [session|stream|all] [--auto]; /streams settings; /streams attach; /streams pane [close]. Put report instructions after 'report'.",level:'warning'}]);
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
test('streams settings dispatches to the existing reporting dialog without reporting',async t=>{
 const root=mkdtempSync(join(tmpdir(),'card-settings-command-'));t.after(()=>rmSync(root,{recursive:true,force:true}));
 const f=commandFixture({agentDir:root});await f.run(' settings ');
 assert.equal(f.dialogs.length,1);assert.match(f.dialogs[0].title,/Streams Pi & naming settings/);assert.ok(f.dialogs[0].rows.includes('pi.reporting.frequency: 1'));
 assert.equal(f.reads(),0);assert.deepEqual(f.requests,[]);assert.deepEqual(f.writes,[]);
});
test('helper logging setting toggles in place and passes log options to reports and resets without changing model settings',async()=>{
 const root=mkdtempSync(join(tmpdir(),'card-helper-setting-')),choices=['Debug','pi.reporting.session_logging: false','Back','Close','Debug','pi.reporting.session_logging: true','Back','Close'],seen=[];
 try {
  const f=commandFixture({agentDir:root,select:()=>choices.shift(),infer:async(req,logging)=>{
   seen.push({type:req.type,logging});
   if(logging){assert.equal(logging.directory,join(root,'stream-reporting-sessions'));logging.onCreate({session_id:'helper',session_file:'helper.jsonl'});}
   return {changes:{}};
  }});
  await f.run('report');await until(()=>f.activity.length===2);assert.ok(seen.every(value=>value.logging===undefined));
  await f.run('settings');
  await f.run('reset session --auto');await until(()=>f.activity.length===4);assert.ok(seen.at(-1).logging);
  assert.deepEqual(f.receipts.filter(entry=>entry.type==='stream-helper-session'),[{type:'stream-helper-session',value:{session_id:'helper',session_file:'helper.jsonl'}}]);
  await f.run('settings');
  await f.run('reset stream');await until(()=>f.activity.length===6);assert.equal(seen.at(-1).logging,undefined);
 } finally {rmSync(root,{recursive:true,force:true});}
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
 const shim=join(process.env.CARGO_TARGET_DIR ?? join(checkout,'target'),'debug','streams');
 const env={...process.env,HOME:root,XDG_CONFIG_HOME:join(root,'config'),XDG_DATA_HOME:join(root,'data'),XDG_STATE_HOME:join(root,'state')};
 for(const key of Object.keys(env))if(key.startsWith('STREAMS_'))delete env[key];
 const clients=[];
 const run=(args,input)=>new Promise((resolve,reject)=>{const child=spawn(shim,args,{cwd,env,stdio:['pipe','pipe','pipe']});let stdout='',stderr='';child.stdout.on('data',chunk=>stdout+=chunk);child.stderr.on('data',chunk=>stderr+=chunk);child.on('error',reject);child.on('close',code=>resolve({code,stdout,stderr}));child.stdin.on('error',error=>{if(error.code!=='EPIPE')reject(error);});child.stdin.end(input);});
 try {
  const init=await run(['init','--sync','none','--json']);assert.equal(init.code,0,init.stderr);
  assert.equal((await run(['config','set','frontend.mode','none','--json'])).code,0);
  const cli=childCli(cwd,'native',env),sibling=childCli(cwd,'sibling-native',env),otherClient=childCli(cwd,'other-native',env);clients.push(cli,sibling,otherClient);const empty=await cli.context('native');assert.equal(empty.session,null);assert.equal(empty.stream,null);
  const enrolled=await cli.enroll({version:2,catalog_source:empty.catalog_source,harness:'pi',harness_session_id:'native',stream:{title:'Catalog port',goal:'Safe durable reports'},session:{title:'Implement reporting'},actor:'automatic',cwd});
  assert.ok(enrolled.saves.length>=2);const captured=await cli.context('native');assert.equal(captured.stream.id,enrolled.stream.id);assert.equal(captured.session.harness_session_id,'native');assert.equal(Object.hasOwn(captured,'authority'),false);assert.ok(captured.catalog_source);
  const again=await cli.enroll({version:2,catalog_source:empty.catalog_source,harness:'pi',harness_session_id:'native',stream_id:captured.stream.id,session:{title:'Ignored existing'},actor:'automatic',cwd});assert.equal(again.saves.length,1);assert.equal(again.saves[0].save.domain.outcome,'no-change');assert.equal(Object.hasOwn(again.saves[0].save,'sharing'),false);assert.deepEqual(again.session,captured.session);
  const wrongSource=editRequest(captured,'session',{state:'Never write cloned source'},'automatic');wrongSource.catalog_source='different-source';await assert.rejects(cli.apply(wrongSource),/source/i);
  const human=await cli.apply(editRequest(captured,'session',{state:'Human progress'},'user'));assert.equal(human.save.domain.outcome,'changes');
  const stale=await cli.apply(editRequest(captured,'session',{state:'Stale generated progress'},'automatic'));assert.equal(stale.save.domain.outcome,'superseded');assert.equal((await cli.context('native')).session.state,'Human progress');
  const protectedTitle=await cli.apply(editRequest(await cli.context('native'),'stream',{title:'Human title'},'user'));assert.equal(protectedTitle.stream.title_mode,'manual');
  const fresh=await cli.context('native');assert.ok(!fresh.patch_schemas.stream.automatic.some(row=>row.field==='title'));assert.deepEqual(fresh.patch_schemas,patchSchemas(fresh),'the fake patch schemas follow Rust');
  const goal=await cli.apply(editRequest(fresh,'stream',{goal:'New automatic goal'},'automatic'));assert.equal(goal.stream.title,'Human title');assert.equal(goal.stream.goal_mode,'automatic');
  await cli.enroll({version:2,catalog_source:empty.catalog_source,harness:'pi',harness_session_id:'sibling-native',stream_id:captured.stream.id,session:{title:'Sibling contribution'},actor:'user',cwd});
  const withSibling=await cli.context('native');assert.deepEqual(withSibling.sessions,[withSibling.session]);
  const guardedSession=editRequest(withSibling,'session',{state:'Own progress unaffected by sibling'},'automatic');
  const guardedStream=editRequest(withSibling,'stream',{goal:'Established purpose with addressed contribution'},'automatic');
  const siblingContext=await sibling.context('sibling-native');await sibling.apply(editRequest(siblingContext,'session',{state:'Changed unrelated contribution'},'user'));
  const unaffected=await cli.apply(guardedStream);assert.equal(unaffected.save.domain.outcome,'changes');
  const ownGuard=editRequest(await cli.context('native'),'stream',{goal:'Before own contribution changed'},'automatic');
  const sessionSaved=await cli.apply(guardedSession);assert.equal(sessionSaved.save.domain.outcome,'changes');
  const streamAfterSession=await cli.apply(ownGuard);assert.equal(streamAfterSession.save.domain.outcome,'changes');
  const staleStream=editRequest(withSibling,'stream',{goal:'Stale effort prose'},'automatic');
  const superseded=await cli.apply(staleStream);assert.equal(superseded.save.domain.outcome,'superseded');
  // A user edit compares and swaps on the record revision, so a stale capture conflicts.
  const staleUser=await cli.apply(editRequest(withSibling,'session',{title:'Stale manual edit'},'user'));assert.equal(staleUser.save.domain.outcome,'conflict');
  const blocked=await cli.apply(editRequest(await cli.context('native'),'session',{status:'blocked'},'automatic'));assert.equal(blocked.session.status,'blocked');
  const invalid=await run(['agent','apply','--json'],'{"kind":"stream","changes":{"repositories":{}}}');assert.equal(invalid.code,2);
  const other=await otherClient.enroll({version:2,catalog_source:empty.catalog_source,harness:'pi',harness_session_id:'other-native',stream:{title:'Other effort'},session:{title:'Other'},actor:'user',cwd});
  await assert.rejects(cli.context('native',other.stream.id),/explicit Stream.*current membership/i);
 } finally {await Promise.all(clients.map(cli=>cli.stop()));rmSync(root,{recursive:true,force:true});}
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
test('automatic Stream captures its addressed contribution; Session and manual Stream edits capture none',()=>{
 const catalog=context();
 assert.deepEqual(editRequest(catalog,'session',{state:'Progress'},'automatic').inputs,[]);
 const metadata=editRequest(catalog,'stream',{goal:'Goal'},'automatic',randomUUID());
 assert.deepEqual(metadata.inputs,[{kind:'session',record:catalog.session}]);
 catalog.session.state='Changed after capture';assert.equal(metadata.inputs[0].record.state,session.state);
 assert.deepEqual(editRequest(catalog,'stream',{goal:'Manual goal'},'user',randomUUID()).inputs,[]);
 assert.deepEqual(editRequest(catalog,'session',{title:'Human title'},'user',randomUUID()).inputs,[]);
});
test('cancelled clean editor leaves no retained badge; failed edit reopens action menu first',async()=>{
 const state={discussion:{input:'',messages:[]},edits:new Map()};const notifications=[];const cli={apply:()=>assert.fail('must not write')};
 await editField({ui:{editor:async()=>undefined}},cli,context(),'stream','title',state,()=>{});
 assert.equal(state.edits.size,0);
 const edit=new FieldEdit('session',context(),'status');edit.value='blocked';edit.error='Stale status; compare live value';state.edits.set('session:status',edit);
 let menus=0;await editField({ui:{editor:()=>assert.fail('retain before edit'),select:async(title,rows)=>{menus++;assert.match(title,/Stale status/);assert.ok(rows.includes('Discard input'));return 'Keep input and close';},notify:text=>notifications.push(text)}},cli,context(),'session','status',state,()=>{});
 assert.equal(menus,1);assert.equal(state.edits.get('session:status').value,'blocked');
});
test('a refused short goal stays retained and archive has one confirmation',async()=>{
 const catalog=context();catalog.stream.goal=null;const state={discussion:{input:'',messages:[]},edits:new Map()};
 const edit=new FieldEdit('stream',catalog,'goal_short');edit.value='No source goal';state.edits.set('stream:goal_short',edit);let menus=0;
 await editField({ui:{select:async(title)=>{if(menus++===0)return 'Save field';assert.match(title,/goal/i);return 'Keep input and close';},notify:()=>{}}},{apply:async()=>{throw Error('invalid captured edit: stream.goal_short requires a goal');}},catalog,'stream','goal_short',state,()=>{});
 assert.equal(edit.value,'No source goal');assert.match(edit.error,/goal/);
 const actions=['Stream · Close…','Close'];let confirmations=0,writes=0;
 await openCard({sessionManager:{getSessionId:()=> 'native'},ui:{select:async(title,rows)=>{assert.ok(rows.some(row=>row.startsWith('Stream · goal_short')));return actions.shift();},confirm:async()=>{confirmations++;return true;},notify:()=>{}}},{context:async()=>catalog,apply:async req=>{writes++;assert.deepEqual(req.changes,{status:'closed'});return {save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}}};}},state,{receipt:()=>{},discuss:()=>{},settings:()=>{},report:()=>{}});
 assert.equal(confirmations,1);assert.equal(writes,1);
});
test('fallback reference manager selects identical metadata by exact keys for edit and drop',async()=>{
 const live=context(),state={discussion:{input:'',messages:[]},edits:new Map()},writes=[],receipts=[];
 const value={name:'Same',purpose:'Same purpose',role:'brief',state:'active'};
 live.stream.references=[{...value,target:{kind:'link',url:'https://example.org/a'}},{...value,target:{kind:'link',url:'https://example.org/b'}}];
 const choose=[(title,rows)=>{assert.equal(title,'Work card');return 'Stream · References…';},
  (title,rows)=>{assert.equal(title,'References');assert.match(rows[0],/^1\. Same · Link · Brief · Active/);assert.match(rows[1],/^2\. Same · Link · Brief · Active/);assert.notEqual(rows[0],rows[1]);return rows[0];},
  (title)=>{assert.match(title,/example.org\/a/);return 'Edit';},()=> 'Name…',()=> 'Save reference',
  (_title,rows)=>rows.find(row=>row.startsWith('2. ')),(title)=>{assert.match(title,/example.org\/b/);return 'Drop';},
  ()=> 'Close',()=> 'Close'];
 const ctx={sessionManager:{getSessionId:()=> 'native'},ui:{select:async(title,rows)=>{assert.ok(choose.length,title);return choose.shift()(title,rows);},editor:async()=> 'Renamed',notify:assert.fail}};
 const cli={context:async()=>structuredClone(live),apply:async request=>{writes.push(request);const row=request.changes.reference_changes[0],index=live.stream.references.findIndex(ref=>ref.target.url===row.key.url);live.stream.references[index]={...row.desired};return {save:{local:'persisted',domain:{outcome:'changes'}}};}};
 await openCard(ctx,cli,state,{receipt:value=>receipts.push(value),discuss:assert.fail,settings:assert.fail,report:assert.fail});
 assert.deepEqual(writes.map(request=>request.changes.reference_changes[0].key.url),['https://example.org/a','https://example.org/b']);
 assert.equal(live.stream.references[0].name,'Renamed');assert.equal(live.stream.references[1].state,'dropped');assert.equal(receipts.length,2);assert.equal(choose.length,0);
});
/** A fake apply that refuses the reference values Rust refuses; the wire types mirror its validation. */
test('the fake patch schemas follow Rust for every title mode, title default and goal mode',async()=>{
 const titleModes=['automatic','manual','inherit'],defaults=['automatic','manual'];
 for(const streamTitle of titleModes)for(const sessionTitle of titleModes)for(const streamDefault of defaults)for(const sessionDefault of defaults)for(const goalMode of defaults){
  const catalog=context();Object.assign(catalog.stream,{title_mode:streamTitle,goal_mode:goalMode});catalog.session.title_mode=sessionTitle;catalog.sessions=[structuredClone(catalog.session)];
  catalog.title_defaults={stream:streamDefault,session:sessionDefault};
  const {catalog:projected}=await rustPreview({catalog,action:'apply',request:editRequest(catalog,'session',{},'user')});
  assert.deepEqual(patchSchemas(catalog),projected.patch_schemas,JSON.stringify({streamTitle,sessionTitle,streamDefault,sessionDefault,goalMode}));
 }
});
test('new document form retains invalid input and resumes without repository aliases',async()=>{
 const live=context();live.stream.repositories={};const state={discussion:{input:'',messages:[]},edits:new Map()},writes=[],notices=[];
 const actions=['Stream · References…','Add reference…','Kind…','Document','Role…','Brief','Name…','Purpose…','Repository URL…','Path…','Ref…','Save reference','Keep input and close','Add reference…','Ref…','Save reference','Close','Close'];
 const text=['Plan','Guide rollout','git@example.org:repo','docs/plan.md','  ','main'];
 const ctx={sessionManager:{getSessionId:()=> 'native'},ui:{select:async(title,rows)=>{const next=actions.shift();if(next==='Add reference…')return rows.find(row=>row.startsWith(next));assert.ok(rows.includes(next),`${title}: ${next}`);return next;},editor:async()=>text.shift(),notify:text=>notices.push(text)}};
 const cli={context:async()=>structuredClone(live),apply:validatingApply(writes)};
 await openCard(ctx,cli,state,{receipt:()=>{},discuss:assert.fail,settings:assert.fail,report:assert.fail});
 assert.equal(writes.length,1);assert.equal(writes[0].changes.reference_changes[0].desired.target.ref,'main');assert.equal(writes[0].changes.reference_changes[0].desired.target.repository_url,'git@example.org:repo');
 assert.equal(writes[0].changes.reference_changes[0].desired.state,'active');assert.ok(notices.length);assert.equal(state.referenceEdits.size,0);assert.deepEqual(actions,[]);
});
test('a refused URL stays retained and can be corrected without restarting',async()=>{
 const live=context(),state={discussion:{input:'',messages:[]},edits:new Map()},writes=[],notices=[];
 const actions=['Stream · References…','Add reference…','Kind…','Issue','Role…','Background','Name…','Purpose…','URL…','Save reference','Keep input and close','Add reference…','URL…','Save reference','Close','Close'],text=['Issue','Reference note','bad URL','https://example.org/issue'];
 const ctx={sessionManager:{getSessionId:()=> 'native'},ui:{select:async(title,rows)=>{const action=actions.shift();if(action==='Add reference…')return rows.find(row=>row.startsWith(action));assert.ok(rows.includes(action),`${title}: ${action}`);return action;},editor:async(title,initial)=>{if(title==='URL'&&initial)assert.equal(initial,'bad URL');return text.shift();},notify:text=>notices.push(text)}};
 await openCard(ctx,{context:async()=>structuredClone(live),apply:validatingApply(writes)},state,{receipt:()=>{},discuss:assert.fail,settings:assert.fail,report:assert.fail});
 assert.equal(writes.length,1);assert.equal(writes[0].changes.reference_changes[0].desired.target.url,'https://example.org/issue');assert.ok(notices.length);assert.equal(state.referenceEdits.size,0);
});
test('refused manual addition remains reachable through its retained form',async()=>{
 const live=context(),state={discussion:{input:'',messages:[]},edits:new Map()},receipts=[],notices=[],key={kind:'url',url:'https://example.org/new'};
 const actions=['Stream · References…','Add reference…','Kind…','Link','Role…','Brief','Name…','Purpose…','URL…','Save reference','Keep input and close','Close','Close'],text=['New','Useful','https://example.org/new'];
 const ctx={sessionManager:{getSessionId:()=> 'native'},ui:{select:async(title,rows)=>{const action=actions.shift();if(action==='Add reference…')return rows.find(row=>row.startsWith(action));if(title==='References'&&action==='Close')assert.ok(rows.some(row=>row.startsWith('Add reference…')&&row.includes('Retained input')));return action;},editor:async()=>text.shift(),notify:text=>notices.push(text)}};
 const cli={context:async()=>structuredClone(live),apply:async request=>({save:{local:'persisted',domain:{outcome:'conflict',reference_results:[{key,outcome:'conflict',reason:'Changed concurrently'}]}}})};
 await openCard(ctx,cli,state,{receipt:value=>receipts.push(value),discuss:assert.fail,settings:assert.fail,report:assert.fail});
 assert.equal(receipts.length,1);assert.ok(notices.length);assert.equal(state.referenceEdits.get('new').desired.name,'New');assert.deepEqual(actions,[]);
});
test('custom reference list uses exact selector for equal-metadata rows',async()=>{
 const live=context(),state={discussion:{input:'',messages:[]},edits:new Map()},writes=[],renders=[];let main=0,manager=0;
 const base={name:'Same',purpose:'Same',role:'brief',state:'active'};
 live.stream.references=['a','b'].map(url=>({...base,target:{kind:'link',url:`https://example.org/${url}`}}));
 const actions=['Edit','Name…','Save reference','Drop'];
 const ctx={sessionManager:{getSessionId:()=> 'native'},ui:{notify:assert.fail,editor:async()=> 'Renamed',select:async(title,choices)=>{const action=actions.shift();assert.ok(choices.includes(action),`${title}: ${action}`);return action;},custom:async factory=>new Promise(done=>{
  const component=factory({terminal:{rows:40},requestRender(){}},{fg:(_c,t)=>t,bold:t=>t},{},done),screen=component.render(150).join('\n');renders.push(screen);
  if(screen.includes('1: Actions')) {if(main++===0)selectCardRow(component,'2','References…');else component.handleInput('\x1b');}
  else {const n=manager++;if(n===0) {assert.match(screen,/1\. Same · Link · Brief · Active/);assert.match(screen,/2\. Same · Link · Brief · Activ/);component.handleInput('\r');}
   else if(n===1) {component.handleInput('\x1b[B');assert.match(component.render(150).join('\n'),/2\. Same · Link · Brief · Active · \{"kind":"url","url":"https:\/\/example.org\/b"\}/);component.handleInput('\r');}
   else component.handleInput('\x1b');}
 })}};
 const cli={context:async()=>structuredClone(live),apply:async request=>{writes.push(request);const row=request.changes.reference_changes[0],index=live.stream.references.findIndex(ref=>ref.target.url===row.key.url);live.stream.references[index]={...row.desired};return {save:{local:'persisted',domain:{outcome:'changes'}}};}};
 await openCard(ctx,cli,state,{receipt:()=>{},discuss:assert.fail,settings:assert.fail,report:assert.fail});
 assert.deepEqual(writes.map(request=>request.changes.reference_changes[0].key.url),['https://example.org/a','https://example.org/b']);assert.equal(live.stream.references[0].name,'Renamed');assert.equal(live.stream.references[1].state,'dropped');
});
test('refused target correction retains its original selector and desired destination',async()=>{
 const live=context(),old={name:'Link',purpose:'Purpose',role:'background',state:'active',target:{kind:'link',url:'https://example.org/old'}};live.stream.references=[old];
 const state={discussion:{input:'',messages:[]},edits:new Map()},receipts=[],writes=[],notices=[],actions=['Stream · References…','row','Edit','URL…','Save reference','Keep input and close','row','Inspect retained input','Back','Close','Close'];
 const ctx={sessionManager:{getSessionId:()=> 'native'},ui:{select:async(title,rows)=>{const action=actions.shift();if(action==='row'){assert.match(rows[0],/https:\/\/example.org\/old/);return rows[0];}if(action==='Back')assert.match(title,/https:\/\/example.org\/new/);assert.ok(rows.includes(action),`${title}: ${action}`);return action;},editor:async()=> 'https://example.org/new',notify:text=>notices.push(text)}};
 await openCard(ctx,{context:async()=>structuredClone(live),apply:async request=>{writes.push(request);return {save:{local:'persisted',domain:{outcome:'conflict',reference_results:[{key:{kind:'url',url:old.target.url},outcome:'conflict',reason:'Destination occupied'}]}}};}},state,{receipt:value=>receipts.push(value),discuss:assert.fail,settings:assert.fail,report:assert.fail});
 const row=writes[0].changes.reference_changes[0];assert.equal(row.key.url,'https://example.org/old');assert.equal(row.desired.target.url,'https://example.org/new');assert.equal(receipts.length,1);assert.ok(notices.length);assert.ok(state.referenceEdits.has(referenceSelector(row.key)));
});
test('retained-only reference rows expose missing sources, deletions, additions and manual form',async()=>{
 const original=context(),a={...structuredClone(proposedDocument)},b={name:'Delete',purpose:'Old',role:'dependency',state:'active',target:{kind:'link',url:'https://example.org/delete'}};
 original.stream.references=[a,b];const live=structuredClone(original);live.stream.references=[];
 const keyA={kind:'document',repository_url:a.target.repository_url,path:a.target.path},keyB={kind:'url',url:b.target.url},keyC={kind:'url',url:'https://example.org/new'};
 const automatic={...structuredClone(proposedDocument),purpose:'Changed'},addition={name:'New',purpose:'New reason',role:'background',state:'active',target:{kind:'link',url:keyC.url}};
 const editA=new ReferenceEdit(original,keyA,'automatic',automatic),editB=new ReferenceEdit(original,keyB,'user',null),editC=new ReferenceEdit(original,keyC,'user',addition);
 const base=structuredClone(editA.base),state={discussion:{input:'',messages:[]},edits:new Map(),referenceEdits:new Map([[referenceSelector(keyA),editA],[referenceSelector(keyB),editB],[referenceSelector(keyC),editC]])};
 const steps=[
  ()=> 'Stream · References…',(_t,rows)=>{assert.match(rows[0],/Retained edit · Source absent/);assert.match(rows[1],/Retained deletion · Source absent/);assert.match(rows[2],/Retained addition · Source absent/);return rows[1];},
  ()=> 'Inspect current row',(title)=>{assert.match(title,/Source absent/);return 'Back';},(_t,rows)=>rows[1],()=> 'Inspect retained input',(title)=>{assert.match(title,/Retained input: null/);return 'Back';},(_t,rows)=>rows[1],()=> 'Discard',
  (_t,rows)=>rows[0],()=> 'Edit retained input',()=> 'Name…',()=> 'Keep input and close',
  (_t,rows)=>rows.find(row=>row.startsWith('2. ')),()=> 'Inspect retained input',()=> 'Back',
  (_t,rows)=>rows.find(row=>row.startsWith('2. ')),()=> 'Edit retained input',()=> 'Purpose…',()=> 'Keep input and close',
  (_t,rows)=>{assert.match(rows[1],/Retained addition · Source absent/);return rows.find(row=>row.startsWith('Add reference…'));},()=> 'Name…',()=> 'Keep input and close',
  (_t,rows)=>{assert.ok(rows.find(row=>row.startsWith('Add reference…')&&row.includes('Retained input')));return 'Close';},()=> 'Close'];
 const texts=['Edited retained','','Unfinished'];
 const ctx={sessionManager:{getSessionId:()=> 'native'},ui:{select:async(title,rows)=>{assert.ok(steps.length,title);const selected=steps.shift()(title,rows);assert.ok(rows.includes(selected),`${title}: ${selected}`);return selected;},editor:async()=>texts.shift(),notify:assert.fail}};
 await openCard(ctx,{context:async()=>structuredClone(live),preview:rustPreview,apply:assert.fail},state,{receipt:assert.fail,discuss:assert.fail,settings:assert.fail,report:assert.fail});
 assert.deepEqual(editA.base,base);assert.equal(editA.actor,'user');assert.equal(editA.desired.name,'Edited retained');assert.equal(state.referenceEdits.has(referenceSelector(keyB)),false);assert.equal(state.referenceEdits.has(referenceSelector(keyC)),true);assert.equal(state.referenceEdits.has('new'),true);assert.deepEqual(steps,[]);
});
test('every missing captured source and pending addition can be inspected, edited and discarded independently',async()=>{
 const original=context(),live=structuredClone(original),state={discussion:{input:'',messages:[]},edits:new Map(),referenceEdits:new Map()},instances=[];
 const value=(name,url)=>({name,purpose:'Purpose',role:'brief',state:'active',target:{kind:'link',url}});
 const captured=[value('Alpha','https://example.org/a'),value('Beta','https://example.org/b'),value('Delete','https://example.org/c')];
 original.stream.references=captured;live.stream.references=[];
 const drafts=[...captured.map((row,i)=>({key:{kind:'url',url:row.target.url},desired:i===2?null:{...value(row.name,row.target.url),purpose:'Updated'}})),...['d','e'].map(letter=>({key:{kind:'url',url:`https://example.org/${letter}`},desired:value(`New ${letter}`,`https://example.org/${letter}`)}))];
 for(const draft of drafts) {const edit=new ReferenceEdit(original,draft.key,'user',draft.desired);instances.push({edit,base:structuredClone(edit.base)});state.referenceEdits.set(referenceSelector(draft.key),edit);}
 const steps=[()=> 'Stream · References…'],texts=[];
 for(const [i,draft] of drafts.entries()) {
  const row=(_title,rows)=>{assert.match(rows[0],/Source absent/);return rows[0];};
  steps.push(row,()=> 'Inspect retained input',(title)=>{assert.match(title,/Retained input:/);return 'Back';},row,()=> 'Inspect current row',(title)=>{assert.match(title,/Source absent/);return 'Back';});
  if(draft.desired) {steps.push(row,()=> 'Edit retained input',()=> 'Name…',()=> 'Keep input and close');texts.push(`Edited ${i}`);}
  steps.push(row,()=> 'Discard');
 }
 steps.push(()=> 'Close',()=> 'Close');
 const ctx={sessionManager:{getSessionId:()=> 'native'},ui:{select:async(title,rows)=>{const action=steps.shift()(title,rows);assert.ok(rows.includes(action),`${title}: ${action}`);return action;},editor:async()=>texts.shift(),notify:assert.fail}};
 await openCard(ctx,{context:async()=>structuredClone(live),apply:assert.fail},state,{receipt:assert.fail,discuss:assert.fail,settings:assert.fail,report:assert.fail});
 assert.equal(state.referenceEdits.size,0);assert.deepEqual(steps,[]);for(const {edit,base} of instances)assert.deepEqual(edit.base,base);
 assert.equal(instances[0].edit.desired.name,'Edited 0');assert.equal(instances[1].edit.desired.name,'Edited 1');assert.equal(instances[2].edit.desired,null);assert.equal(instances[3].edit.desired.name,'Edited 3');assert.equal(instances[4].edit.desired.name,'Edited 4');
});
test('explicit fresh reapply preserves Dropped by default and current recovery context',async()=>{
 for(const selection of ['Keep Dropped','Restore Active','Restore Fulfilled','Cancel']) {
  const original=context(),old={...structuredClone(proposedDocument)};Object.assign(old.target,{commit:'old-commit',source_session_id:'old-session'});original.stream.references=[old];
  const live=structuredClone(original);live.stream.revision=7;live.stream.references[0]={...old,state:'dropped',target:{...old.target,commit:'new-commit',source_session_id:'new-session'}};
  const key={kind:'document',repository_url:old.target.repository_url,path:old.target.path},desired=structuredClone(proposedDocument);delete desired.target.ref;desired.purpose='Retained purpose';
  const edit=new ReferenceEdit(original,key,'automatic',desired),state={discussion:{input:'',messages:[]},edits:new Map(),referenceEdits:new Map([[referenceSelector(key),edit]])},writes=[];
  const actions=['Stream · References…','row','Reapply against current row','Reapply',selection,'Close','Close'];
  const ctx={sessionManager:{getSessionId:()=> 'native'},ui:{select:async(title,rows)=>{const action=actions.shift();if(action==='row')return rows[0];if(action==='Reapply')assert.match(title,/new-commit/);if(action==='Keep Dropped')assert.deepEqual(rows,['Keep Dropped','Restore Active','Restore Fulfilled','Cancel']);assert.ok(rows.includes(action),`${title}: ${action}`);return action;},notify:assert.fail}};
  await openCard(ctx,{context:async()=>structuredClone(live),preview:rustPreview,apply:async request=>{writes.push(request);return {save:{local:'persisted',domain:{outcome:'changes'}}};}},state,{receipt:()=>{},discuss:assert.fail,settings:assert.fail,report:assert.fail});
  if(selection==='Cancel') {assert.equal(writes.length,0);assert.deepEqual(edit.base,original);assert.equal(edit.actor,'automatic');}
  else {const row=writes[0].changes.reference_changes[0];assert.equal(writes[0].before.revision,7);assert.equal(row.desired.state,selection==='Keep Dropped'?'dropped':selection==='Restore Active'?'active':'fulfilled');assert.equal(row.desired.target.commit,'new-commit');assert.equal(row.desired.target.source_session_id,'new-session');assert.equal(state.referenceEdits.size,0);}
 }
});
test('Dropped references stay Dropped during edits and restore only through explicit state choice',async()=>{
 const live=context(),state={discussion:{input:'',messages:[]},edits:new Map()},writes=[];
 live.stream.references=[{name:'Plan',purpose:'Old',role:'brief',state:'dropped',target:{kind:'link',url:'https://example.org/plan'}}];
 const actions=['Stream · References…','row','Edit','Purpose…','Save reference','row','Restore','Fulfilled','Close','Close'];
 const ctx={sessionManager:{getSessionId:()=> 'native'},ui:{select:async(title,rows)=>{const action=actions.shift();if(action==='row')return rows[0];if(action==='Purpose…')assert.ok(!rows.includes('State…'));if(action==='Restore')assert.ok(!rows.includes('Drop'));assert.ok(rows.includes(action),`${title}: ${action}`);return action;},editor:async()=> 'Updated purpose',notify:assert.fail}};
 const cli={context:async()=>structuredClone(live),apply:async request=>{writes.push(request);live.stream.references[0]={...request.changes.reference_changes[0].desired};return {save:{local:'persisted',domain:{outcome:'changes'}}};}};
 await openCard(ctx,cli,state,{receipt:()=>{},discuss:assert.fail,settings:assert.fail,report:assert.fail});
 assert.deepEqual(writes.map(request=>request.changes.reference_changes[0].desired.state),['dropped','fulfilled']);assert.equal(live.stream.references[0].purpose,'Updated purpose');assert.deepEqual(actions,[]);
});
test('editing a retained automatic restoration requires an explicit Dropped choice',async()=>{
 for(const choice of ['Keep Dropped','Restore Active','Restore Fulfilled','Cancel']) {
  const live=context(),old={name:'Plan',purpose:'Old',role:'brief',state:'dropped',target:{kind:'link',url:'https://example.org/plan'}};live.stream.references=[old];
  const key={kind:'url',url:old.target.url},desired={...old,state:'active',purpose:'New'};
  const edit=new ReferenceEdit(live,key,'automatic',desired),state={discussion:{input:'',messages:[]},edits:new Map(),referenceEdits:new Map([[referenceSelector(key),edit]])},writes=[];
  const actions=['Stream · References…','row','Edit retained input','Save reference',choice,...(choice==='Cancel'?['Keep input and close']:[]),'Close','Close'];
  const ctx={sessionManager:{getSessionId:()=> 'native'},ui:{select:async(title,rows)=>{const action=actions.shift();if(action==='row')return rows[0];if(action==='Save reference')assert.ok(!rows.includes('State…'));if(action===choice)assert.deepEqual(rows,['Keep Dropped','Restore Active','Restore Fulfilled','Cancel']);assert.ok(rows.includes(action),`${title}: ${action}`);return action;},notify:assert.fail}};
  await openCard(ctx,{context:async()=>structuredClone(live),preview:rustPreview,apply:async request=>{writes.push(request);return {save:{local:'persisted',domain:{outcome:'changes'}}};}},state,{receipt:()=>{},discuss:assert.fail,settings:assert.fail,report:assert.fail});
  assert.equal(writes.length,choice==='Cancel'?0:1);
  if(choice==='Cancel')assert.equal(edit.desired.state,'active');else assert.equal(writes[0].changes.reference_changes[0].desired.state,choice==='Keep Dropped'?'dropped':choice==='Restore Active'?'active':'fulfilled');
 }
});
test('document recovery editor clears only exactly empty values and preserves nonempty text',async()=>{
 const live=context(),state={discussion:{input:'',messages:[]},edits:new Map()},writes=[];const old={...structuredClone(proposedDocument)};
 Object.assign(old.target,{ref:'old',commit:'old-commit',source_session_id:'old-session'});live.stream.references=[old];
 const actions=['Stream · References…','row','Edit','Ref…','Commit…','Source session…','Save reference','Close','Close'],text=[' main ','',' new-session '];
 const ctx={sessionManager:{getSessionId:()=> 'native'},ui:{select:async(_title,rows)=>{const action=actions.shift();return action==='row'?rows[0]:action;},editor:async()=>text.shift(),notify:assert.fail}};
 await openCard(ctx,{context:async()=>structuredClone(live),apply:async request=>{writes.push(request);return {save:{local:'persisted',domain:{outcome:'changes'}}};}},state,{receipt:()=>{},discuss:assert.fail,settings:assert.fail,report:assert.fail});
 const row=writes[0].changes.reference_changes[0];assert.equal(row.desired.target.ref,' main ');assert.equal(Object.hasOwn(row.desired.target,'commit'),false);assert.equal(row.desired.target.source_session_id,' new-session ');assert.equal(writes[0].before.references[0].target.commit,'old-commit');
});
test('retained deletion asks again against the fresh row and cancellation preserves capture',async()=>{
 for(const accepted of [false,true]) {
  const original=context(),old={name:'Plan\u001b[31m',purpose:'Old',role:'brief',state:'active',target:{kind:'link',url:'https://example.org/delete'}};original.stream.references=[old];
  const live=structuredClone(original);live.stream.revision=7;const key={kind:'url',url:old.target.url},edit=new ReferenceEdit(original,key,'user',null),state={discussion:{input:'',messages:[]},edits:new Map(),referenceEdits:new Map([[referenceSelector(key),edit]])},writes=[],prompts=[];
  const actions=['Stream · References…','row','Reapply against current row','Reapply','Close','Close'];
  const ctx={sessionManager:{getSessionId:()=> 'native'},ui:{select:async(title,rows)=>{const action=actions.shift();if(action==='row'){assert.doesNotMatch(rows[0],/\u001b/);assert.match(rows[0],/\\u001b/);return rows[0];}return action;},confirm:async(title,body)=>{prompts.push({title,body});return accepted;},notify:assert.fail}};
  await openCard(ctx,{context:async()=>structuredClone(live),apply:async request=>{writes.push(request);return {save:{local:'persisted',domain:{outcome:'changes'}}};}},state,{receipt:()=>{},discuss:assert.fail,settings:assert.fail,report:assert.fail});
  assert.equal(prompts.length,1);assert.match(prompts[0].body,/already underway/);assert.doesNotMatch(prompts[0].title,/\u001b/);assert.equal(writes.length,accepted?1:0);
  if(accepted) {assert.equal(writes[0].before.revision,7);assert.equal(writes[0].changes.reference_changes[0].desired,null);assert.equal(state.referenceEdits.size,0);}
  else {assert.deepEqual(edit.base,original);assert.equal(state.referenceEdits.size,1);}
 }
});
test('settings validate model types via validateSettings',()=>{
 for(const session_logging of [null,'enabled',0,{}]){assert.throws(()=>validateSettings({...defaults,session_logging}),/logging.*boolean/i);}
 for(const model of [null,[],42,{provider:42,id:'model'},{provider:'provider',id:false}]){assert.throws(()=>validateSettings({...defaults,model}),/model|provider/i);}
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
test('all session descriptions and short forms share automatic report and reset handling',async()=>{
 const catalog=context();
 Object.assign(catalog.session,{recap:'Built settings.',recap_short:'Settings',state:'Under review.',state_short:'Review',remaining:'Land settings.',remaining_short:'Land'});
 catalog.sessions=[structuredClone(catalog.session)];
 const f=commandFixture({catalog,infer:async req=>({changes:req.type==='session'?{recap:'Built settings.',state:'Review complete.',remaining:'Land settings.'}:{}})});
 await f.run('reset session');await until(()=>f.writes.length===1);
 const saved=f.writes.find(request=>request.kind==='session');
 for(const field of ['recap_short','state_short','remaining_short'])assert.equal(saved.changes[field],null);
 assert.equal(saved.changes.recap,'Built settings.');
 const payload=buildInferencePayload({type:'session',catalog,messages:[]});
 for(const field of ['recap','recap_short','remaining_short'])assert.ok(Object.hasOwn(payload.tools[0].parameters.properties.changes.properties,field));
 for(const field of ['recap_short','state_short','remaining_short']){
  const changes={[field]:' 😀'.repeat(30)};
  const result=parseInferenceResponse({type:'session',catalog,messages:[]},{content:[{type:'toolCall',name:'record_card',arguments:{changes}}]});
  assert.equal(result.changes[field],changes[field],'Rust cuts a long short form');
 }
});
test('infer normalizes null remaining/state, empty nullable fields and echoed non-automatic fields, and leaves short forms and the goal rule to Rust',async()=>{
 const ctx=changes=>({model:{contextWindow:100000},modelRegistry:{complete:async()=>({stopReason:'toolUse',content:[{type:'toolCall',name:'record_card',arguments:{changes}}]})}});
 const sessionResult=await infer(ctx({title:'  ',state:null,state_short:`${'x'.repeat(90)}\nline`,remaining:null}),{type:'session',catalog:context(),messages:[],reset:true},new AbortController().signal);
 assert.deepEqual(sessionResult,{changes:{state:'',state_short:`${'x'.repeat(90)}\nline`,remaining:''}});
 const streamCatalog=context();streamCatalog.stream.goal=null;
 const streamResult=await infer(ctx({title:'Echoed manual',status:'active',goal:'   ',goal_short:'Orphan short'}),{type:'stream',catalog:streamCatalog,messages:[],reset:true},new AbortController().signal);
 assert.deepEqual(streamResult,{changes:{goal:null,goal_short:'Orphan short'}});
});
test('a superseded report retains its receipt without retrying inference; a later explicit reset remains possible',async()=>{
 const catalog=context(),sibling={...session,id:'sibling',harness_session_id:'sibling-native',state:'Before'};
 catalog.sessions.push(sibling);
 const receipts=[];let sessionAttempts=0;
 const f=reporterFixture();
 const cli={context:async()=>structuredClone(catalog),discardUnreadableCard:async()=>({session:null,stream:null}),apply:async req=>{
  if(req.kind==='session'&&++sessionAttempts===1){
   sibling.state='Concurrent sibling progress';
   return {save:{local:'unchanged',sharing:'local-only',domain:{outcome:'superseded',conflicts:['inputs'],reason:'captured catalog input changed'}}};
  }
  Object.assign(catalog[req.kind],req.changes);
  return {save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}}};
 }};
 const reporter=new Reporter({cli:()=>cli,settings:()=>defaults,infer:async(_ctx,req)=>({changes:req.type==='session'?{state:'Recovered progress'}:{goal:'Updated goal'}}),notice:text=>assert.fail(text),receipt:value=>receipts.push(value)});
 reporter.request(f.ctx);await reporter.idle();
 assert.equal(sessionAttempts,1);
 assert.equal(receipts.length,1);assert.equal(receipts[0].save.domain.outcome,'superseded');
 reporter.reset(f.ctx,true);await reporter.idle();assert.ok(receipts.length>1);assert.equal(receipts.at(-1).save.domain.outcome,'changes');
});
test('failed manual field edit can rebase onto updated live catalog and save retained input',async()=>{
 const state={discussion:{input:'',messages:[]},edits:new Map()},live=context();
 const edit=new FieldEdit('session',context(),'state');edit.value='My retained edit';edit.error='Edit conflict';
 state.edits.set('session:state',edit);
 live.session.state='Concurrent update';
 let savedReq;
 await editField({ui:{editor:()=>assert.fail('must not reopen editor'),select:async(_title,rows)=>{assert.ok(rows.includes('Rebase on live field & save'));return 'Rebase on live field & save';},notify:assert.fail}},{apply:async req=>{savedReq=req;return {save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}}};}},live,'session','state',state,()=>{});
 assert.equal(savedReq.before.state,'Concurrent update');
 assert.equal(savedReq.changes.state,'My retained edit');
 assert.equal(state.edits.size,0);
});
test('receiptText formats human-readable summaries on success and background reports show Updated in the status bar without chat spam',async()=>{
 assert.equal(receiptText({save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}},session:{...session,title:'Card UX',state:'Detailed state',state_short:'UX fixes deployed'}}),'Work card updated (Card UX): UX fixes deployed');
 assert.equal(receiptText({save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}},stream:{...stream,title:'Streams',goal:'Full goal',goal_short:'Compact goal'}}),'Work card Stream updated (Streams): Compact goal');
 assert.equal(receiptText({save:{local:'unchanged',sharing:'local-only',domain:{outcome:'no-change'}}}),'Work card up to date (no changes)');
 const handlers=new Map(),notices=[],statuses=[];const catalog=context();
 registerWorkCard({on:(name,fn)=>handlers.set(name,fn),registerCommand:()=>{},registerTool:()=>{},registerShortcut:()=>{},appendEntry:()=>{}},{
  settings:{...defaults,reporting:'automatic',frequency:1},
  cli:{sync:localSync,context:async()=>structuredClone(catalog),apply:async req=>{Object.assign(catalog[req.kind],req.changes);return {save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}},[req.kind]:structuredClone(catalog[req.kind])};}},
  infer:async(_ctx,req)=>({changes:req.type==='session'?{state:'Auto progress',state_short:'Auto short'}:{goal:'Auto goal'}}),
 });
 const ctx={mode:'tui',hasUI:true,cwd:'/repo',sessionManager:{getSessionId:()=> 'native',getLeafId:()=> 'leaf',getBranch:()=>[{id:'leaf'}],buildSessionProjection:()=>({messages:[{role:'user',content:'Work'},{role:'assistant',content:[{type:'text',text:'Done'}],stopReason:'stop'}]})},ui:{notify:(text,level)=>notices.push({text,level}),setStatus:(_k,text)=>statuses.push(text),theme:{fg:(_c,t)=>`<green>${t}</green>`}}};
 await handlers.get('session_start')({},ctx);
 handlers.get('agent_end')({messages:[{role:'assistant',stopReason:'stop'}]},ctx);
 await until(()=>statuses.some(s=>s&&s.includes('Updated')));
 assert.equal(notices.length,0);
 assert.equal(statuses.at(-1),'Streams · <green>✓</green> Updated');
 await handlers.get('session_shutdown')({},ctx);
});
test('openCard renders 3-tab Pi TUI (Actions, Stream, Session) and edits fields directly from Stream/Session tabs',async()=>{
 const state={discussion:{input:'',messages:[]},edits:new Map()},live=context();
 const renders=[],saved=[];let step=0;
 const ctx={
  sessionManager:{getSessionId:()=> 'native'},
  ui:{
   notify:assert.fail,
   editor:async(_title,val)=>`${val} (edited)`,
   select:async()=>'Save field',
   custom:async factory=>new Promise(resolve=>{
    const comp=factory({terminal:{rows:30},requestRender(){}},{fg:(_c,t)=>t,bold:t=>t},{},resolve);
    if(step===0){
     renders.push(comp.render(100).join('\n'));
     comp.handleInput('2');
     renders.push(comp.render(100).join('\n'));
     comp.handleInput('\r');
    }else if(step===1){
     comp.handleInput('3');
     renders.push(comp.render(100).join('\n'));
     comp.handleInput('\r');
    }else{
     comp.handleInput('\x1b');
    }
    step++;
   }),
  },
 };
 await openCard(ctx,{context:async()=>live,apply:async req=>{saved.push(req);return {save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}}};}},state,{receipt:()=>{},discuss:assert.fail,settings:assert.fail,report:assert.fail});
 assert.match(renders[0],/\[ 1: Actions \].*2: Stream.*3: Session/);
 assert.match(renders[1],/\[ 2: Stream \]/);
 assert.match(renders[2],/\[ 3: Session \]/);
 assert.match(renders[0],/Reset Stream and Session…/);
 assert.doesNotMatch(renders[0],/Finish…|Close…|Archive…|edit fields/);
 assert.match(renders[1],/Reset…[\s\S]*Finish…[\s\S]*Close…/);
 assert.match(renders[2],/Reset…[\s\S]*Finish…/);
 assert.doesNotMatch(renders[2],/Close…|Archive…|Restore…/);
 for(const rendered of renders)assert.doesNotMatch(rendered,/Whole [Ee]ffort|This [Tt]ab|archived…|⟳/);
 assert.equal(saved.length,2);
 assert.deepEqual(saved[0].changes,{title:'Work (edited)'});
 assert.deepEqual(saved[1].changes,{title:'Session (edited)'});
});

function selectCardRow(component,tab,label) {
 component.handleInput(tab);
 for(let i=0;i<30;i++) {
  if(component.render(140).map(line=>line.replace(/\x1b\[[0-9;]*m/g,'')).some(line=>line.startsWith(`→ ${label} `)||line===`→ ${label}`)){component.handleInput('\r');return;}
  component.handleInput('\x1b[B');
 }
 assert.fail(`Missing ${label} on tab ${tab}`);
}
test('panel keeps all tabs and continuation labels within narrow terminal widths',async()=>{
 const {visibleWidth}=await import('@earendil-works/pi-tui');
 const ctx={sessionManager:{getSessionId:()=> 'native'},ui:{notify:assert.fail,custom:async factory=>new Promise(resolve=>{
  const component=factory({terminal:{rows:22},requestRender(){}},{fg:(_c,t)=>t,bold:t=>t},{},resolve);
  for(const tab of ['1','2','3'])for(const width of [40,60,80]) {
   component.handleInput(tab);const lines=component.render(width);
   assert.ok(lines.every(line=>visibleWidth(line)<=width),`${tab} at ${width}`);
   assert.match(lines.join('\n'),tab==='1'?/\[ 1: Actions \]/:tab==='2'?/\[ 2: Stream \]/:/\[ 3: Session \]/);
  }
  component.handleInput('\x1b');
 })}};
 await openCard(ctx,{context:async()=>context(),apply:assert.fail},{discussion:{input:'',messages:[]},edits:new Map()},{receipt:assert.fail,discuss:assert.fail,settings:assert.fail,report:assert.fail});
});
for(const [tab,target] of [['1','all'],['2','stream'],['3','session']])for(const mode of ['Preserve naming modes','Restore automatic naming',undefined])test(`panel Reset ${target} with ${mode??'cancel'} is scoped and explicit`,async()=>{
 const resets=[];let opened=0;
 const label=target==='all'?'Reset Stream and Session…':'Reset…';
 const ctx={sessionManager:{getSessionId:()=> 'native'},ui:{
  notify:assert.fail,
  select:async(title,choices)=>{assert.match(title,new RegExp(`Reset ${target==='all'?'Stream and Session':target==='stream'?'Stream':'Session'} ·`));assert.deepEqual(choices,['Preserve naming modes','Restore automatic naming']);return mode;},
  custom:async factory=>new Promise(resolve=>{
   const component=factory({terminal:{rows:36},requestRender(){}},{fg:(_c,t)=>t,bold:t=>t},{},resolve);
   if(opened++===0)selectCardRow(component,tab,label);
   else {assert.ok(component.render(140).map(line=>line.replace(/\x1b\[[0-9;]*m/g,'')).some(line=>line.startsWith(`→ ${label} `)),'cancel preserves tab and selected row');component.handleInput('\x1b');}
  }),
 }};
 await openCard(ctx,{context:async()=>context(),apply:assert.fail},{discussion:{input:'',messages:[]},edits:new Map()},{receipt:assert.fail,discuss:assert.fail,settings:assert.fail,report:assert.fail,reset:(auto,target)=>resets.push({auto,target})});
 assert.deepEqual(resets,mode?[{auto:mode==='Restore automatic naming',target}]:[]);
 assert.equal(opened,mode?1:2);
});
for(const [tab,kind] of [['2','stream'],['3','session']])for(const confirmed of [true,false])test(`panel Finish ${kind} ${confirmed?'saves':'cancels'} only that record`,async()=>{
 const saved=[],confirmations=[];let opened=0;
 const ctx={sessionManager:{getSessionId:()=> 'native'},ui:{notify:assert.fail,
  confirm:async(title,body)=>{confirmations.push({title,body});return confirmed;},
  custom:async factory=>new Promise(resolve=>{const component=factory({terminal:{rows:36},requestRender(){}},{fg:(_c,t)=>t,bold:t=>t},{},resolve);if(opened++===0)selectCardRow(component,tab,'Finish…');else component.handleInput('\x1b');}),
 }};
 await openCard(ctx,{context:async()=>context(),apply:async req=>{saved.push(req);return {save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}}};}},{discussion:{input:'',messages:[]},edits:new Map()},{receipt:()=>{},discuss:assert.fail,settings:assert.fail,report:assert.fail});
 assert.equal(confirmations.length,1);assert.equal(confirmations[0].title,kind==='stream'?'Finish Stream?':'Finish Session?');
 assert.doesNotMatch(confirmations[0].body,/effort/i);
 assert.deepEqual(saved.map(req=>({kind:req.kind,changes:req.changes})),confirmed?[{kind,changes:{status:'done'}}]:[]);
});
for(const closed of [true,false])test(`panel ${closed?'Restore':'Close'} is only on Stream and has one confirmation`,async()=>{
 const catalog=context();catalog.stream.status=closed?'closed':'active';
 const saved=[];let opened=0,confirmed=0;
 const ctx={sessionManager:{getSessionId:()=> 'native'},ui:{notify:assert.fail,
  confirm:async title=>{confirmed++;assert.equal(title,closed?'Restore Stream?':'Close Stream?');return true;},
  custom:async factory=>new Promise(resolve=>{const component=factory({terminal:{rows:36},requestRender(){}},{fg:(_c,t)=>t,bold:t=>t},{},resolve);if(opened++===0)selectCardRow(component,'2',closed?'Restore…':'Close…');else component.handleInput('\x1b');}),
 }};
 await openCard(ctx,{context:async()=>catalog,apply:async req=>{saved.push(req);return {save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}}};}},{discussion:{input:'',messages:[]},edits:new Map()},{receipt:()=>{},discuss:assert.fail,settings:assert.fail,report:assert.fail});
 assert.equal(confirmed,1);assert.deepEqual(saved.map(req=>({kind:req.kind,changes:req.changes})),[{kind:'stream',changes:{status:closed?'active':'closed'}}]);
});
test('close failures remain reachable via retained status edit',async()=>{
 const state={discussion:{input:'',messages:[]},edits:new Map()},choices=['Stream · Close…','Stream · status… *','Discard input','Close'];let confirmations=0,writes=0;
 const ctx={sessionManager:{getSessionId:()=> 'native'},ui:{notify:()=>{},confirm:async()=>{confirmations++;return true;},select:async(_title,rows)=>{
  assert.ok(rows.every(row=>!row.startsWith('Stream · archived')));
  const choice=choices.shift();assert.ok(rows.includes(choice),choice);return choice;
 }}};
 await openCard(ctx,{context:async()=>context(),apply:async()=>{writes++;throw Error('Save failed');}},state,{receipt:assert.fail,discuss:assert.fail,settings:assert.fail,report:assert.fail});
 assert.equal(confirmations,1);assert.equal(writes,1);assert.equal(state.edits.size,0);assert.deepEqual(choices,[]);
});
test('unenrolled Session tab has no Session actions',async()=>{
 const catalog=context();catalog.session=null;catalog.sessions=[];
 const ctx={sessionManager:{getSessionId:()=> 'native'},ui:{notify:assert.fail,custom:async factory=>new Promise(resolve=>{
  const component=factory({terminal:{rows:36},requestRender(){}},{fg:(_c,t)=>t,bold:t=>t},{},resolve);
  component.handleInput('3');const rendered=component.render(100).join('\n');assert.match(rendered,/Not yet enrolled/);assert.doesNotMatch(rendered,/Reset…|Finish…/);component.handleInput('\x1b');
 })}};
 await openCard(ctx,{context:async()=>catalog,apply:assert.fail},{discussion:{input:'',messages:[]},edits:new Map()},{receipt:assert.fail,discuss:assert.fail,settings:assert.fail,report:assert.fail});
});

test('record-local v2 edits capture defaults and only the addressed automatic Stream dependency', () => {
 const captured=context();
 captured.sessions.push({...captured.session,id:'sibling',harness_session_id:'other'});
 const progress=editRequest(captured,'session',{state:'Progress'},'automatic','progress');
 const automatic=editRequest(captured,'stream',{goal:'Purpose'},'automatic','purpose');
 const manual=editRequest(captured,'stream',{goal:'User purpose'},'user','manual');
 assert.equal(progress.version,2);assert.equal(progress.title_default,'automatic');
 assert.equal(Object.hasOwn(progress,'ownership_token'),false);
 assert.deepEqual(progress.inputs,[]);assert.deepEqual(manual.inputs,[]);
 assert.deepEqual(automatic.inputs,[{kind:'session',record:captured.session}]);
 captured.title_defaults.stream='manual';assert.equal(automatic.title_default,'automatic');
});

test('record-local inherited title policy is pure and captured per request', () => {
 const captured=context();captured.session.title_mode='inherit';captured.title_defaults.session='manual';
 assert.equal(Object.hasOwn(changesSchema('session',captured,'automatic').properties,'title'),false);
 captured.title_defaults.session='automatic';
 const request=editRequest(captured,'session',{title:'Generated'},'automatic');
 assert.equal(request.title_default,'automatic');assert.equal(request.before.title_mode,'inherit');
 assert.equal(changesSchema('session',captured,'automatic').properties.title.type,'string');
});

test('record-local automatic Stream input cannot be replaced by a roster', () => {
 const captured=context();captured.session=null;
 assert.deepEqual(editRequest(captured,'stream',{goal:'No owner'},'automatic').inputs,[]);
 assert.deepEqual(editRequest(captured,'stream',{goal:'User purpose'},'user').inputs,[]);
});

test('record-local save receipt does not claim publication and failed writes retain dirty input', async () => {
 const accepted={save:{local:'persisted',domain:{outcome:'changes'}},session};
 const cli=new StreamCli('/tmp',async()=>({status:'success',result:accepted}));
 assert.deepEqual(await cli.apply({}),accepted);assert.equal(receiptCanContinue(accepted),true);
 // A save that did not finish is an error; the record state decides a later retry.
 let calls=0;
 const failed=new StreamCli('/tmp',async()=>{calls++;return {status:'error',error:{kind:'storage',message:'replacement flush failed'}};});
 const edit=new FieldEdit('session',context(),'state');edit.value='Retained progress';
 await assert.rejects(edit.save(failed),/replacement flush failed/);
 assert.equal(calls,1);assert.equal(edit.dirty,true);
 await assert.rejects(new StreamCli('/tmp',async()=>({status:'success',result:{save:{local:'uncertain',domain:null}}})).apply({}),/Invalid catalog receipt/);
 const publication=new StreamCli('/tmp',async()=>({status:'success',result:{local:'unchanged',domain:null}}));
 await assert.rejects(publication.sync(),/Invalid catalog receipt/);
});

test('interactive TUI sessions resume on start and park on shutdown without a frontend, stream reset does not mutate session status, and excluded contexts never start child or native controllers', async () => {
 const lifecycleCalls=[];let inferCalls=0;
 const handlers=new Map();
 const liveCatalog=context();liveCatalog.session={...liveCatalog.session,status:'parked'};
 const notices=[];
 registerWorkCard({on:(name,fn)=>handlers.set(name,fn),registerCommand:()=>{},registerTool:()=>{},appendEntry:()=>{}},{
  settings:{...defaults,reporting:'automatic',enrollment:'automatic',frequency:1},
  cli:{sync:localSync,context:async()=>structuredClone(liveCatalog),lifecycle:async action=>{lifecycleCalls.push(action);return {session_changed:true,stream_changed:false};},apply:async()=>({save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}},session:liveCatalog.session})},
  infer:async()=>{inferCalls++;return {changes:{}};},
 });
 const tuiCtx={mode:'tui',hasUI:true,cwd:'/repo',sessionManager:{getSessionId:()=> 'native',getSessionFile:()=>'/tmp/s.jsonl',getLeafId:()=> 'leaf',getBranch:()=>[{id:'leaf'}],buildSessionProjection:()=>({messages:[{role:'user',content:'Real work',timestamp:0}]})},ui:{notify:text=>notices.push(text),setStatus:()=>{}}};
 await handlers.get('session_start')({},tuiCtx);
 assert.deepEqual(lifecycleCalls,['resume']);
 handlers.get('agent_end')({messages:[{role:'assistant',stopReason:'stop'}]},tuiCtx);
 await until(()=>inferCalls>=1);
 assert.deepEqual(lifecycleCalls,['resume']);
 await handlers.get('session_shutdown')({},tuiCtx);
 assert.deepEqual(lifecycleCalls,['resume','park']);
 assert.deepEqual(notices,[]);

 // Production-shape child factory without native UI still resumes on start and parks on shutdown.
 const prodHandlers=new Map(),prodRequests=[];
 registerWorkCard({on:(name,fn)=>prodHandlers.set(name,fn),registerCommand:()=>{},registerTool:()=>{},appendEntry:()=>{}},{
  settings:{...defaults,reporting:'automatic',enrollment:'automatic',frequency:1},
  child:()=>({run:async req=>{prodRequests.push(req);return {status:'success',result:{session_changed:true,stream_changed:false,saves:[]}};},stop:async()=>{}}),
 });
 const plainTuiCtx={mode:'tui',hasUI:true,cwd:'/repo',sessionManager:{getSessionId:()=> 'plain-1',getSessionFile:()=>'/tmp/plain.jsonl',getLeafId:()=> 'l0',getBranch:()=>[{id:'l0'}],buildSessionProjection:()=>({messages:[{role:'user',content:'Prior work',timestamp:0}]})},ui:{notify:()=>{},setStatus:()=>{}}};
 await prodHandlers.get('session_start')({},plainTuiCtx);
 await prodHandlers.get('session_shutdown')({},plainTuiCtx);
 assert.deepEqual(prodRequests.filter(r=>r.kind==='lifecycle').map(r=>r.action),['resume','park']);

 // Stream-only reset on a parked Session must keep capture observational and not call lifecycle.
 const resetCalls=[];
 const resetReporter=new Reporter({
  cli:()=>({context:async()=>structuredClone(liveCatalog),lifecycle:async()=>{resetCalls.push('lifecycle');return {session_changed:true,stream_changed:false};},apply:async()=>({save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}}})}),
  settings:()=>defaults,
  infer:async()=>({changes:{goal:'Updated goal'}}),
  notice:()=>{},
  receipt:()=>{},
 });
 resetReporter.reset(tuiCtx,false,'stream');
 await resetReporter.idle();
 assert.deepEqual(resetCalls,[]);

 // Excluded contexts tested with production-shape child + native factories (no options.cli).
 for (const skippedCtx of [
  {mode:'print',hasUI:false,cwd:'/repo',sessionManager:{getSessionId:()=> 'p1',getSessionFile:()=>'/tmp/p1.jsonl',getLeafId:()=> 'l1',getBranch:()=>[{id:'l1'}],buildSessionProjection:()=>({messages:[{role:'user',content:'Print',timestamp:0}]})},ui:{notify:()=>{},setStatus:()=>{}}},
  {mode:'json',hasUI:false,cwd:'/repo',sessionManager:{getSessionId:()=> 'j1',getSessionFile:()=>'/tmp/j1.jsonl',getLeafId:()=> 'l2',getBranch:()=>[{id:'l2'}],buildSessionProjection:()=>({messages:[{role:'user',content:'Json',timestamp:0}]})},ui:{notify:()=>{},setStatus:()=>{}}},
  {mode:'tui',hasUI:true,cwd:'/repo',sessionManager:{getSessionId:()=> 'ephem',getSessionFile:()=>undefined,getLeafId:()=> 'l3',getBranch:()=>[{id:'l3'}],buildSessionProjection:()=>({messages:[{role:'user',content:'Ephem',timestamp:0}]})},ui:{notify:()=>{},setStatus:()=>{}}},
 ]) {
  const skippedHandlers=new Map();
  let childStarts=0,childRequests=0,nativeStarts=0;
  registerWorkCard({on:(name,fn)=>skippedHandlers.set(name,fn),registerCommand:()=>{},registerTool:()=>{},appendEntry:()=>{}},{
   settings:{...defaults,reporting:'automatic',enrollment:'automatic',frequency:1},
   child:()=>{childStarts++;return {run:async()=>{childRequests++;return {status:'success',result:{}};},stop:async()=>{}};},
   native:async()=>{nativeStarts++;return {start:async()=>{nativeStarts++;},open:async()=>{},close:async()=>{},dispose:async()=>{}};},
  });
  await skippedHandlers.get('session_start')({},skippedCtx);
  await skippedHandlers.get('session_info_changed')({name:'Renamed'},skippedCtx);
  skippedHandlers.get('session_tree')({},skippedCtx);
  skippedHandlers.get('agent_end')({messages:[{role:'assistant',stopReason:'stop'}]},skippedCtx);
  await turn();
  assert.equal(childStarts,0);
  assert.equal(childRequests,0);
  assert.equal(nativeStarts,0);
 }
});
