import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, readFileSync, readdirSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { setTimeout as delay } from "node:timers/promises";
import test from "node:test";
import { createRuntime, main, parseArgs, resolveSdkEntry } from "../extensions/stream-checkpoint/lab-cli.mjs";

function fixture(t) {
 const root = realpathSync(mkdtempSync(join(tmpdir(), "streams-prompt-cli-")));
 t.after(() => rmSync(root, {recursive: true, force: true}));
 const sdk = join(root, "sdk.mjs"); writeFileSync(sdk, "");
 return {root, sdk};
}

test("all operations use one strict parser and preserve literal identities", () => {
 assert.deepEqual(parseArgs(["capture", "workspace", "wH", "--json"]), {operation: {operation: "capture", target: {kind: "workspace", id: "wH"}}, json: true});
 assert.deepEqual(parseArgs(["prepare", "c1", "--sample", "0", "--task", "session", "--full", "--reset", "--prompt", "a file"]).operation,
  {operation: "prepare", capture_id: "c1", sample: 0, task: "session", trim: false, reset: true});
 assert.deepEqual(parseArgs(["prepare", "--request", "r1"]).operation, {operation: "prepare", request_id: "r1"});
 assert.deepEqual(parseArgs(["run", "r1", "--model", "provider/model/variant"]).operation, {operation: "run", request_id: "r1", model: {provider: "provider", id: "model/variant"}});
 assert.deepEqual(parseArgs(["replay", "c1", "--model", "p/m", "--reset", "--auto", "--full"]).operation,
  {operation: "replay", capture_id: "c1", model: {provider: "p", id: "m"}, reset: "all", auto: true, trim: false});
 for (const reset of ["session", "stream", "all"]) assert.equal(parseArgs(["replay", "c", "--model", "p/m", "--reset", reset]).operation.reset, reset);
 assert.deepEqual(parseArgs(["inspect", "run", "r1", "--section", "errors", "--offset", "0", "--limit", "15"]).operation,
  {operation: "inspect", kind: "run", id: "r1", section: "errors", offset: 0, limit: 15});
 assert.deepEqual(parseArgs(["list", "request"]).operation, {operation: "list", kind: "request"});
});

test("reject missing values, incompatible flags, model fallback and invalid numbers", () => {
 for (const args of [[], ["nope"], ["capture", "current", "x"], ["capture", "stream"], ["list", "run", "--full"],
  ["prepare", "c", "--task", "session"], ["prepare", "c", "--sample", "-1", "--task", "session"],
  ["prepare", "c", "--sample", "1.2", "--task", "session"], ["prepare", "c", "--sample", "0", "--task", "invalid"],
  ["prepare", "--request", "r", "--full"], ["prepare", "c", "--request", "r"], ["prepare", "--request"],
  ["run", "r"], ["run", "r", "--model", "p/"], ["run", "r", "--model", "p"],
  ["run", "r", "--model", "p/m", "--model", "p/n"], ["replay", "c", "--model", "p/m", "--reset", "invalid"],
  ["replay", "c", "--model", "p/m", "--auto"],
  ["inspect", "run", "r", "--limit", "0"], ["inspect", "run", "r", "--limit", "50001"],
  ["inspect", "run", "r", "--offset", "NaN"], ["inspect", "run", "r", "--limit", "9007199254740992"],
  ["inspect", "run", "r", "--section", "raw"], ["list", "request", "--unknown"], ["list", "--json", "--json", "run"]]) {
  assert.throws(() => parseArgs(args), undefined, JSON.stringify(args));
 }
});

test("help is usable with missing SDK, invalid settings and no services", async t => {
 const {root} = fixture(t); writeFileSync(join(root, "stream-reporting.json"), "invalid");
 let output = "";
 assert.equal(await main(["--help"], {env: {STREAMS_PI_SDK_ENTRY: "/does/not/exist", PI_CODING_AGENT_DIR: root}, stdout: text => output += text, loadModules() {throw Error("must not load");}}), 0);
 assert.match(output, /capture stream\|workspace/);
 const child = spawnSync(process.execPath, [new URL("../extensions/stream-checkpoint/lab-cli.mjs", import.meta.url).pathname, "replay", "--help"], {env: {PATH: "", PI_CODING_AGENT_DIR: root}, encoding: "utf8"});
 assert.equal(child.status, 0); assert.match(child.stdout, /Usage:/);
});

function fakeModules({enabled = true, result = {id: "artifact"}, calls = [], sdk} = {}) {
 return {
  loadSettings(agentDir) {calls.push(["settings", agentDir]); return {debug_commands: enabled};},
  async core() {
   calls.push(["core"]);
   return {StreamCli: class {constructor(cwd) {this.cwd = cwd;}}, async executeLab(operation, environment, signal) {calls.push(["execute", operation, environment, signal]); return result;}};
  },
  async sdk() {calls.push(["sdk"]); if (!sdk) throw Error("unexpected model bootstrap"); return sdk;},
 };
}

