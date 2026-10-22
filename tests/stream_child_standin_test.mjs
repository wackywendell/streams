import assert from 'node:assert/strict';
import test from 'node:test';
import { spawn } from 'node:child_process';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { createServer } from 'node:net';
import { once } from 'node:events';
import { createInterface } from 'node:readline';
import { join } from 'node:path';
import { tmpdir } from 'node:os';

const standin = new URL('./helpers/streams-child-standin.mjs', import.meta.url).pathname;
const fakeSource = `#!${process.execPath}
const {connect} = require('node:net');
const {spawn} = require('node:child_process');
let input = '';
process.stdin.setEncoding('utf8');process.stdin.on('data', chunk => input += chunk);
process.stdin.on('end', () => {
 const socket = connect(process.env.FAKE_SOCKET);
 socket.on('connect', () => socket.write(JSON.stringify({argv:process.argv.slice(2), input:input ? JSON.parse(input) : null})+'\\n'));
 require('node:readline').createInterface({input:socket}).on('line', line => {
  const reply = JSON.parse(line);
  if (reply.descendant) {
   spawn(process.execPath, ['-e', "const s=require('node:net').connect(process.env.FAKE_SOCKET);s.on('connect',()=>s.write(JSON.stringify({descendant:process.pid})+'\\\\n'));"], {stdio:'inherit'});
   return;
  }
  process.stdout.write(reply.raw ?? JSON.stringify(reply.value));
  process.stderr.write((reply.notices ?? []).join('\\n') + (reply.notices?.length ? '\\n' : ''));
  process.exitCode = reply.code ?? 0;socket.end();
 });
});
`;
function inbox() {
 const entries = [], waiting = [];
 return {
  add(value) {
   const index = waiting.findIndex(waiter => waiter.check(value));
   if (index === -1) entries.push(value);else waiting.splice(index, 1)[0].resolve(value);
  },
  next(check = () => true) {
   const index = entries.findIndex(check);
   if (index !== -1) return Promise.resolve(entries.splice(index, 1)[0]);
   return new Promise(resolve => waiting.push({check, resolve}));
  },
  entries,
 };
}
async function fixture(t, scope = {owner:'first'}) {
 const directory = await mkdtemp(join(tmpdir(), 'streams-child-'));
 const executable = join(directory, 'fake.cjs'), socketPath = join(directory, 'cli.sock');
 await writeFile(executable, fakeSource, {mode:0o755});
 const calls = inbox(), messages = inbox(), sockets = new Set();
 const f = {scope, calls, messages, observed:[]};
 const server = createServer(socket => {
  sockets.add(socket);socket.on('close', () => sockets.delete(socket));
  createInterface({input:socket}).on('line', line => {
   const call = {...JSON.parse(line), socket, reply:value => socket.write(`${JSON.stringify(value)}\n`)};
   if (call.argv?.includes('observe')) {
    f.observed.push(call.argv);
    call.reply(f.scope === null ? {code:1, value:{error:{kind:'unbound', message:'Not enrolled'}}} : {value:f.scope});
   } else calls.add(call);
  });
 });
 server.listen(socketPath);await once(server, 'listening');
 const child = spawn(process.execPath, [standin, 'serve', '--session-id', 'session', '--plugin-root', '/plugin'], {
  env:{...process.env, STREAMS_STANDIN_BIN:executable, FAKE_SOCKET:socketPath}, stdio:['pipe', 'pipe', 'pipe'],
 });
 const exit = once(child, 'exit'), close = once(child, 'close');
 let stderr = '';child.stderr.setEncoding('utf8');child.stderr.on('data', chunk => {stderr += chunk;});
 createInterface({input:child.stdout}).on('line', line => messages.add(JSON.parse(line)));
 t.after(async () => {
  child.kill('SIGKILL');await close;
  for (const socket of sockets) socket.destroy();
  await new Promise(resolve => server.close(resolve));
  await rm(directory, {recursive:true, force:true});
 });
 Object.assign(f, {
  child, exit, close, stderr:() => stderr,
  send:message => child.stdin.write(`${JSON.stringify(message)}\n`),
  request:(id, argv, input) => child.stdin.write(`${JSON.stringify({type:'request', id, argv, ...(input === undefined ? {} : {input})})}\n`),
  response:id => messages.next(message => message.type === 'response' && message.id === id),
  scopeEvent:() => messages.next(message => message.event === 'scope'),
 });
 assert.deepEqual(await messages.next(), {type:'ready', protocol:1});
 const initial = await f.scopeEvent();assert.deepEqual(initial.value, scope);
 assert.deepEqual(f.observed[0], ['native', 'observe', '--session-id', 'session', '--plugin-root', '/plugin', '--json']);
 return f;
}

