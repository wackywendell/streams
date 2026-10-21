import { join } from "node:path";
import { Type } from "typebox";
import { Check } from "typebox/value";
import type { AssistantMessage, Usage } from "@earendil-works/pi-ai";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { helperSessionLogger } from "./agent.ts";
import type { InferenceRuntime, ModelSelection } from "./agent.ts";
import { captureSessions, loadCapture, parseCaptureInventory, privateDirectory, privateWrite } from "./capture.ts";
import { prepare, prepareInput, run } from "./prompt-lab.ts";
import { artifactPath, createArtifact, listArtifacts, loadPrepared, readArtifact, savePrepared } from "./lab-artifacts.ts";
import type { ArtifactKind } from "./lab-artifacts.ts";
import { replayCapture } from "./replay.ts";
import type { StreamCli } from "./streams.ts";

const text = () => Type.String({minLength: 1});
const kind = Type.Union([Type.Literal("capture"), Type.Literal("request"), Type.Literal("run")]);
const model = Type.Object({provider: text(), id: text()}, {additionalProperties: false});
const scope = Type.Union([Type.Literal("session"), Type.Literal("stream"), Type.Literal("all")]);
const object = (properties: Record<string, any>) => Type.Object(properties, {additionalProperties: false});
export const labParameters = {
 capture: object({target: object({kind: Type.Union([Type.Literal("stream"), Type.Literal("workspace")]), id: text()})}),
 prepare: object({capture_id: Type.Optional(text()), request_id: Type.Optional(text()), sample: Type.Optional(Type.Integer({minimum: 0})), task: Type.Optional(Type.Union([Type.Literal("enroll"), Type.Literal("session"), Type.Literal("stream")])), reset: Type.Optional(Type.Boolean()), trim: Type.Optional(Type.Boolean()), prompt: Type.Optional(text())}),
 run: object({request_id: text(), model}),
 replay: object({capture_id: text(), model, reset: Type.Optional(scope), auto: Type.Optional(Type.Boolean()), trim: Type.Optional(Type.Boolean()), prompt: Type.Optional(text())}),
 inspect: object({kind, id: text(), section: Type.Optional(Type.Union([Type.Literal("summary"), Type.Literal("input"), Type.Literal("output"), Type.Literal("errors")])), offset: Type.Optional(Type.Integer({minimum: 0})), limit: Type.Optional(Type.Integer({minimum: 1, maximum: 50000}))}),
 list: object({kind}),
};
export type LabOperation =
 | {operation: "capture"; target: {kind: "stream" | "workspace"; id: string}}
 | {operation: "prepare"; capture_id?: string; request_id?: string; sample?: number; task?: "enroll" | "session" | "stream"; reset?: boolean; trim?: boolean; prompt?: string}
 | {operation: "run"; request_id: string; model: ModelSelection}
 | {operation: "replay"; capture_id: string; model: ModelSelection; reset?: "session" | "stream" | "all"; auto?: boolean; trim?: boolean; prompt?: string}
 | {operation: "inspect"; kind: ArtifactKind; id: string; section?: "summary" | "input" | "output" | "errors"; offset?: number; limit?: number}
 | {operation: "list"; kind: ArtifactKind};
