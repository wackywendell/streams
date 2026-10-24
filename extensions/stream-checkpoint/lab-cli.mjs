#!/usr/bin/env node
import { accessSync, constants, readFileSync, realpathSync } from "node:fs";
import { delimiter, dirname, isAbsolute, join, resolve } from "node:path";
import { homedir } from "node:os";
import { randomUUID } from "node:crypto";
import { fileURLToPath, pathToFileURL } from "node:url";

export const HELP = `Usage: streams debug <operation> [options]

  capture stream|workspace ID
  prepare CAPTURE --sample N --task enroll|session|stream [--reset] [--full]
  prepare --request REQUEST
  run REQUEST --model PROVIDER/ID
  replay CAPTURE --model PROVIDER/ID [--reset [session|stream|all]] [--auto] [--full]
  inspect capture|request|run ID [--section summary|input|output|errors] [--offset N] [--limit N]
  list capture|request|run

Options:
  --prompt FILE    Read literal prompt text from a UTF-8 file (prepare/replay)
  --full           Include full, untrimmed context (prepare/replay)
  --json           Print structured JSON instead of raw inspection text
  -h, --help       Show help without loading settings or model services

Requires debug_commands=true in PI_CODING_AGENT_DIR/stream-reporting.json.
Runtime overrides: STREAMS_PI_SDK_ENTRY (absolute SDK entry), PI_CODING_AGENT_DIR.
`;

const choices = (value, allowed, label) => {
 if (!allowed.includes(value)) throw Error(`${label} must be ${allowed.join("| ")}`);
 return value;
};
const integer = (value, label) => {
 if (!/^(0|[1-9]\d*)$/.test(value) || !Number.isSafeInteger(Number(value))) throw Error(`${label} requires a nonnegative integer`);
 return Number(value);
};
const model = value => {
 const slash = value.indexOf("/");
 if (slash < 1 || slash === value.length - 1 || /\s/.test(value)) throw Error("--model requires PROVIDER/ID");
 return {provider: value.slice(0, slash), id: value.slice(slash + 1)};
};

// Parsing is deliberately independent of jiti, Pi, settings and the filesystem.
export function parseArgs(argv) {
 if (argv.includes("--help") || argv.includes("-h")) return {help: true};
 const [verb, ...rest] = argv;
 choices(verb, ["capture", "prepare", "run", "replay", "inspect", "list"], "operation");
 const allowed = {
  capture: [], prepare: ["request", "sample", "task", "prompt", "full", "reset"],
  run: ["model"], replay: ["model", "prompt", "full", "reset", "auto"],
  inspect: ["section", "offset", "limit"], list: [],
 }[verb];
 const options = {}, positionals = [];
 for (let index = 0; index < rest.length; index++) {
  const token = rest[index];
  if (!token.startsWith("-")) {positionals.push(token); continue;}
  const name = token.slice(2);
  if (!token.startsWith("--") || ![...allowed, "json"].includes(name)) throw Error(`Unknown option: ${token}`);
  if (name in options) throw Error(`Duplicate option: ${token}`);
  if (["json", "full", "auto"].includes(name) || name === "reset" && verb === "prepare") options[name] = true;
  else if (name === "reset") {
   options.reset = rest[index + 1] && !rest[index + 1].startsWith("-") ? choices(rest[++index], ["session", "stream", "all"], "--reset") : "all";
  } else {
   const value = rest[++index];
   if (!value || value.startsWith("--")) throw Error(`${token} requires a value`);
   options[name] = value;
  }
 }
 const requireCount = count => {if (positionals.length !== count || positionals.some(value => !value.trim())) throw Error(`${verb} requires ${count} positional argument(s); see --help`);};
 const operation = {operation: verb};
 if (verb === "capture") {
  requireCount(2); operation.target = {kind: choices(positionals[0], ["stream", "workspace"], "capture target"), id: positionals[1]};
 } else if (verb === "prepare") {
  if (options.request !== undefined) {
   requireCount(0);
   if (["sample", "task", "reset", "full"].some(key => key in options)) throw Error("--request cannot be combined with --sample, --task, --reset or --full");
   operation.request_id = options.request;
  } else {
   requireCount(1); operation.capture_id = positionals[0];
   operation.sample = integer(options.sample ?? "", "--sample");
   operation.task = choices(options.task, ["enroll", "session", "stream"], "--task");
   if (options.reset) operation.reset = true;
   if (options.full) operation.trim = false;
  }
 } else if (verb === "run" || verb === "replay") {
  requireCount(1); operation[verb === "run" ? "request_id" : "capture_id"] = positionals[0];
  if (!options.model) throw Error(`${verb} requires an explicit --model PROVIDER/ID`);
  operation.model = model(options.model);
  if (verb === "replay") {
   if (options.reset) operation.reset = options.reset;
   if (options.auto) {
    if (!options.reset) throw Error("--auto requires --reset");
    operation.auto = true;
   }
   if (options.full) operation.trim = false;
  }
 } else {
  requireCount(verb === "inspect" ? 2 : 1);
  operation.kind = choices(positionals[0], ["capture", "request", "run"], "artifact kind");
  if (verb === "inspect") {
   operation.id = positionals[1];
   if (options.section !== undefined) operation.section = choices(options.section, ["summary", "input", "output", "errors"], "--section");
   for (const key of ["offset", "limit"]) if (options[key] !== undefined) operation[key] = integer(options[key], `--${key}`);
   if (operation.limit !== undefined && (operation.limit < 1 || operation.limit > 50000)) throw Error("--limit must be between 1 and 50000");
  }
 }
 return {operation, json: !!options.json, ...(options.prompt !== undefined ? {promptFile: options.prompt} : {})};
}

