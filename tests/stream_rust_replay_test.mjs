import assert from 'node:assert/strict';
import test from 'node:test';
import {sessionId} from './helpers/session-id.mjs';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtempSync, mkdirSync, readFileSync, readdirSync, realpathSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createJiti } from 'jiti';
import { childCli } from './helpers/stream-cli.mjs';
const jiti = createJiti(import.meta.url);
const { captureSessions } = await jiti.import('../extensions/stream-checkpoint/capture.ts');
const { replayCapture } = await jiti.import('../extensions/stream-checkpoint/replay.ts');
const packageRoot = fileURLToPath(new URL('../', import.meta.url));
const binary = join(resolve(packageRoot, process.env.CARGO_TARGET_DIR ?? 'target'), 'debug', 'streams');
const model = {provider:'fixture', id:'deterministic', contextWindow:100000};
const response = arguments_ => ({role:'assistant', content:[{type:'toolCall',id:'call',name:'record_card',arguments:arguments_}],api:'fixture',provider:model.provider,model:model.id,stopReason:'toolUse',timestamp:0});
const runtime = generate => ({model, modelRegistry:{find:(provider,id)=>provider===model.provider && id===model.id?model:undefined,complete:async(_model,payload)=>generate(JSON.parse(payload.messages.at(-1).content))}});
async function isolated(run) {
 const root = mkdtempSync(join(realpathSync(tmpdir()), 'streams-rust-replay-'));
 const saved = {...process.env},clients=[];
 try {
  for (const key of Object.keys(process.env)) if (/^(HERDR_|CMUX_|STREAMS_|PI_CODING_AGENT_)/.test(key)) delete process.env[key];
  Object.assign(process.env,{HOME:root,XDG_CONFIG_HOME:join(root,'config'),XDG_DATA_HOME:join(root,'data'),XDG_STATE_HOME:join(root,'state'),PI_CODING_AGENT_DIR:join(root,'pi'),STREAMS_BIN_PATH:binary});
  return await run(root,sessionId=>{const client=childCli(root,sessionId,{...process.env});clients.push(client);return client;});
 } finally {
  await Promise.all(clients.map(client=>client.stop()));
  for (const key of Object.keys(process.env)) delete process.env[key];
  Object.assign(process.env,saved);rmSync(root,{recursive:true,force:true});
 }
}
function cli(root,...args) {
 const result = spawnSync(binary,[...args,'--json'],{cwd:root,encoding:'utf8',timeout:10000});
 assert.ifError(result.error);assert.equal(result.status,0,result.stdout + result.stderr);
 return JSON.parse(result.stdout);
}
function files(root) {
 const result = {};
 function visit(path) {
  const stat = statSync(path);
  if(stat.isDirectory()) for(const name of readdirSync(path)) visit(join(path,name));
  else result[path] = [createHash('sha256').update(readFileSync(path)).digest('hex'),stat.mtimeMs,stat.mode];
 }
 visit(root);return result;
}
function transcript(root,id) {
 const file=join(root,'pi','sessions',`${id}.jsonl`);mkdirSync(dirname(file),{recursive:true});
 writeFileSync(file,[{type:'session',version:3,id,cwd:root,timestamp:new Date(0).toISOString()},{type:'message',id:'message',parentId:null,timestamp:new Date(0).toISOString(),message:{role:'user',content:'Investigate the saved replay boundary',timestamp:0}}].map(entry=>JSON.stringify(entry)).join('\n')+'\n');
 return file;
}