test('concurrent requests preserve argv, JSON input, exit codes and stderr; EOF drains work', {timeout:10000}, async t => {
 const f = await fixture(t);
 const argv = ['agent', 'context', '--session-id', 'session', '--json'];
 f.request(1, argv, {text:'hello'});
 const slow = await f.calls.next();assert.deepEqual(slow.argv, argv);assert.deepEqual(slow.input, {text:'hello'});
 f.request(2, ['list', '--json']);const fast = await f.calls.next();
 fast.reply({code:2, value:{saved:true}, notices:['first', 'second']});
 assert.deepEqual(await f.messages.next(), {type:'response', id:2, code:2, value:{saved:true}, notices:['first', 'second']});
 f.child.stdin.end();slow.reply({value:['done']});
 assert.deepEqual(await f.response(1), {type:'response', id:1, code:0, value:['done'], notices:[]});
 assert.equal((await f.exit)[0], 0);assert.equal(f.observed.length, 1);
});
test('only changed observations are emitted after native operations and enroll/apply', {timeout:10000}, async t => {
 const f = await fixture(t, null);
 f.request(1, ['agent', 'enroll', '--json']);const enroll = await f.calls.next();
 f.scope = {owner:'enrolled'};enroll.reply({value:{ok:true}});
 await f.response(1);assert.deepEqual((await f.scopeEvent()).value, f.scope);
 f.request(2, ['native', '--session-id', 'session', '--plugin-root', '/plugin', 'project', '--json']);
 (await f.calls.next()).reply({value:{ok:true}});await f.response(2);
 // A later changed event fences the unchanged observation in the serial observation lane.
 f.request(3, ['agent', 'apply', '--json']);const apply = await f.calls.next();
 f.scope = {owner:'moved'};apply.reply({value:{ok:true}});
 await f.response(3);assert.deepEqual((await f.scopeEvent()).value, f.scope);
 f.child.stdin.end();assert.equal((await f.exit)[0], 0);
 assert.equal(f.observed.length, 4);assert.deepEqual(f.messages.entries, []);
});
test('unsupported commands and native session mismatch do not dispatch requested operations', {timeout:10000}, async t => {
 const f = await fixture(t);
 f.request(1, ['debug', 'run']);assert.equal((await f.response(1)).value.error.kind, 'unsupported-command');
 f.request(2, ['native', '--session-id=another', 'open', '--json']);assert.equal((await f.response(2)).value.error.kind, 'session-mismatch');
 f.request(3, ['native', 'open', '--session-id', 'another', '--json']);assert.equal((await f.response(3)).value.error.kind, 'session-mismatch');
 f.child.stdin.end();assert.equal((await f.exit)[0], 0);assert.deepEqual(f.calls.entries, []);
});
test('cancellation kills the CLI process group, including a descendant, and the child remains usable', {timeout:10000}, async t => {
 const f = await fixture(t);
 f.request(1, ['workspace', 'capture', '--json']);const call = await f.calls.next();
 call.reply({descendant:true});const descendant = await f.calls.next(value => !!value.descendant);
 const disconnected = Promise.all([once(call.socket, 'close'), once(descendant.socket, 'close')]);
 f.send({type:'cancel', id:1});assert.equal((await f.response(1)).value.error.kind, 'cancelled');await disconnected;
 f.send({type:'cancel', id:999});f.send({type:'cancel', id:1});
 f.request(2, ['sync', '--json']);(await f.calls.next()).reply({value:{ok:true}});
 assert.equal((await f.response(2)).code, 0);f.child.stdin.end();assert.equal((await f.exit)[0], 0);
});
for (const frame of [
 'not json\n', '{}\n', '{"type":"wat","id":1}\n', '{"type":"cancel"}\n',
 '{"type":"request","id":1,"argv":[42]}\n', '{"type":"cancel","id":1}', '\n',
 '{"type":"request","id":1,"argv":["list"]}\n{"type":"request","id":1,"argv":["list"]}\n',
]) test(`malformed framing fails nonzero: ${JSON.stringify(frame)}`, {timeout:10000}, async t => {
 const f = await fixture(t);f.child.stdin.end(frame);
 assert.equal((await f.exit)[0], 1);assert.ok(f.stderr());
});
