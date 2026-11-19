import assert from 'node:assert/strict';
import test from 'node:test';
import { createJiti } from 'jiti';
import { withPatchSchemas } from './helpers/patch-schemas.mjs';
const jiti = createJiti(import.meta.url);
const { StreamCli, receiptCanContinue, receiptHasUnappliedReferences, receiptHasWarning, receiptText, referenceText, resultNeedsAttention, safeReferenceJson } = await jiti.import('../extensions/stream-checkpoint/streams.ts');
const { FieldEdit } = await jiti.import('../extensions/stream-checkpoint/core.ts');
const save = {local:'committed',sharing:'pending',domain:{outcome:'changes'}};
const publication = {local:'unchanged',sharing:'confirmed',domain:null};
const receipt = {save};

test('local acceptance, publication warnings and repair are independent facts', () => {
 for (const sharing of ['local-only','pending','confirmed','failed','blocked','uncertain']) {
  const published = {...publication,sharing};
  const value = {...receipt,publication:published};
  const warning = ['failed','blocked','uncertain'].includes(sharing);
  assert.equal(resultNeedsAttention(published),warning);
  assert.equal(receiptCanContinue(value),true);
  assert.equal(receiptHasWarning(value),warning);
  assert.equal(receiptCanContinue({...value,publication:{...published,local:'repair-required'}}),false);
 }
 for (const outcome of ['superseded','conflict']) assert.equal(receiptCanContinue({...receipt,save:{...save,domain:{outcome}}}),false);
 assert.equal(receiptCanContinue({...receipt,save:{...save,domain:null}}),false);
 assert.equal(receiptCanContinue({...receipt,completion_error:{message:'incomplete'}}),false);
 assert.equal(receiptHasWarning({...receipt,save:{...save,sharing:'uncertain'}}),true);
});

test('wire parsing rejects unknown outcomes and ambiguous receipt shapes', async () => {
 for (const value of [
  {...receipt,save:{...save,local:'failed'}},
  {...receipt,save:{...save,sharing:'offline'}},
  {...receipt,publication:{sharing:'failed'}},
  {...receipt,publication:{...publication,domain:{outcome:'changes'}}},
  {...receipt,saves:[]},
 ]) {
  const cli = new StreamCli('/unused',async()=>({status:'success',result:value}));
  await assert.rejects(cli.apply({}));
 }
 const retained = {...receipt,publication:{...publication,local:'repair-required'}};
 let calls=0;
 const cli = new StreamCli('/unused',async()=>{calls++;return {status:'success',result:retained};});
 assert.deepEqual(await cli.apply({}),retained);
 assert.equal(calls,1);
});

test('partial reference receipts preserve acceptance and expose safe diagnostics', async () => {
 const key = {kind:'document',repository_url:'repo\u200d',path:'docs/\nplan.md'};
 const rows = [
  {key,desired:null,action:'update',outcome:'superseded',reason:'changed\u001b[31m'},
  {key:{kind:'url',url:'https://example.org/new'},desired:null,action:'add',outcome:'changes',reason:''},
 ];
 const partial = {save:{...save,domain:{outcome:'changes',reference_results:rows}}};
 assert.equal(receiptCanContinue(partial),true);
 assert.equal(receiptHasUnappliedReferences(partial),true);
 assert.equal(resultNeedsAttention(partial.save),true);
 assert.equal(receiptHasWarning(partial),true);
 const diagnostic = `Reference not applied: ${safeReferenceJson(key)}; ${referenceText(rows[0].reason)}`;
 assert.match(receiptText(partial),/^save: changes; local=committed; sharing=pending\n/);
 assert.ok(receiptText(partial).includes(diagnostic));
 assert.equal(receiptText({...partial,warnings:[diagnostic]}).split(diagnostic).length,2);
 assert.equal(receiptCanContinue({...partial,save:{...partial.save,local:'repair-required'}}),false);
 assert.equal(receiptHasWarning({...partial,publication:{...publication,sharing:'failed'}}),true);
 assert.deepEqual(JSON.parse(safeReferenceJson(key)),key);
 assert.equal(referenceText('ordinary 🐟'),'ordinary 🐟');
 assert.equal(referenceText('a\u{e0001}b'),'"a\\udb40\\udc01b"');
 const cli = new StreamCli('/unused',async()=>({status:'success',result:partial}));
 assert.deepEqual(await cli.apply({}),partial);
});

test('reference receipt rows require the exact shape', async () => {
 const valid = {key:{kind:'url',url:'https://example.org/a'},action:'update',outcome:'conflict',reason:'changed'};
 for (const rows of [null, {}, [{...valid,extra:true}], [{...valid,key:{...valid.key,extra:true}}], [{...valid,action:'changed'}], [{...valid,outcome:'unknown'}], [{...valid,reason:null}]]) {
  const value = {...receipt,save:{...save,domain:{outcome:'changes',reference_results:rows}}};
  const cli = new StreamCli('/unused',async()=>({status:'success',result:value}));
  await assert.rejects(cli.apply({}));
 }
});

test('field editing retains input and its receipt when publication needs repair', async () => {
 const session = {id:'session',host_id:'laptop',stream_id:'effort',title:'Before'};
 const context = withPatchSchemas({catalog_source:'source',host_id:'laptop',stream:null,session,title_defaults:{stream:'automatic',session:'automatic'}});
 const edit = new FieldEdit('session',context,'title');edit.value='After';
 const value = {...receipt,publication:{...publication,local:'repair-required'}};
 const receipts=[];
 await assert.rejects(edit.save({apply:async()=>value},r=>receipts.push(r)),/Resubmitting the same captured request is safe/);
 assert.equal(edit.dirty,true);assert.equal(edit.value,'After');assert.equal(receipts[0],value);
});