test('production replay invokes real Rust preview for ordered reports without opening poisoned configuration',async()=>isolated(async(root,clientFor)=>{
 const id='00000000-0000-4000-8000-000000000001';
 const stream={id:'effort',title:'Work',title_mode:'automatic',goal:'Old goal',goal_short:null,goal_mode:'automatic',status:'active',owner:{host_id:'host'},revision:1,updated_at_ms:0,repositories:{},references:[]};
 const session={id:sessionId('host','pi',id),host_id:'host',revision:1,updated_at_ms:0,stream_id:stream.id,harness:'pi',harness_session_id:id,title:'Lane',title_mode:'automatic',status:'active',state:'Old state',state_short:null,remaining:'Old remaining',checkout:null};
 const catalog={catalog_source:'frozen-source',host_id:'host',stream,session,sessions:[session],title_defaults:{stream:'automatic',session:'automatic'}};
 const file=transcript(root,id);
 const {capture}=captureSessions({version:1,host_id:'host',streams:[stream],workspaces:[],topology_error:null,target:{kind:'stream',id:stream.id,title:stream.title},samples:[{host_id:'host',harness:'pi',native_session_id:id,open:false,locations:[],catalog,transcript_file:file,unavailable:null}],unsupported:[]},join(root,'private'));
 mkdirSync(join(root,'config','streams'),{recursive:true});writeFileSync(join(root,'config','streams','settings.json'),'INVALID CONFIGURATION');
 const before=files(join(root,'config')), evidence=readFileSync(file,'utf8'), tasks=[], client=clientFor(id);
 const {run}=await replayCapture(capture,join(root,'private'),runtime(input=>{
  tasks.push(input.task);
  if(input.task==='session') return response({changes:{state:'Simulated session state'}});
  const contribution=input.catalog.sessions.find(row=>row.harness_session_id===id);
  assert.ok(contribution,'stream report must include the current session contribution');
  assert.equal(contribution.state,'Simulated session state');
  return response({changes:{goal:'Simulated effort goal'}});
 }),model,{kind:'report',force:true},(input,signal)=>client.preview(input,signal),new AbortController().signal);
 assert.equal(run.samples[0].status,'completed',run.samples[0].error);
 assert.deepEqual(tasks,['session','stream']);assert.equal(run.samples[0].after.stream.goal,'Simulated effort goal');
 assert.ok(run.samples[0].receipts.every(receipt=>receipt.simulated && receipt.save.local==='unchanged'));
 assert.equal(capture.samples[0].catalog.session.state,'Old state');assert.equal(readFileSync(file,'utf8'),evidence);
 assert.deepEqual(files(join(root,'config')),before);assert.equal(readdirSync(root).includes('data'),false);assert.equal(readdirSync(root).includes('state'),false);
}));

