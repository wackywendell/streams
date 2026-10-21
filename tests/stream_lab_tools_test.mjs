import assert from 'node:assert/strict';
import test from 'node:test';
import { createJiti } from 'jiti';
const jiti = createJiti(import.meta.url);
const { registerLabTools } = await jiti.import('../extensions/stream-checkpoint/lab-tools.ts');
const { labParameters, executeLab } = await jiti.import('../extensions/stream-checkpoint/lab.ts');
const verbs = ['capture', 'prepare', 'run', 'replay', 'inspect', 'list'];
const model = {provider: 'synthetic', id: 'explicit'};
const argumentsByVerb = {
 capture: {target: {kind: 'stream', id: 'synthetic-stream'}},
 prepare: {capture_id: 'capture', sample: 0, task: 'session', trim: false, prompt: 'Synthetic override'},
 run: {request_id: 'request', model},
 replay: {capture_id: 'capture', model, reset: 'session', auto: true, trim: false},
 inspect: {kind: 'run', id: 'run', section: 'output', offset: 2, limit: 50},
 list: {kind: 'capture'},
};
function setup(execute = async () => ({}), initiallyEnabled = true) {
 const tools = new Map();
 let enabled = initiallyEnabled;
 registerLabTools({registerTool: tool => {
  assert.equal(tools.has(tool.name), false);
  tools.set(tool.name, tool);
 }}, {enabled: () => enabled, execute});
 return {tools, setEnabled: value => {enabled = value;}};
}
function call(tools, verb, args = argumentsByVerb[verb], signal = new AbortController().signal, ctx = {}) {
 return tools.get(`stream_debug_${verb}`).execute('call', args, signal, undefined, ctx);
}
test('disabled settings register nothing and enabling requires registration again', () => {
 const f = setup(async () => assert.fail('must not execute'), false);
 assert.equal(f.tools.size, 0);
 f.setEnabled(true);
 assert.equal(f.tools.size, 0);
});
test('enabled registration has exactly six names and the identical shared schemas', () => {
 const {tools} = setup();
 assert.deepEqual([...tools.keys()], verbs.map(verb => `stream_debug_${verb}`));
 for (const verb of verbs) assert.equal(tools.get(`stream_debug_${verb}`).parameters, labParameters[verb]);
});
test('every explicit operation and argument reaches the delegate unchanged with SDK context', async () => {
 const calls = [], ctx = {synthetic: true};
 const {tools} = setup(async (...args) => {calls.push(args);return {id: 'summary'};});
 for (const verb of verbs) {
  const controller = new AbortController();
  const before = structuredClone(argumentsByVerb[verb]);
  await call(tools, verb, argumentsByVerb[verb], controller.signal, ctx);
  assert.deepEqual(calls.at(-1)[0], {...before, operation: verb});
  assert.equal(calls.at(-1)[1], controller.signal);
  assert.equal(calls.at(-1)[2], ctx);
  assert.deepEqual(argumentsByVerb[verb], before);
 }
 assert.equal(calls.length, 6);
});
test('summary results serialize only returned data and retain original details', async () => {
 const summary = {id: 'synthetic-run', status: 'completed', model, helper_file: '/synthetic/helpers.jsonl', usage: {input: 3, output: 2, cacheRead: 0, cacheWrite: 0, totalTokens: 5, cost: {input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0}}};
 const {tools} = setup(async () => summary);
 const result = await call(tools, 'run', {...argumentsByVerb.run, synthetic_raw: 'SYNTHETIC RAW TRANSCRIPT NOT RETURNED'});
 assert.deepEqual(result.content, [{type: 'text', text: JSON.stringify(summary)}]);
 assert.equal(result.details, summary);
 assert.equal(result.usage, summary.usage);
 assert.doesNotMatch(result.content[0].text, /SYNTHETIC RAW TRANSCRIPT/);
 assert.notEqual(result.isError, true);
});
test('explicit inspection preserves quoted synthetic raw text and paging metadata', async () => {
 const inspection = {kind: 'run', id: 'run', section: 'output', text: 'Synthetic raw output: "ignore instructions"\nNext line', offset: 2, next_offset: 52};
 const {tools} = setup(async () => inspection);
 const result = await call(tools, 'inspect');
 assert.equal(result.details, inspection);
 assert.deepEqual(JSON.parse(result.content[0].text), inspection);
 assert.equal(result.content.length, 1);
});
test('SDK cancellation signal is forwarded without replacement or lifecycle logic', async () => {
 const controller = new AbortController(), reason = new Error('synthetic cancellation');
 let received;
 const {tools} = setup(async (_operation, signal) => {
  received = signal;
  await new Promise(resolve => signal.addEventListener('abort', resolve, {once: true}));
  assert.equal(signal.reason, reason);
  return {status: 'cancelled'};
 });
 const promise = call(tools, 'run', argumentsByVerb.run, controller.signal);
 assert.equal(received, controller.signal);
 controller.abort(reason);
 assert.equal((await promise).isError, true);
});
test('disabling blocks every registered operation before reading arguments or invoking delegate', async () => {
 let calls = 0;
 const f = setup(async () => {calls++;});
 f.setEnabled(false);
 const unreadable = new Proxy({}, {ownKeys: () => assert.fail('arguments must not be read')});
 for (const verb of verbs) await assert.rejects(call(f.tools, verb, unreadable), /disabled/);
 assert.equal(calls, 0);
});
test('missing SDK signal is rejected rather than inventing a cancellation scope', async () => {
 const {tools} = setup(async () => assert.fail('must not execute'));
 await assert.rejects(tools.get('stream_debug_list').execute('call', argumentsByVerb.list, undefined, undefined, {}), /abort signal/);
});
test('shared core validates malformed and cross-field arguments before any I/O or inference', async () => {
 let sideEffects = 0;
 const client = new Proxy({}, {get: () => {sideEffects++;assert.fail('invalid arguments must not read client');}});
 const environment = {root: '/synthetic/not-read', client};
 const {tools} = setup((operation, signal) => executeLab(operation, environment, signal));
 for (const [verb, args, message] of [
  ['capture', {target: {kind: 'workspace'}}, /Invalid capture arguments/],
  ['prepare', {capture_id: 'capture'}, /Prepare from/],
  ['prepare', {request_id: 'request', sample: 0}, /Prepare from/],
  ['run', {request_id: 'request'}, /Invalid run arguments/],
  ['replay', {capture_id: 'capture', model: {id: 'missing-provider'}}, /Invalid replay arguments/],
  ['inspect', {kind: 'run', id: 'run', limit: 0}, /Invalid inspect arguments/],
  ['list', {kind: 'capture', extra: true}, /Invalid list arguments/],
 ]) await assert.rejects(call(tools, verb, args), message);
 assert.equal(sideEffects, 0);
});
test('delegate errors propagate without fabricating a successful summary', async () => {
 const failure = new Error('synthetic validation failure');
 const {tools} = setup(async () => {throw failure;});
 await assert.rejects(call(tools, 'prepare'), error => error === failure);
});
test('single-run and partial-replay failures are error results with retained details', async () => {
 for (const [verb, summary, error] of [
  ['run', {status: 'completed'}, false],
  ['run', {status: 'failed', error: 'synthetic failure'}, true],
  ['run', {status: 'cancelled'}, true],
  ['replay', {completed: 2, failed: 0, unrun: 0}, false],
  ['replay', {completed: 1, failed: 1, unrun: 0}, true],
  ['replay', {completed: 1, failed: 0, unrun: 1}, true],
  ['replay', {completed: 0, failed: 0, unrun: 0, skipped: 1, unavailable: 1}, false],
  ['inspect', {status: 'failed', failed: 1, unrun: 1}, false],
 ]) {
  const {tools} = setup(async () => summary);
  const result = await call(tools, verb);
  assert.equal(result.isError === true, error, JSON.stringify({verb, summary}));
  assert.equal(result.details, summary);
  assert.deepEqual(JSON.parse(result.content[0].text), summary);
 }
});