export function resolveExecutable(name, env = process.env, cwd = process.cwd()) {
 const candidates = name.includes("/") ? [resolve(cwd, name)] : (env.PATH ?? "").split(delimiter).map(directory => resolve(cwd, directory, name));
 for (const candidate of candidates) {
  try {accessSync(candidate, constants.X_OK); return realpathSync(candidate);} catch {}
 }
 throw Error(`Cannot find executable ${name}; install Pi or set STREAMS_PI_SDK_ENTRY`);
}

export function resolveSdkEntry(env = process.env, cwd = process.cwd()) {
 if (env.STREAMS_PI_SDK_ENTRY) {
  if (!isAbsolute(env.STREAMS_PI_SDK_ENTRY)) throw Error("STREAMS_PI_SDK_ENTRY must be an absolute file path");
  return realpathSync(env.STREAMS_PI_SDK_ENTRY);
 }
 const executable = resolveExecutable("pi", env, cwd);
 const source = readFileSync(executable, "utf8");
 let cli = executable;
 if (source.startsWith("#!/bin/sh\n# aube-bin-shim v2 target=")) {
  // Aube records its Node entry explicitly; never evaluate the shell wrapper.
  const target = source.split("\n")[1].slice("# aube-bin-shim v2 target=".length);
  if (!target || /[\s\0]/.test(target) || !source.includes('exec node "$basedir/') || !target.endsWith("/dist/bundle/cli.js")) throw Error("Unsupported Pi executable shim; set STREAMS_PI_SDK_ENTRY");
  cli = realpathSync(resolve(dirname(executable), target));
 } else if (!/^#![^\n]*(?:\/|\s)node(?:\s|$)/.test(source.split("\n")[0])) {
  throw Error("Unsupported Pi executable (only Node launchers are supported); set STREAMS_PI_SDK_ENTRY");
 }
 let root;
 if (cli.endsWith("/dist/bundle/cli.js")) root = resolve(dirname(cli), "../..");
 else if (cli.endsWith("/dist/cli.js")) root = resolve(dirname(cli), "..");
 else throw Error("Unsupported Pi Node entry path; set STREAMS_PI_SDK_ENTRY");
 const manifest = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
 if (manifest.name !== "@earendil-works/pi-coding-agent" || !["dist/cli.js", "dist/bundle/cli.js"].includes(manifest.bin?.pi)) throw Error("Pi executable does not belong to a supported SDK package");
 return realpathSync(join(root, "dist/index.js"));
}

export async function loadModules(sdkEntry) {
 const {createJiti} = await import("jiti");
 const selected = createJiti(sdkEntry, {fsCache: false});
 const alias = {"@earendil-works/pi-coding-agent": sdkEntry};
 for (const name of ["@earendil-works/pi-ai", "@earendil-works/pi-agent-core", "@earendil-works/pi-tui", "typebox"]) {
  alias[name] = fileURLToPath(selected.esmResolve(name));
  // Aliases must resolve subpath exports too, not append /compat to index.js.
  if (name === "@earendil-works/pi-ai") alias[`${name}/compat`] = fileURLToPath(selected.esmResolve(`${name}/compat`));
  if (name === "typebox") alias[`${name}/value`] = fileURLToPath(selected.esmResolve(`${name}/value`));
 }
 const jiti = createJiti(import.meta.url, {alias, interopDefault: false, fsCache: false});
 const settings = await jiti.import("./settings.ts");
 return {
  loadSettings: settings.loadSettings,
  async core() {
   const [lab, streams, child] = await Promise.all([jiti.import("./lab.ts"), jiti.import("./streams.ts"), jiti.import("./child.ts")]);
   return {executeLab: lab.executeLab, StreamCli: streams.StreamCli, StreamsChild:child.StreamsChild};
  },
  sdk: () => import(pathToFileURL(sdkEntry).href),
 };
}