test('partial reference replay retains refused restoration and applies an independent update deterministically',async()=>isolated(async(root,clientFor)=>{
 const id='00000000-0000-4000-8000-000000000004',file=transcript(root,id);
 const dropped={name:'Retired',purpose:'Old purpose',role:'dependency',state:'dropped',target:{kind:'link',url:'https://example.org/retired'}};
 const current={name:'Brief',purpose:'Original purpose',role:'brief',state:'active',target:{kind:'issue',url:'https://example.org/brief'}};
 const stream={id:'effort',title:'Work',title_mode:'manual',goal:'Goal',goal_short:null,goal_mode:'manual',status:'active',owner:{host_id:'host'},revision:1,updated_at_ms:0,repositories:{},references:[dropped,current]};
 const session={id:sessionId('host','pi',id),host_id:'host',revision:1,updated_at_ms:0,stream_id:stream.id,harness:'pi',harness_session_id:id,title:'Lane',title_mode:'automatic',status:'active',state:'Old',state_short:null,remaining:'',checkout:null};
 const catalog={catalog_source:'frozen-source',host_id:'host',stream,session,sessions:[session],title_defaults:{stream:'automatic',session:'automatic'},checkouts:[],document_locations:[]};
 const {capture}=captureSessions({version:1,host_id:'host',streams:[stream],workspaces:[],topology_error:null,target:{kind:'stream',id:stream.id,title:stream.title},samples:[{host_id:'host',harness:'pi',native_session_id:id,open:false,locations:[],catalog,transcript_file:file,unavailable:null}],unsupported:[]},join(root,'private'));
 const restore={...dropped},update={...current};restore.state='active';update.purpose='Updated purpose';
 const client=clientFor(id),execute=()=>replayCapture(capture,join(root,'private'),runtime(()=>response({changes:{},reference_proposals:[restore,update]})),model,{kind:'report',force:true},(input,signal)=>client.preview(input,signal),new AbortController().signal);
 const first=(await execute()).run.samples[0],second=(await execute()).run.samples[0];
 assert.equal(first.status,'completed',first.error);assert.deepEqual(first.after.stream.references,second.after.stream.references);
 assert.deepEqual(first.after.stream.references[0],dropped);assert.equal(first.after.stream.references[1].purpose,'Updated purpose');
 const rows=first.receipts[0].save.domain.reference_results;assert.deepEqual(rows.map(row=>row.outcome),['superseded','changes']);
 assert.ok(first.notices.some(text=>text.includes('Reference not applied:')));assert.ok(first.notices.every(text=>!text.startsWith('Reference added:')&&!text.startsWith('Reference dropped:')));
 assert.equal(readdirSync(root).includes('data'),false);assert.equal(readdirSync(root).includes('state'),false);
}));
test('unattached setup uses real Rust enrollment preview before reporting',async()=>isolated(async(root,clientFor)=>{
 const id='00000000-0000-4000-8000-000000000002', file=transcript(root,id);
 const catalog={catalog_source:'frozen-source',host_id:'host',stream:null,session:null,sessions:[],title_defaults:{stream:'automatic',session:'automatic'}};
 const {capture}=captureSessions({version:1,host_id:'host',streams:[],workspaces:[],topology_error:null,target:{kind:'workspace',id:'workspace',title:'Workspace'},samples:[{host_id:'host',harness:'pi',native_session_id:id,open:null,locations:[],catalog,transcript_file:file,unavailable:null}],unsupported:[]},join(root,'private'));
 const tasks=[], client=clientFor(id);
 const {run}=await replayCapture(capture,join(root,'private'),runtime(input=>{
  tasks.push(input.task);return response(input.task==='enroll'?{meaningful:true,stream:{title:'Effort'},session:{title:'Lane'}}:{changes:input.task==='session'?{state:'New state'}:{goal:'New goal'}});
 }),model,{kind:'report',force:true},(input,signal)=>client.preview(input,signal),new AbortController().signal);
 assert.equal(run.samples[0].status,'completed',run.samples[0].error);assert.deepEqual(tasks,['enroll','session','stream']);
 assert.equal(run.samples[0].after.session.state,'New state');assert.equal(run.samples[0].after.stream.goal,'New goal');assert.equal(capture.samples[0].catalog.stream,null);
 assert.equal(readdirSync(root).includes('data'),false);assert.equal(readdirSync(root).includes('state'),false);
}));

test('exact Rust Stream capture does not discover Sessions or transcripts without saved native inputs',async()=>isolated(async(root,clientFor)=>{
 cli(root,'init','--sync','none');const created=cli(root,'create','Captured effort');
 const id='00000000-0000-4000-8000-000000000003';transcript(root,id);
 cli(root,'session','attach',created.stream.id,'--harness','pi','--session-id',id,'--title','Captured lane');
 const before=files(join(root,'data')), stateBefore=readdirSync(root).includes('state')?files(join(root,'state')):null, client=clientFor(id);
 const inventory=await client.captureInputs({kind:'stream',id:created.stream.id});
 assert.equal(inventory.streams.length,1);assert.equal(inventory.streams[0].id,created.stream.id);
 assert.deepEqual(inventory.samples,[],'an exact Stream key is not a contributor roster or transcript locator');
 assert.ok(inventory.topology_error);
 assert.deepEqual(files(join(root,'data')),before);assert.deepEqual(readdirSync(root).includes('state')?files(join(root,'state')):null,stateBefore);
}));
