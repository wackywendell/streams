#!/usr/bin/env node
// Test-only bridge to the CLI; no subscriptions or polling.
import { spawn } from 'node:child_process';
import { isAbsolute } from 'node:path';
import { isDeepStrictEqual } from 'node:util';

const binary = process.env.STREAMS_STANDIN_BIN;
const args = process.argv.slice(2);
const sessionId = args[2], pluginRoot = args[4];
const children = new Set(), requests = new Map(), seen = new Set();
let failed = false, previousScope, observations = Promise.resolve();
const errorValue = (kind, message) => ({error:{kind, message}});
const send = message => {if (!failed) process.stdout.write(`${JSON.stringify(message)}\n`);};
function kill(child) {
 if (!child?.pid) return;
 try {process.kill(-child.pid, 'SIGKILL');} catch (error) {if (error.code !== 'ESRCH') throw error;}
}
function invoke(argv, input, request) {
 return new Promise(resolve => {
  const child = spawn(binary, argv, {detached:true, stdio:['pipe', 'pipe', 'pipe']});
  children.add(child);
  if (request) request.child = child;
  let stdout = '', stderr = '', spawnError;
  child.stdout.setEncoding('utf8');child.stderr.setEncoding('utf8');
  child.stdout.on('data', chunk => {stdout += chunk;});
  child.stderr.on('data', chunk => {stderr += chunk;});
  child.on('error', error => {spawnError = error;});
  child.stdin.on('error', error => {if (error.code !== 'EPIPE') spawnError = error;});
  child.on('close', code => {
   children.delete(child);
   if (request) request.child = undefined;
   const notices = stderr.split(/\r?\n/);if (notices.at(-1) === '') notices.pop();
   let value;
   if (request?.cancelled) value = errorValue('cancelled', 'Request cancelled');
   else if (spawnError) value = errorValue('spawn-error', spawnError.message);
   else {
    try {value = JSON.parse(stdout);} catch {value = errorValue('invalid-output', 'CLI stdout was not JSON');code = 1;}
   }
   resolve({code:request?.cancelled || spawnError ? 1 : (code ?? 1), value, notices});
  });
  child.stdin.end(input === undefined ? undefined : `${JSON.stringify(input)}\n`);
 });
}
function observe() {
 observations = observations.then(async () => {
  if (failed) return;
  const result = await invoke(['native', 'observe', '--session-id', sessionId, '--plugin-root', pluginRoot, '--json']);
  const scope = result.code === 0 && result.value !== null
   ? {type:'event', event:'scope', value:result.value}
   : {type:'event', event:'scope', value:null, reason:result.value?.error?.message ?? (result.notices.join('\n') || 'No owner observation')};
  if (!isDeepStrictEqual(scope, previousScope)) {previousScope = scope;send(scope);}
 });
 return observations;
}
function optionValues(argv, name) {
 const values = [];
 for (let i = 0; i < argv.length; i++) {
  if (argv[i] === name) values.push(argv[++i]);
  else if (argv[i].startsWith(`${name}=`)) values.push(argv[i].slice(name.length + 1));
 }
 return values;
}
function command(argv) {
 // Native global options can appear on either side of the subcommand.
 const positional = [];
 for (let i = 0; i < argv.length; i++) {
  if (['--session-id', '--plugin-root', '--state-dir', '--expect'].includes(argv[i])) {i++;continue;}
  if (argv[i].startsWith('--')) continue;
  positional.push(argv[i]);
  if (positional.length === 2) break;
 }
 return positional;
}
async function dispatch(message, request) {
 const [group, action] = command(message.argv);
 const supported = group === 'native' ? ['observe', 'open', 'close', 'project', 'capture'].includes(action)
  : group === 'agent' ? ['context', 'capture-inputs', 'preview', 'enroll', 'apply'].includes(action)
  : group === 'workspace' ? action === 'capture' : ['list', 'sync'].includes(group);
 let result;
 if (!supported) result = {code:1, value:errorValue('unsupported-command', 'Command is not supported by the child'), notices:[]};
 else if (group === 'native' && optionValues(message.argv, '--session-id').some(value => value !== sessionId)) {
  result = {code:1, value:errorValue('session-mismatch', 'Native session differs from the child session'), notices:[]};
 } else result = await invoke(message.argv, message.input, request);
 send({type:'response', id:message.id, ...result});
 if (supported && !request.cancelled && (group === 'native' || (group === 'agent' && ['enroll', 'apply'].includes(action)))) await observe();
}
function accept(line) {
 const message = JSON.parse(line);
 if (!message || typeof message !== 'object' || Array.isArray(message)
  || !['request', 'cancel'].includes(message.type) || !Number.isSafeInteger(message.id) || message.id <= 0) throw Error('Invalid protocol message');
 if (message.type === 'cancel') {
  const request = requests.get(message.id);
  if (request) {request.cancelled = true;kill(request.child);}
  return;
 }
 if (!Array.isArray(message.argv) || !message.argv.every(arg => typeof arg === 'string') || seen.has(message.id)) throw Error('Invalid request');
 seen.add(message.id);
 const request = {cancelled:false};
 requests.set(message.id, request);
 request.done = dispatch(message, request).finally(() => {requests.delete(message.id);});
}
async function main() {
 if (!binary || !isAbsolute(binary)) throw Error('STREAMS_STANDIN_BIN must be an absolute CLI path');
 if (args.length !== 5 || args[0] !== 'serve' || args[1] !== '--session-id' || !sessionId || args[3] !== '--plugin-root' || !pluginRoot) {
  throw Error('Usage: streams-child-standin.mjs serve --session-id <id> --plugin-root <root>');
 }
 send({type:'ready', protocol:1});
 await observe();
 let buffer = '';
 process.stdin.setEncoding('utf8');
 for await (const chunk of process.stdin) {
  buffer += chunk;
  let newline;
  while ((newline = buffer.indexOf('\n')) !== -1) {
   const line = buffer.slice(0, newline);buffer = buffer.slice(newline + 1);accept(line);
  }
 }
 if (buffer) throw Error('Unterminated protocol frame');
 await Promise.all([...requests.values()].map(request => request.done));
 await observations;
}
main().catch(async error => {
 failed = true;
 process.stderr.write(`${error.message}\n`);
 for (const child of children) kill(child);
 await Promise.allSettled([...requests.values()].map(request => request.done));
 process.exitCode = 1;
});