export async function createRuntime(sdk, selection, cwd, agentDir, signal) {
 if (typeof sdk.createAgentSessionServices !== "function") throw Error("Selected Pi SDK lacks createAgentSessionServices; install Pi 1.3+ or set STREAMS_PI_SDK_ENTRY");
 const services = await sdk.createAgentSessionServices({cwd, agentDir, modelRuntimeSignal: signal,
  resourceLoaderOptions: {noSkills: true, noPromptTemplates: true, noThemes: true, noContextFiles: true}});
 const errors = [...(services.diagnostics ?? []).filter(item => item.type === "error"), ...(services.resourceLoader.getExtensions().errors ?? [])];
 if (errors.length) throw Error(`Pi provider extension loading failed: ${errors.map(item => item.message ?? item.error ?? String(item)).join("; ")}`);
 const modelRegistry = new sdk.ModelRegistry(services.modelRuntime);
 const selectedModel = modelRegistry.find(selection.provider, selection.id);
 if (!selectedModel) throw Error(`Model unavailable: ${selection.provider}/${selection.id}; no fallback is permitted`);
 return {model: selectedModel, modelRegistry};
}

export async function main(argv = process.argv.slice(2), dependencies = {}) {
 const stdout = dependencies.stdout ?? (text => process.stdout.write(text));
 const stderr = dependencies.stderr ?? (text => process.stderr.write(text));
 let parsed;
 try {
  parsed = parseArgs(argv);
  if (parsed.help) {stdout(HELP); return 0;}
  const env = dependencies.env ?? process.env, cwd = dependencies.cwd ?? process.cwd();
  const agentDir = resolve(env.PI_CODING_AGENT_DIR?.replace(/^~(?=\/|$)/, homedir()) || join(homedir(), ".pi", "agent"));
  const modules = await (dependencies.loadModules ?? loadModules)(resolveSdkEntry(env, cwd));
  if (!modules.loadSettings(agentDir).debug_commands) throw Error("Streams debug commands are disabled; enable debug_commands in stream-reporting.json");
  if (parsed.promptFile !== undefined) parsed.operation.prompt = readFileSync(resolve(cwd, parsed.promptFile), "utf8");
  const {executeLab, StreamCli, StreamsChild} = await modules.core();
  const controller = new AbortController();
  const cancel = () => controller.abort(Error("Streams debug command cancelled"));
  process.on("SIGINT", cancel); process.on("SIGTERM", cancel);
  // Debug has no Pi lifecycle: its lazy child serves explicit capture/preview only.
  const child = new StreamsChild({sessionId:randomUUID(), cwd, env, warning:message => stderr(`streams debug: ${message}\n`)});
  try {
   const environment = {root: join(agentDir, "stream-debug"), client: new StreamCli(cwd, child.run)};
   if (["run", "replay"].includes(parsed.operation.operation)) environment.runtime = await createRuntime(await modules.sdk(), parsed.operation.model, cwd, agentDir, controller.signal);
   const result = await executeLab(parsed.operation, environment, controller.signal);
   const inspection = parsed.operation.operation === "inspect" && parsed.operation.section && parsed.operation.section !== "summary" && typeof result?.text === "string";
   stdout((!parsed.json && inspection ? result.text : typeof result === "string" && !parsed.json ? result : JSON.stringify(result, null, 2)) + "\n");
   if (controller.signal.aborted || result?.status === "cancelled") return 130;
   return result?.status === "partial" || result?.status === "failed" || typeof result?.failed === "number" && result.failed > 0 || typeof result?.unrun === "number" && result.unrun > 0 ? 2 : 0;
  } finally {
   try {await child.stop();} finally {process.removeListener("SIGINT", cancel); process.removeListener("SIGTERM", cancel);}
  }
 } catch (error) {
  if (parsed?.json || argv.includes("--json")) stdout(JSON.stringify({error: {message: error.message ?? String(error)}}) + "\n");
  else stderr(`streams debug: ${error.message ?? String(error)}\n`);
  return 2;
 }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) process.exitCode = await main();