async function invoke(t, args, options = {}) {
 const {root, sdk} = fixture(t), calls = []; let output = "", error = "";
 const modules = fakeModules({...options, calls});
 const code = await main(args, {env: {STREAMS_PI_SDK_ENTRY: sdk, PI_CODING_AGENT_DIR: root}, cwd: root,
  loadModules: async () => modules, stdout: text => output += text, stderr: text => error += text});
 return {root, calls, code, output, error};
}

test("debug gate fails closed before core, prompt reads or services", async t => {
 const value = await invoke(t, ["prepare", "--request", "r", "--prompt", "/missing", "--json"], {enabled: false});
 assert.equal(value.code, 2); assert.match(value.output, /disabled/); assert.equal(value.calls.length, 1);
 assert.deepEqual(readdirSync(value.root).sort(), ["sdk.mjs"]);
});

test("nonmodel operations never bootstrap model services or sessions", async t => {
 for (const args of [["capture", "stream", "s"], ["prepare", "--request", "r"], ["list", "capture"], ["inspect", "request", "r"]]) {
  const value = await invoke(t, args);
  assert.equal(value.code, 0, value.error); assert.equal(value.calls.some(call => call[0] === "sdk"), false);
  const call = value.calls.find(call => call[0] === "execute");
  assert.equal(call[2].root, join(value.root, "stream-debug"));
  assert.equal(call[2].client.cwd, value.root); assert.equal(call[2].runtime, undefined); assert.equal(call[2].current, undefined);
  assert.deepEqual(readdirSync(value.root), ["sdk.mjs"]);
 }
});

test("prompt files become literal operation text and output preserves inspection format", async t => {
 const {root, sdk} = fixture(t), prompt = "user {{literal}}\nsecond line\n", calls = [];
 writeFileSync(join(root, "prompt.txt"), prompt); let output = "";
 const code = await main(["prepare", "--request", "r", "--prompt", "prompt.txt", "--json"], {
  cwd: root, env: {STREAMS_PI_SDK_ENTRY: sdk, PI_CODING_AGENT_DIR: root},
  loadModules: async () => fakeModules({calls, result: "input text"}), stdout: text => output += text,
 });
 assert.equal(code, 0); assert.equal(calls.find(call => call[0] === "execute")[1].prompt, prompt);
 assert.equal(JSON.parse(output), "input text");
 const raw = await invoke(t, ["inspect", "request", "r", "--section", "input"], {result: {text: "literal input", offset: 0, next_offset: 10}});
 assert.equal(raw.output, "literal input\n");
});

test("partial capture, failed results and replay failure counts produce nonzero exits", async t => {
 for (const result of [{status: "partial", path: "/saved"}, {status: "failed", path: "/saved"}, {failed: 2}, {unrun: 1}]) {
  const value = await invoke(t, ["list", "run", "--json"], {result});
  assert.equal(value.code, 2); assert.deepEqual(JSON.parse(value.output), result);
 }
});

function serviceSdk({errors = [], diagnostics = [], available = true, calls = []} = {}) {
 return {
  async createAgentSessionServices(options) {calls.push(["services", options]); return {modelRuntime: {provider: "registered"}, diagnostics, resourceLoader: {getExtensions: () => ({errors})}};},
  ModelRegistry: class {
   constructor(runtime) {assert.equal(runtime.provider, "registered");}
   find(provider, id) {calls.push(["find", provider, id]); return available ? {provider, id} : undefined;}
  },
  createAgentSession() {throw Error("must never create a session");},
  SessionManager: {create() {throw Error("must never create session files");}},
 };
}

test("run/replay use explicit provider services without AgentSession/lifecycle/catalog writes", async t => {
 for (const verb of ["run", "replay"]) {
  const calls = [], value = await invoke(t, [verb, "id", "--model", "registered/m"], {sdk: serviceSdk({calls})});
  assert.equal(value.code, 0, value.error);
  assert.deepEqual(calls[0][1].resourceLoaderOptions, {noSkills: true, noPromptTemplates: true, noThemes: true, noContextFiles: true});
  assert.equal(calls[0][1].agentDir, value.root);
  assert.deepEqual(calls[1], ["find", "registered", "m"]);
  assert.deepEqual(value.calls.find(call => call[0] === "execute")[2].runtime.model, {provider: "registered", id: "m"});
  assert.deepEqual(readdirSync(value.root), ["sdk.mjs"]);
 }
});

test("cancellation awaits final artifact save and restores signal handlers", async t => {
 const {root, sdk} = fixture(t), before = process.listenerCount("SIGINT");
 let finished = false, output = "";
 const modules = fakeModules();
 modules.core = async () => ({StreamCli: class {}, async executeLab(_operation, _environment, signal) {
  try {
   process.emit("SIGINT");
   assert.equal(signal.aborted, true);
   await Promise.resolve();
   return {status: "cancelled", helper_file: "saved"};
  } finally {
   writeFileSync(join(root, "cancelled-artifact"), "saved"); finished = true;
  }
 }});
 const code = await main(["list", "run", "--json"], {env: {PI_CODING_AGENT_DIR: root, STREAMS_PI_SDK_ENTRY: sdk},
  loadModules: async () => modules, stdout: text => output += text});
 assert.equal(code, 130); assert.equal(finished, true); assert.equal(readFileSync(join(root, "cancelled-artifact"), "utf8"), "saved");
 assert.equal(JSON.parse(output).status, "cancelled"); assert.equal(process.listenerCount("SIGINT"), before);
});

