import assert from 'node:assert/strict';
import test from 'node:test';
import { createJiti } from 'jiti';
const jiti = createJiti(import.meta.url);
const { parseNamingEdit, saveNamingEdit } = await jiti.import(new URL('../extensions/stream-checkpoint/naming.ts', import.meta.url).pathname);
const { validateChanges } = await jiti.import(new URL('../extensions/stream-checkpoint/core.ts', import.meta.url).pathname);
const { editField } = await jiti.import(new URL('../extensions/stream-checkpoint/panel.ts', import.meta.url).pathname);

function context() {
 const session = {id:'session', host_id:'host', stream_id:'effort', harness:'pi', harness_session_id:'native', title:'Saved title', title_mode:'automatic', status:'active', state:'Work', state_short:null, remaining:'', checkout:null};
 return {catalog_source:'source',host_id:'host',title_defaults:{stream:'automatic',session:'automatic'},
  stream:{id:'effort',title:'Effort',title_mode:'inherit',goal:null,goal_short:null,goal_mode:'automatic',owner:{host_id:'host'},status:'active',archived:false,repositories:{},documents:[],issues:[],prs:[]},
  session,sessions:[session],authority:{host_id:'host',token:'legacy'}};
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
