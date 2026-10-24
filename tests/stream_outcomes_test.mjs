import assert from 'node:assert/strict';
import test from 'node:test';
import { createJiti } from 'jiti';
const jiti = createJiti(import.meta.url);
const { StreamCli, receiptCanContinue, receiptHasWarning, resultNeedsAttention } = await jiti.import('../extensions/stream-checkpoint/streams.ts');
const { FieldEdit } = await jiti.import('../extensions/stream-checkpoint/core.ts');
const save = {local:'committed',sharing:'pending',domain:{outcome:'changes'}};
const publication = {local:'unchanged',sharing:'confirmed',domain:null};
const receipt = {operation_id:'edit',save};

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
  {save},
 ]) {
  const cli = new StreamCli('/unused',async()=>({status:'success',result:value}));
  await assert.rejects(cli.apply({operation_id:'edit'}));
 }
 const retained = {...receipt,publication:{...publication,local:'repair-required'}};
 let calls=0;
 const cli = new StreamCli('/unused',async()=>{calls++;return {status:'success',result:retained};});
 assert.deepEqual(await cli.apply({operation_id:'edit'}),retained);
 assert.equal(calls,1);
});

test('field editing retains input and its receipt when publication needs repair', async () => {
 const session = {id:'session',host_id:'laptop',stream_id:'effort',title:'Before'};
 const context = {catalog_source:'source',host_id:'laptop',stream:null,session,sessions:[session],authority:{host_id:'laptop',token:'creation'}};
 const edit = new FieldEdit('session',context,'title');edit.value='After';
 const value = {...receipt,publication:{...publication,local:'repair-required'}};
 const receipts=[];
 await assert.rejects(edit.save({apply:async()=>value},r=>receipts.push(r)),/Do not blindly retry/);
 assert.equal(edit.dirty,true);assert.equal(edit.value,'After');assert.equal(receipts[0],value);
});
