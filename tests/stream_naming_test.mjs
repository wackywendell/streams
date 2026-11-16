import assert from 'node:assert/strict';
import test from 'node:test';
import { createJiti } from 'jiti';
import { mkdtempSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
const jiti = createJiti(import.meta.url);
const { parseNamingEdit, saveNamingEdit, savePiNameEdit, namingCompletions } = await jiti.import(new URL('../extensions/stream-checkpoint/naming.ts', import.meta.url).pathname);
const { default: registerWorkCard } = await jiti.import(new URL('../extensions/stream-checkpoint/index.ts',import.meta.url).pathname);
const { defaults } = await jiti.import(new URL('../extensions/stream-checkpoint/settings.ts',import.meta.url).pathname);
const { projectSessionName } = await jiti.import(new URL('../integrations/herdr-work-cards/src/pi/session-naming.ts',import.meta.url).pathname);
const { validateChanges } = await jiti.import(new URL('../extensions/stream-checkpoint/core.ts', import.meta.url).pathname);
const { editField } = await jiti.import(new URL('../extensions/stream-checkpoint/panel.ts', import.meta.url).pathname);

function context() {
 const session = {id:'session', host_id:'host', stream_id:'effort', harness:'pi', harness_session_id:'native', title:'Saved title', title_mode:'automatic', status:'active', state:'Work', state_short:null, remaining:'', checkout:null};
 return {catalog_source:'source',host_id:'host',title_defaults:{stream:'automatic',session:'automatic'},
  stream:{id:'effort',title:'Effort',title_mode:'inherit',goal:null,goal_short:null,goal_mode:'automatic',owner:{host_id:'host'},status:'active',archived:false,repositories:{},references:[]},
  session,sessions:[session]};
}
const accepted = {operation_id:'name',save:{local:'persisted',sharing:'local-only',domain:{outcome:'changes'}}};

test('name syntax defaults to Session, preserves title text and explicitly pins manual mode', () => {
 assert.deepEqual(parseNamingEdit('name','  My  task  '), {target:'session',changes:{title:'My  task',title_mode:'manual'}});
 assert.deepEqual(parseNamingEdit('name','session Session task'), {target:'session',changes:{title:'Session task',title_mode:'manual'}});
 assert.deepEqual(parseNamingEdit('name','stream Shared effort'), {target:'stream',changes:{title:'Shared effort',title_mode:'manual'}});
 assert.equal(parseNamingEdit('name','automatic').changes.title, 'automatic');
 for (const args of ['', ' ', 'session', 'stream ', 'bad\0name']) assert.throws(() => parseNamingEdit('name',args), /usage/i);
});
test('mode controls change only the selected title policy and never supply another title', () => {
 for (const mode of ['automatic','manual','inherit']) {
  assert.deepEqual(parseNamingEdit('title-mode',mode), {target:'session',changes:{title_mode:mode}});
  assert.deepEqual(parseNamingEdit('title-mode','stream '+mode), {target:'stream',changes:{title_mode:mode}});
 }
 for (const args of ['', 'stream', 'auto', 'manual extra']) assert.throws(() => parseNamingEdit('title-mode',args), /usage/i);
});
test('user title modes accept inherit without allowing it for goal policy', () => {
 const catalog = context();
 validateChanges('session',catalog.session,{title_mode:'inherit'},'user');
 assert.throws(() => validateChanges('stream',catalog.stream,{goal_mode:'inherit'},'user'), /automatic or manual/);
});
test('manual naming captures one current target and saves title plus mode, including same-text pins', async () => {
 for (const target of ['session','stream']) {
  const catalog = context(), calls = [];
  const client = {context:async id => {calls.push(['context',id]);return catalog;},apply:async request => {calls.push(['apply',request]);return accepted;}};
  const title = catalog[target].title;
  assert.equal(await saveNamingEdit(client,'native',parseNamingEdit('name',target+' '+title),()=>{}), accepted);
  assert.deepEqual(calls.map(call=>call[0]), ['context','apply']);
  assert.deepEqual(calls[1][1].changes, {title,title_mode:'manual'});
  assert.equal(calls[1][1].kind,target);
  assert.equal(calls[1][1].actor,'user');
  catalog[target].title = 'Later';
  assert.equal(calls[1][1].before.title,title);
 }
});
test('mode-only saves do not include title text or request title generation', async () => {
 for (const args of ['manual','session inherit','stream automatic']) {
  const writes = [], edit = parseNamingEdit('title-mode',args);
  const client = {context:async()=>context(),apply:async request=>{writes.push(request);return accepted;}};
  assert.equal(await saveNamingEdit(client,'native',edit,()=>{}),accepted);
  assert.deepEqual(writes[0].changes,edit.changes);
  assert.deepEqual(Object.keys(writes[0].changes),['title_mode']);
 }
});
test('panel offers inherited title policy but only two goal modes', async () => {
 for (const [field,mode,expected] of [
  ['title_mode','inherit',['automatic','manual','inherit']],
  ['goal_mode','manual',['automatic','manual']],
 ]) {
  let selected = 0;
  const writes = [];
  const ctx = {ui:{notify:assert.fail,select:async(_title,rows)=>{
   if (selected++ === 0) {assert.deepEqual(rows,expected);return mode;}
   return 'Save field';
  }}};
  const catalog = context();catalog.stream.title_mode = 'automatic';
  await editField(ctx,{apply:async request=>{writes.push(request);return accepted;}},catalog,'stream',field,{edits:new Map()},()=>{});
  assert.deepEqual(writes[0].changes,{[field]:mode});
 }
});
test('stale identity fences refuse before reading or saving; an accepted save is not discarded after it completes', async () => {
 let reads = 0, saves = 0, checks = 0;
 const client = {context:async()=>{reads++;return context();},apply:async()=>{saves++;return accepted;}};
 await assert.rejects(saveNamingEdit(client,'native',parseNamingEdit('name','Task'),()=>{throw Error('stale');}),/stale/);
 assert.equal(reads,0);assert.equal(saves,0);
 await assert.rejects(saveNamingEdit(client,'native',parseNamingEdit('name','Task'),()=>{if (++checks === 2) throw Error('stale');}),/stale/);
 assert.equal(reads,1);assert.equal(saves,0);
 checks = 0;
 assert.equal(await saveNamingEdit(client,'native',parseNamingEdit('name','Task'),()=>{checks++;}),accepted);
 assert.equal(checks,2);
});
test('unbound conversations and failed saves do not enroll, retry or call frontend setters', async () => {
 let saves = 0;
 const unbound = context();unbound.session = null;
 const client = {context:async()=>unbound,apply:async()=>{saves++;throw Error('save failed');}};
 await assert.rejects(saveNamingEdit(client,'native',parseNamingEdit('name','Task'),()=>{}),/attached session/);
 assert.equal(saves,0);
 client.context = async()=>context();
 await assert.rejects(saveNamingEdit(client,'native',parseNamingEdit('name','Task'),()=>{}),/save failed/);
 assert.equal(saves,1);
});

test('Pi names save only changed bound titles, without pinning matching names', async () => {
 for (const title_mode of ['automatic','manual','inherit']) for (const name of ['Saved title','Changed title']) {
  const catalog=context();catalog.session.title_mode=title_mode;
  const writes=[],client={context:async id=>{assert.equal(id,'native');return catalog;},apply:async request=>{writes.push(request);return accepted;}};
  const value=await savePiNameEdit(client,'native',name,()=>{});
  if(name==='Saved title') {assert.equal(value,undefined);assert.deepEqual(writes,[]);assert.equal(catalog.session.title_mode,title_mode);}
  else {assert.equal(value,accepted);assert.deepEqual(writes[0].changes,{title:name,title_mode:'manual'});assert.equal(writes[0].actor,'user');assert.equal(writes[0].kind,'session');}
 }
});
test('empty names and unbound Pi conversations do not save or enroll', async () => {
 let reads=0;
 const client={context:async()=>{reads++;return {...context(),session:null};},apply:assert.fail};
 for(const name of [undefined,'','  ','bad\0name']) assert.equal(await savePiNameEdit(client,'native',name,assert.fail),undefined);
 assert.equal(reads,0);
 assert.equal(await savePiNameEdit(client,'native','Native only',()=>{}),undefined);
 assert.equal(reads,1);
});
test('Pi name edits fence before read and apply, propagate failure and retain accepted results', async () => {
 let reads=0,saves=0,checks=0;
 const client={context:async()=>{reads++;return context();},apply:async()=>{saves++;return accepted;}};
 await assert.rejects(savePiNameEdit(client,'native','Task',()=>{throw Error('stale');}),/stale/);
 assert.equal(reads,0);assert.equal(saves,0);
 await assert.rejects(savePiNameEdit(client,'native','Task',()=>{if(++checks===2)throw Error('stale');}),/stale/);
 assert.equal(reads,1);assert.equal(saves,0);
 checks=0;
 assert.equal(await savePiNameEdit(client,'native','Task',()=>{checks++;}),accepted);assert.equal(checks,2);
 client.apply=async()=>{saves++;throw Error('disk unavailable');};
 await assert.rejects(savePiNameEdit(client,'native','Task',()=>{}),/disk unavailable/);
 assert.equal(saves,2);
});
test('a late Pi projection echo may become a manual name after the saved title changes', async () => {
 const catalog=context();catalog.session.title='New saved title';
 let saved;
 assert.equal(await savePiNameEdit({context:async()=>catalog,apply:async request=>{saved=request;return accepted;}},'native','Older projected title',()=>{}),accepted);
 assert.deepEqual(saved.changes,{title:'Older projected title',title_mode:'manual'});
});

async function bridgeFixture(options={}) {
 const catalog=context(),handlers=new Map(),commands=new Map(),writes=[],entries=[],notices=[],setters=[],deliveries=[];
 if(options.unbound)catalog.session=null;
 let nativeId='native',shown='Frontend name',reads=0;
 const ctx={cwd:'/tmp',hasUI:false,mode:'print',sessionManager:{getSessionId:()=>nativeId},ui:{notify:(text,level)=>notices.push({text,level}),setStatus(){}}};
 const pi={on:(name,handler)=>handlers.set(name,handler),registerCommand:(name,value)=>commands.set(name,value),registerTool(){},appendEntry:(type,value)=>entries.push({type,value}),getSessionName:()=>shown,setSessionName:name=>{
  shown=name;setters.push(name);deliveries.push(handlers.get('session_info_changed')({name},ctx));
 }};
 const cli={sync:async()=>({local:'unchanged',sharing:'local-only',domain:null}),context:async()=>{reads++;await options.read?.();return structuredClone(catalog);},apply:async request=>{
  writes.push(request);await options.apply?.();
  if(options.fail)throw Error('catalog disk unavailable');
  Object.assign(catalog[request.kind],request.changes);return accepted;
 }};
 registerWorkCard(pi,{cli,agentDir:options.agentDir,settings:{...defaults,reporting:'manual',enrollment:'manual',...options.settings},infer:()=>assert.fail('Naming must not infer')});
 await handlers.get('session_start')({},ctx);
 return {catalog,writes,entries,notices,setters,handlers,ctx,reads:()=>reads,shown:()=>shown,switch:()=>{nativeId='other';},
  run:args=>commands.get('streams').handler(args,ctx),
  rename:async name=>{shown=name;await handlers.get('session_info_changed')({name},ctx);},
  project:async title=>{projectSessionName(pi,ctx.sessionManager,nativeId,title,()=>{});await Promise.all(deliveries.splice(0));},
 };
}
test('registered naming commands pin equal titles, change modes only and never project directly',async()=>{
 const f=await bridgeFixture();f.catalog.session.title_mode='inherit';
 await f.run('name Saved title');await f.run('title-mode stream manual');await f.run('title-mode inherit');
 assert.deepEqual(f.writes.map(request=>({kind:request.kind,changes:request.changes})),[
  {kind:'session',changes:{title:'Saved title',title_mode:'manual'}},
  {kind:'stream',changes:{title_mode:'manual'}},
  {kind:'session',changes:{title_mode:'inherit'}},
 ]);
 assert.deepEqual(f.setters,[]);assert.equal(f.entries.length,3);assert.equal(f.catalog.stream.title,'Effort');
});
test('Pi name events import different names, ignore equal projection echoes, and leave unbound chats native-only',async()=>{
 const f=await bridgeFixture();
 await f.project('Saved title');assert.deepEqual(f.writes,[]);assert.equal(f.catalog.session.title_mode,'automatic');
 await f.rename('My name');assert.deepEqual(f.writes[0].changes,{title:'My name',title_mode:'manual'});
 assert.deepEqual(f.setters,['Saved title']);assert.equal(f.entries.length,1);assert.deepEqual(f.notices,[]);
 const unbound=await bridgeFixture({unbound:true});await unbound.rename('Independent chat');
 assert.deepEqual(unbound.writes,[]);assert.deepEqual(unbound.notices,[]);assert.equal(unbound.shown(),'Independent chat');
});
test('Pi import is default-enabled and independent of outgoing Pi synchronization',async()=>{
 for(const pi_name_edits of [false,true]) {
  const f=await bridgeFixture({settings:{pi_name_edits}});
  await f.rename('Changed in Pi');
  assert.equal(f.writes.length,pi_name_edits?1:0);assert.equal(f.reads(),pi_name_edits?1:0);
  assert.deepEqual(f.setters,[]);
 }
 assert.equal(defaults.pi_name_edits,true);
});
test('Pi import failures are visible after the native rename and do not project a replacement',async()=>{
 const f=await bridgeFixture({fail:true});
 await f.rename('Already changed in Pi');
 assert.equal(f.shown(),'Already changed in Pi');assert.equal(f.catalog.session.title,'Saved title');
 assert.equal(f.writes.length,1);assert.deepEqual(f.entries,[]);assert.deepEqual(f.setters,[]);
 assert.match(f.notices[0].text,/Pi name changed, but its Streams edit failed:.*catalog disk unavailable/);
});
for(const changed of ['session','shutdown'])test(`Pi name reads cannot save after ${changed} changes`,async()=>{
 let release;
 const f=await bridgeFixture({read:()=>new Promise(resolve=>release=resolve)});
 const rename=f.rename('Too late');assert.equal(typeof release,'function');
 if(changed==='session')f.switch();else await f.handlers.get('session_shutdown')({},f.ctx);
 release();await rename;assert.deepEqual(f.writes,[]);assert.deepEqual(f.notices,[]);
});
test('accepted Pi name saves retain receipts after branch cancellation',async()=>{
 let release;
 const f=await bridgeFixture({apply:()=>new Promise(resolve=>release=resolve)});
 const rename=f.rename('Accepted name');while(!release)await new Promise(resolve=>setImmediate(resolve));
 f.handlers.get('session_tree')({},f.ctx);release();await rename;
 assert.equal(f.writes.length,1);assert.equal(f.entries.length,1);assert.equal(f.catalog.session.title,'Accepted name');
});
test('naming completions distinguish saved-name targets from title policies',()=>{
 assert.deepEqual(namingCompletions('name s').map(item=>item.value),['name session','name stream']);
 assert.equal(namingCompletions('name session My title'),null);
 assert.deepEqual(namingCompletions('title-mode ').map(item=>item.value),['title-mode session','title-mode stream','title-mode automatic','title-mode manual','title-mode inherit']);
 assert.deepEqual(namingCompletions('title-mode stream i').map(item=>item.value),['title-mode stream inherit']);
 assert.equal(namingCompletions('title-mode session unknown'),null);
});