test("repeated foreground-group cancellation cannot interrupt saved artifacts", {timeout: 10000}, async t => {
 const {root, sdk} = fixture(t), entry = join(root, "cancel.mjs"), artifact = join(root, "run.json");
 writeFileSync(entry, `import {main} from ${JSON.stringify(new URL('../extensions/stream-checkpoint/lab-cli.mjs', import.meta.url).href)};
import {writeFileSync} from 'node:fs';
import {setTimeout} from 'node:timers/promises';
process.exitCode = await main(['list','run','--json'], {
 env: {PI_CODING_AGENT_DIR: ${JSON.stringify(root)}, STREAMS_PI_SDK_ENTRY: ${JSON.stringify(sdk)}},
 loadModules: async () => ({loadSettings: () => ({debug_commands:true}), core: async () => ({StreamCli: class {}, executeLab: async (_op,_env,signal) => {
  const hold = globalThis.setInterval(() => {}, 1000);
  process.stdout.write('READY\\n');
  await new Promise(resolve => signal.addEventListener('abort',resolve,{once:true}));
  await setTimeout(250);globalThis.clearInterval(hold);
  writeFileSync(${JSON.stringify(artifact)}, JSON.stringify({status:'cancelled'}));
  return {status:'cancelled'};
 }})})
});`);
 const child = spawn(process.execPath, [entry], {detached: true, stdio: ['ignore','pipe','pipe']});
 let closed = false, output = '', errors = '';
 const completion = new Promise((resolve, reject) => {child.once('error', reject);child.once('close', (code, signal) => {closed = true;resolve({code, signal});});});
 t.after(() => {if (!closed) try {process.kill(-child.pid, 'SIGKILL');} catch {}});
 child.stderr.on('data', data => {errors += data;});
 await new Promise((resolve, reject) => {
  child.stdout.on('data', data => {output += data;if (output.includes('READY\n')) resolve();});
  completion.then(() => reject(Error(`Child exited before readiness: ${errors}`)), reject);
 });
 process.kill(-child.pid, 'SIGINT');
 await delay(40);
 child.kill('SIGINT');
 const result = await completion;
 assert.equal(result.code, 130, errors);assert.equal(result.signal, null);
 assert.deepEqual(JSON.parse(readFileSync(artifact, 'utf8')), {status:'cancelled'});
});

test("extension diagnostics, extension errors, missing models and stale SDK fail closed", async t => {
 for (const sdk of [{}, serviceSdk({errors: [{error: "broken extension"}]}), serviceSdk({diagnostics: [{type: "error", message: "provider error"}]}), serviceSdk({available: false})]) {
  const value = await invoke(t, ["run", "r", "--model", "p/m"], {sdk});
  assert.equal(value.code, 2); assert.equal(value.calls.some(call => call[0] === "execute"), false);
  assert.match(value.error, /loading failed|Model unavailable|lacks createAgentSessionServices/);
 }
 await assert.rejects(createRuntime(serviceSdk({available: false}), {provider: "p", id: "m"}, "/tmp", "/tmp"), /no fallback/);
});

test("resolves symlinked Node and normal Aube launcher without running shell code", t => {
 const {root} = fixture(t), bin = join(root, "bin"), pkg = join(root, "package");
 mkdirSync(bin); mkdirSync(join(pkg, "dist", "bundle"), {recursive: true});
 writeFileSync(join(pkg, "package.json"), JSON.stringify({name: "@earendil-works/pi-coding-agent", bin: {pi: "dist/bundle/cli.js"}}));
 const index = join(pkg, "dist", "index.js"); writeFileSync(index, "");
 const cli = join(pkg, "dist", "bundle", "cli.js"); writeFileSync(cli, "#!/usr/bin/env node\nthrow Error('must not execute');\n", {mode: 0o700});
 symlinkSync(cli, join(bin, "pi")); assert.equal(resolveSdkEntry({PATH: bin}, root), index);
 const shimdir = join(root, "shim"); mkdirSync(shimdir);
 const shim = join(shimdir, "pi");
 writeFileSync(shim, '#!/bin/sh\n# aube-bin-shim v2 target=../package/dist/bundle/cli.js\nexec node "$basedir/../package/dist/bundle/cli.js" "$@"\n', {mode: 0o700});
 assert.equal(resolveSdkEntry({PATH: shimdir}, root), index);
 writeFileSync(shim, "#!/bin/sh\nexec imaginary-pi\n");
 assert.throws(() => resolveSdkEntry({PATH: shimdir}, root), /Unsupported Pi executable/);
 assert.throws(() => resolveSdkEntry({PATH: ""}, root), /Cannot find executable/);
 assert.throws(() => resolveSdkEntry({STREAMS_PI_SDK_ENTRY: "relative"}, root), /absolute/);
});