export type LabEnvironment = {root: string; client: Pick<StreamCli, "captureInputs" | "preview">; runtime?: InferenceRuntime; current?: ExtensionContext["sessionManager"]};
function totalUsage(responses: AssistantMessage[]): Usage | undefined {
 if (!responses.length) return undefined;
 const total: Usage = {input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: {input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0}};
 for (const response of responses) {
  for (const key of ["input", "output", "cacheRead", "cacheWrite", "totalTokens"] as const) total[key] += response.usage[key];
  for (const key of ["input", "output", "cacheRead", "cacheWrite", "total"] as const) total.cost[key] += response.usage.cost[key];
 }
 return total;
}
function summary(root: string, kind: ArtifactKind, value: any): any {
 const base = {id: value.id, directory: artifactPath(root, kind, value.id)};
 if (kind === "capture") return {...base, status: value.topology_error || value.unresolved?.length ? "partial" : "completed", unresolved: value.unresolved ?? [], target: value.target, context_mode: value.context_mode, samples: value.samples.map((sample: any, index: number) => ({sample: index, host_id: sample.host_id, native_session_id: sample.native_session_id, title: sample.catalog.session?.title ?? null, open: sample.open, available: !!sample.evidence && !sample.unavailable, error: sample.unavailable, warnings: sample.warnings})), topology_error: value.topology_error, unsupported: value.unsupported};
 if (kind === "request") return {...base, task: value.input.type, source: value.source, trim: value.trim, input_hash: value.input_hash, request_file: join(base.directory, "request.json")};
 if (value.request_id) return {...base, request_id: value.request_id, model: value.model, status: value.status, error: value.error, helper_file: value.helper_file, usage: value.raw_response?.usage};
 return {...base, capture_id: value.capture_id, model: value.model, usage: totalUsage(value.samples.flatMap((s: any) => s.tasks.flatMap((t: any) => t.raw_response ? [t.raw_response] : []))), completed: value.samples.filter((s: any) => s.status === "completed").length, failed: value.samples.filter((s: any) => s.status === "failed").length, skipped: value.samples.filter((s: any) => s.status === "skipped").length, unavailable: value.samples.filter((s: any) => s.status === "unavailable").length, unrun: value.samples.filter((s: any) => ["not-run", "cancelled"].includes(s.status)).length, comparison_file: join(base.directory, "comparison.md")};
}
/** A shared adapter for tools and CLI. Its catalog capabilities are read-only inventory and pure preview. */
export async function executeLab(command: LabOperation, environment: LabEnvironment, signal: AbortSignal): Promise<any> {
 if (!command || !Object.hasOwn(labParameters, command.operation)) throw Error("Choose capture, prepare, run, replay, inspect or list");
 const {operation, ...args} = command;
 if (!Check(labParameters[operation], args)) throw Error(`Invalid ${operation} arguments`);
 signal.throwIfAborted();
 const {root, client, runtime} = environment;
 if (command.operation === "capture") {
  const inventory = parseCaptureInventory(await client.captureInputs(command.target, signal));signal.throwIfAborted();
  if (inventory.target?.kind !== command.target.kind || inventory.target?.id !== command.target.id) throw Error("Capture inventory does not match the requested target");
  const {capture} = captureSessions(inventory, root, environment.current, signal);
  return summary(root, "capture", capture);
 }
 if (command.operation === "prepare") {
  const fork = command.request_id !== undefined;
  if (fork ? command.capture_id !== undefined || command.sample !== undefined || command.task !== undefined || command.reset !== undefined : !command.capture_id || command.sample === undefined || !command.task) throw Error("Prepare from capture_id + sample + task, or from request_id");
  let prepared;
  if (fork) {
   const previous = loadPrepared(root, command.request_id!);
   if (command.trim !== undefined && command.trim !== previous.trim) throw Error("To change trimming, prepare from the full capture instead");
   prepared = prepareInput(previous.input, previous.source, command.prompt ?? previous.model_request.systemPrompt, previous.trim);
  } else prepared = prepare(loadCapture(root, command.capture_id!), {sample: command.sample!, type: command.task!, reset: command.reset}, command.prompt, command.trim ?? true);
  const artifact = savePrepared(root, prepared);
  return summary(root, "request", {...prepared, id: artifact.id});
 }
 if (command.operation === "run" || command.operation === "replay") {
  if (!runtime || !runtime.modelRegistry.find(command.model.provider, command.model.id)) throw Error("Configured model is unavailable; no fallback was used");
  if (command.operation === "replay") {
   if (command.auto && !command.reset) throw Error("--auto requires --reset");
   const {run: replay} = await replayCapture(loadCapture(root, command.capture_id), root, runtime, command.model, command.reset ? {kind: "reset", target: command.reset, auto: command.auto ?? false} : {kind: "report", force: true}, (input, signal) => client.preview(input, signal), signal, {trim: command.trim ?? true, prompt: command.prompt});
   return summary(root, "run", replay);
  }
  const request = loadPrepared(root, command.request_id), artifact = createArtifact(root, "run");
  const helpers = join(artifact.directory, "helpers");privateDirectory(helpers);
  let helper_file: string | undefined;
  const logger = helperSessionLogger({cwd: request.source.cwd, sessionId: request.source.native_session_id, leafId: request.source.leafId}, {directory: helpers, onCreate: value => {helper_file = value.session_file;}});
  const started_at = new Date().toISOString();
  const outcome = await run(request, runtime, command.model, signal, logger);
  const value = {version: 1, id: artifact.id, request_id: command.request_id, model: command.model, started_at, completed_at: new Date().toISOString(), helper_file, ...outcome};
  privateWrite(join(artifact.directory, "run.json"), JSON.stringify(value, null, 2) + "\n");
  return summary(root, "run", value);
 }
 if (command.operation === "list") return {kind: command.kind, artifacts: listArtifacts(root, command.kind).map(id => ({id, directory: artifactPath(root, command.kind, id)}))};
 const value = command.kind === "capture" ? loadCapture(root, command.id) : command.kind === "request" ? {...loadPrepared(root, command.id), id: command.id} : readArtifact(root, command.kind, command.id);
 const section = command.section ?? "summary";
 let content: unknown = summary(root, command.kind, value);
 if (section === "input") content = command.kind === "request" ? {input: value.input, model_request: value.model_request} : command.kind === "run" && value.request_id ? loadPrepared(root, value.request_id) : value;
 if (section === "output") content = command.kind === "run" ? value.request_id ? {raw_response: value.raw_response, value: value.value, status: value.status, error: value.error} : value.samples : value;
 if (section === "errors") content = command.kind === "capture" ? value.samples.map((s: any) => ({sample: s.native_session_id, error: s.unavailable, warnings: s.warnings})) : command.kind === "request" ? [] : value.request_id ? {status: value.status, error: value.error} : value.samples.map((s: any) => ({sample: s.native_session_id, status: s.status, error: s.error, notices: s.notices, tasks: s.tasks.filter((t: any) => t.error)}));
 const text = JSON.stringify(content, null, 2), offset = command.offset ?? 0, limit = command.limit ?? 8000;
 return {kind: command.kind, id: command.id, section, text: text.slice(offset, offset + limit), offset, ...(offset + limit < text.length ? {next_offset: offset + limit} : {})};
}
