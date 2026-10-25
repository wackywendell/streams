import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createJiti } from 'jiti';
import { SessionManager } from '@earendil-works/pi-coding-agent';
const jiti = createJiti(import.meta.url);
const { captureSessions, loadCapture, parseCaptureInventory } = await jiti.import(new URL('../extensions/stream-checkpoint/capture.ts',import.meta.url).pathname);
const assistant = content => ({role:'assistant',api:'openai-responses',provider:'test',model:'model',timestamp:1,stopReason:'toolUse',usage:{input:0,output:0,cacheRead:0,cacheWrite:0,totalTokens:0,cost:{input:0,output:0,cacheRead:0,cacheWrite:0,total:0}},content});
const call = id => ({type:'toolCall',id,name:'read',arguments:{path:`${id}.txt`}});
const result = (id,text) => ({role:'toolResult',toolCallId:id,toolName:'read',isError:false,timestamp:2,content:[{type:'text',text}]});
const snapshot = manager => [manager.getHeader(),...manager.getEntries()].map(entry=>JSON.stringify(entry)).join('\n')+'\n';
const evidence = manager => structuredClone({messages:manager.buildSessionProjection().messages,leafId:manager.getLeafId()});
function fixture() {
 const dir=mkdtempSync(join(tmpdir(),'streams-capture-full-')),root=join(dir,'debug'),source=join(dir,'source.jsonl');
 const manager=SessionManager.inMemory(dir,{id:'native'});
 const inventory={version:1,host_id:'host',streams:[],workspaces:[],topology_error:null,target:{kind:'workspace',id:'workspace',title:'Work'},samples:[{host_id:'host',harness:'pi',native_session_id:'native',open:false,locations:[],catalog:{catalog_source:'source',host_id:'host',stream:null,session:null,sessions:[],title_defaults:{stream:'automatic',session:'automatic'}},transcript_file:source,unavailable:null}]};
 return {dir,root,source,manager,inventory,dispose:()=>rmSync(dir,{recursive:true,force:true})};
}
test('unresolved native observations preserve every supported harness without hiding healthy samples',()=>{
 const f=fixture();try {
  f.inventory.unresolved=['pi','codex','claude'].map((harness,index)=>({workspace_id:'workspace',tab_id:'tab',pane_id:`pane-${index}`,terminal_id:`terminal-${index}`,harness,error:'Native identity is unavailable'}));
  assert.deepEqual(parseCaptureInventory(structuredClone(f.inventory)).unresolved,f.inventory.unresolved);
  writeFileSync(f.source,snapshot(f.manager));
  const {capture}=captureSessions(f.inventory,f.root);
  assert.deepEqual(loadCapture(f.root,capture.id).unresolved,f.inventory.unresolved);
  assert.equal(capture.samples[0].unavailable,null);
  f.inventory.unresolved[0].harness='unknown';
  assert.throws(()=>parseCaptureInventory(f.inventory),/Invalid unresolved/);
 }finally{f.dispose();}
});
test('capture freezes unfiltered wide content, images, complete and incomplete tool batches',()=>{
 const f=fixture();try {
  const wide='Wide evidence '.repeat(4000),image={type:'image',data:'aW1hZ2U=',mimeType:'image/png'};
  f.manager.appendMessage({role:'user',timestamp:1,content:[{type:'text',text:wide},image]});
  f.manager.appendMessage(assistant([{type:'thinking',thinking:'Reasoning evidence'},call('complete')]));
  f.manager.appendMessage({...result('complete',wide),content:[{type:'text',text:wide},image]});
  f.manager.appendCustomMessageEntry('stream-private-evidence','Keep cleanup at preparation',false,{});
  f.manager.appendMessage(assistant([call('partial'),call('missing')]));
  f.manager.appendMessage(result('partial','Only one result in this unfinished batch'));
  f.manager.appendMessage(result('orphan','No matching call'));
  const expected=evidence(f.manager),raw=snapshot(f.manager);
  writeFileSync(f.source,raw);const before=statSync(f.source);
  for(const current of [undefined,f.manager]) {
   const {capture,directory}=captureSessions(f.inventory,f.root,current),sample=capture.samples[0];
   assert.equal(capture.context_mode,'full');assert.equal(sample.unavailable,null);
   assert.deepEqual(sample.evidence,expected);
   assert.equal(sample.evidence.messages[0].content[0].text,wide);
   assert.deepEqual(sample.evidence.messages[0].content[1],image);
   assert.ok(sample.evidence.messages.some(message=>message.role==='toolResult'&&message.toolCallId==='complete'));
   assert.ok(sample.evidence.messages.some(message=>message.content?.some?.(block=>block.type==='toolCall'&&block.id==='missing')));
   assert.ok(sample.evidence.messages.some(message=>message.role==='toolResult'&&message.toolCallId==='orphan'));
   assert.ok(sample.evidence.messages.some(message=>message.customType==='stream-private-evidence'));
   assert.equal(readFileSync(join(directory,sample.snapshot_file),'utf8'),raw);
   assert.deepEqual(loadCapture(f.root,capture.id),capture);
   sample.evidence.messages[0].content[0].text='mutated frozen copy';
   assert.deepEqual(evidence(f.manager),expected);
  }
  assert.equal(readFileSync(f.source,'utf8'),raw);assert.equal(statSync(f.source).mtimeMs,before.mtimeMs);assert.equal(statSync(f.source).mode,before.mode);
 }finally{f.dispose();}
});
test('capture preserves distinct current and persisted branch leaves while raw snapshots retain both paths',()=>{
 const f=fixture();try {
  const common=f.manager.appendMessage({role:'user',content:'Common root',timestamp:1});
  const selected=f.manager.appendMessage({role:'user',content:'Selected current branch',timestamp:2});
  const currentEvidence=evidence(f.manager);
  f.manager.branch(common);
  const persisted=f.manager.appendMessage({role:'user',content:'Persisted sibling branch',timestamp:3});
  const persistedEvidence=evidence(f.manager),raw=snapshot(f.manager);
  writeFileSync(f.source,raw);const before=statSync(f.source);
  f.manager.branch(selected);const entries=structuredClone(f.manager.getEntries());
  for(const [current,expected,projection] of [[f.manager,currentEvidence,'current-session'],[undefined,persistedEvidence,'persisted-leaf']]) {
   const {capture,directory}=captureSessions(f.inventory,f.root,current),sample=capture.samples[0];
   assert.equal(sample.projection_source,projection);assert.deepEqual(sample.evidence,expected);
   assert.equal(sample.evidence.messages.length,2);assert.equal(sample.evidence.leafId,current?selected:persisted);
   assert.equal(readFileSync(join(directory,sample.snapshot_file),'utf8'),raw);
   assert.match(raw,/Selected current branch/);assert.match(raw,/Persisted sibling branch/);
   assert.equal(f.manager.getLeafId(),selected);assert.deepEqual(f.manager.getEntries(),entries);
  }
  assert.equal(readFileSync(f.source,'utf8'),raw);assert.equal(statSync(f.source).mtimeMs,before.mtimeMs);assert.equal(statSync(f.source).mode,before.mode);
 }finally{f.dispose();}
});
test('capture keeps partial final raw records and reads legacy trimmed and full manifests without rewriting them',()=>{
 const f=fixture();try {
  f.manager.appendMessage({role:'user',content:'Complete record',timestamp:1});
  const raw=snapshot(f.manager)+'{"type":"message"';writeFileSync(f.source,raw);
  const {capture,directory}=captureSessions(f.inventory,f.root),sample=capture.samples[0],manifest=join(directory,'capture.json');
  assert.deepEqual(sample.evidence,evidence(f.manager));assert.match(sample.warnings[0],/Incomplete final JSONL/);
  assert.equal(readFileSync(join(directory,sample.snapshot_file),'utf8'),raw);assert.equal(readFileSync(f.source,'utf8'),raw);
  for(const mode of ['trimmed','full']) {
   const legacy={...capture,context_mode:mode},content=JSON.stringify(legacy,null,2)+'\n';
   writeFileSync(manifest,content);const before=statSync(manifest);
   assert.deepEqual(loadCapture(f.root,capture.id),legacy);
   assert.equal(readFileSync(manifest,'utf8'),content);assert.equal(statSync(manifest).mtimeMs,before.mtimeMs);
  }
 }finally{f.dispose();}
});
