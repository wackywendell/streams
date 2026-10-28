import test from 'node:test';
import assert from 'node:assert/strict';
import { createJiti } from 'jiti';
const jiti = createJiti(import.meta.url);
const { registerArtifactTool } = await jiti.import('../extensions/stream-checkpoint/artifacts.ts');
const { StreamCli } = await jiti.import('../extensions/stream-checkpoint/streams.ts');

function fixture(respond = () => ({artifact:{reference:'host/stream/session/assessment.md'}})) {
 const requests = [];
 const cli = new StreamCli('/working', async (request, options) => {
  requests.push({request, options});
  return {status:'success', result:respond(request)};
 });
 let tool;
 registerArtifactTool({registerTool:value => {tool = value;}}, () => cli);
 const invoke = args => tool.execute('call', args, new AbortController().signal, undefined, {});
 return {tool, requests, invoke};
}

test('artifact tool routes text and file paths through a fake child without reading source files', async () => {
 const f = fixture();
 assert.equal(f.tool.name, 'artifact');
 await f.invoke({action:'write', name:'assessment.md', description:'Findings', text:'useful output'});
 await f.invoke({action:'write', name:'assessment.md', description:'Findings', file:'/not-readable-by-pi/report.md'});
 await f.invoke({action:'write', name:'assessment.md', description:'New description'});
 assert.deepEqual(f.requests.map(({request}) => request), [
  {kind:'artifact', request:{action:'write',name:'assessment.md',description:'Findings',text:'useful output'}},
  {kind:'artifact', request:{action:'write',name:'assessment.md',description:'Findings',file:'/not-readable-by-pi/report.md'}},
  {kind:'artifact', request:{action:'write',name:'assessment.md',description:'New description'}},
 ]);
 assert.match(f.tool.description, /not instructions/);
 assert.match(f.tool.description, /Do not use for design docs, PR descriptions, progress reports/);
});

test('artifact description guidance orients readers rather than recounting provenance', () => {
 const guidance = fixture().tool.parameters.properties.description.description;
 assert.match(guidance, /one short line, at most 240 characters/);
 assert.match(guidance, /teammate who knows the project but has not read this conversation/);
 assert.match(guidance, /subject and useful contents or main finding/);
 assert.match(guidance, /Prefer meaning over provenance/);
 assert.match(guidance, /revision identifiers, model names and process history/);
 assert.match(guidance, /unless they are its subject/);
});

test('artifact reads use bounded child pages and preserve complete references', async () => {
 const f = fixture(() => ({artifact:{reference:'foreign/stream/session/assessment.md'},content:'assessment',next_offset:10,total_bytes:20}));
 const result = await f.invoke({action:'read', reference:'foreign/stream/session/assessment.md'});
 assert.deepEqual(f.requests[0].request, {kind:'artifact',request:{action:'read',reference:'foreign/stream/session/assessment.md',offset:0,limit:32768}});
 assert.match(result.content[0].text, /assessment/);
 assert.equal(result.details.next_offset, 10);
 await f.invoke({action:'read',reference:'foreign/stream/session/assessment.md',offset:10,limit:100});
 assert.equal(f.requests[1].request.request.offset, 10);
});

test('artifact lists stay scoped and invalid tool inputs never reach the child', async () => {
 const f = fixture(() => ({artifacts:[]}));
 await f.invoke({action:'list'});
 await f.invoke({action:'list',scope:'stream',host:'foreign',stream_id:'work'});
 await f.invoke({action:'list',session_id:'other',stream_id:'work'});
 await f.invoke({action:'delete',reference:'host/work/session/assessment.md'});
 assert.deepEqual(f.requests.map(({request}) => request.request), [
  {action:'list_session'}, {action:'list_stream',host:'foreign',stream_id:'work'},
  {action:'list_session',session_id:'other',stream_id:'work'}, {action:'delete',reference:'host/work/session/assessment.md'},
 ]);
 for (const args of [
  {action:'write',name:'assessment.md'},
  {action:'write',name:'assessment.md',description:'Findings',text:'one',file:'two'},
  {action:'write',name:'assessment.md',description:'Findings',text:'one',session_id:'other'},
  {action:'list',scope:'stream'}, {action:'list',scope:'stream',stream_id:'work',session_id:'other'}, {action:'read'}, {action:'delete'},
 ]) await assert.rejects(f.invoke(args));
 assert.equal(f.requests.length, 4);
});

test('artifact child errors throw but accepted-save warnings remain results', async () => {
 let fail = true;
 const cli = new StreamCli('/working', async () => fail
  ? {status:'error',error:{kind:'error',message:'refused'}}
  : {status:'saved_with_warnings',result:{deleted:'host/work/session/name'},warnings:['reference removed; content may remain']});
 await assert.rejects(cli.artifact({action:'delete',reference:'host/work/session/name'}), /refused/);
 fail = false;
 assert.deepEqual((await cli.artifact({action:'delete',reference:'host/work/session/name'})).warnings, ['reference removed; content may remain']);
});
