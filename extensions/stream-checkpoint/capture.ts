import { createHash, randomUUID } from "node:crypto";
import { lstatSync, readFileSync, readdirSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { privateDirectory, privateRead, privateWrite } from "./private-storage.ts";
export { privateDirectory, privateWrite } from "./private-storage.ts";
import { CURRENT_SESSION_VERSION, SessionManager } from "@earendil-works/pi-coding-agent";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { isStream, parseLocalContext } from "../../integrations/herdr-work-cards/src/streams-context.ts";
import type { LocalContext, StreamWire } from "./streams.ts";

export type CaptureTarget = {kind: "stream" | "workspace"; id: string; title: string};
export type Location = {workspace_id: string; tab_id: string; pane_id: string; terminal_id: string};
export type InventorySample = {host_id: string; harness: "pi"; native_session_id: string; open: boolean | null; locations: Location[]; catalog: LocalContext; transcript_file: string | null; unavailable: string | null};
export type CaptureInventory = {version: 1; host_id: string; streams: StreamWire[]; workspaces: {id: string; label: string; stream_id: string | null}[]; topology_error: string | null; target?: CaptureTarget; samples?: InventorySample[]; unsupported?: unknown[]; unresolved?: (Location & {harness: "pi" | "codex" | "claude"; error: string})[]};
export type FrozenSample = InventorySample & {captured_at: string; cwd?: string; snapshot_file?: string; snapshot_hash?: string; projection_source?: "current-session" | "persisted-leaf"; evidence?: {messages: any[]; leafId: string | null}; evidence_hash?: string; warnings: string[]; helper_logs: {file?: string; session_id: string; error?: string}[]};
export type Capture = {version: 1; id: string; captured_at: string; context_mode: "trimmed" | "full"; target: CaptureTarget; host_id: string; workspaces: CaptureInventory["workspaces"]; topology_error: string | null; unsupported: unknown[]; unresolved?: CaptureInventory["unresolved"]; source_files: Record<string, string>; samples: FrozenSample[]};
const object = (value: any): value is Record<string, any> => !!value && typeof value === "object" && !Array.isArray(value);
const identity = (value: any): value is string => typeof value === "string" && !!value.trim() && !/[\x00-\x1f]/.test(value);
export function parseCaptureInventory(value: any): CaptureInventory {
 if (!object(value) || value.version !== 1 || !identity(value.host_id) || !Array.isArray(value.streams) || !Array.isArray(value.workspaces) || !(value.topology_error === null || typeof value.topology_error === "string")) throw Error("Invalid capture inventory");
 // Use the normal wire parser, not a second definition of the card schema.
 for (const stream of value.streams) if (!isStream(stream)) throw Error("Invalid capture Stream");
 for (const workspace of value.workspaces) if (!object(workspace) || !identity(workspace.id) || typeof workspace.label !== "string" || !(workspace.stream_id === null || identity(workspace.stream_id))) throw Error("Invalid capture workspace");
 if (value.target !== undefined && (!object(value.target) || !["stream", "workspace"].includes(value.target.kind) || !identity(value.target.id) || typeof value.target.title !== "string")) throw Error("Invalid capture target");
 if (value.unresolved !== undefined) {
  if (!Array.isArray(value.unresolved)) throw Error("Invalid unresolved capture observations");
  for (const observation of value.unresolved) if (!object(observation) || !["pi", "codex", "claude"].includes(observation.harness) || !["workspace_id", "tab_id", "pane_id", "terminal_id"].every(key => identity(observation[key])) || typeof observation.error !== "string" || !observation.error.trim()) throw Error("Invalid unresolved capture observation");
 }
 if (value.samples !== undefined) {
  if (!value.target || !Array.isArray(value.samples)) throw Error("Invalid capture samples");
  const seen = new Set<string>();
  for (const sample of value.samples) {
   if (!object(sample) || !identity(sample.host_id) || sample.harness !== "pi" || !identity(sample.native_session_id) || ![true, false, null].includes(sample.open) || !Array.isArray(sample.locations) || !(sample.transcript_file === null || identity(sample.transcript_file)) || !(sample.unavailable === null || typeof sample.unavailable === "string")) throw Error("Invalid capture sample");
   for (const location of sample.locations) if (!object(location) || !["workspace_id", "tab_id", "pane_id", "terminal_id"].every(key => identity(location[key]))) throw Error("Invalid captured location");
   sample.catalog = parseLocalContext(sample.catalog);
   const session = sample.catalog.session;
   const liveWorkspace = sample.open === true && sample.locations.some((location: Location) => value.workspaces.some((workspace: CaptureInventory["workspaces"][number]) => workspace.id === location.workspace_id && workspace.stream_id === value.target.id));
   if (sample.catalog.host_id !== value.host_id || session && sample.catalog.stream?.id !== session.stream_id || value.target.kind === "stream" && sample.catalog.stream?.id !== value.target.id && !liveWorkspace) throw Error("Captured catalog/target identity mismatch");
   if (sample.host_id !== value.host_id && (!identity(sample.unavailable) || sample.transcript_file !== null)) throw Error("Foreign capture samples must be unavailable without a local transcript");
   if (session && (session.host_id !== sample.host_id || session.harness !== "pi" || session.harness_session_id !== sample.native_session_id)) throw Error("Captured card/conversation identity mismatch");
   const key = JSON.stringify([sample.host_id, sample.native_session_id]);
   if (seen.has(key)) throw Error("Duplicate capture sample");seen.add(key);
  }
 }
 return value as CaptureInventory;
}
export const digest = (value: string): string => createHash("sha256").update(value).digest("hex");
export function sourceFiles(): Record<string, string> {
 return Object.fromEntries(["agent.ts", "core.ts", "report-workflow.ts", "capture.ts", "prompt-lab.ts", "replay.ts", "private-storage.ts", "helper-log.ts"].map(name => [name, digest(readFileSync(new URL(name, import.meta.url), "utf8"))]));
}
function transcript(content: string, expected: string): {entries: any[]; partial: boolean} {
 const lines = content.split("\n");if (lines.at(-1) === "") lines.pop();
 let partial = false;
 const entries = lines.flatMap((line, index) => {
  try {return [JSON.parse(line)];} catch (error) {
   if (index !== lines.length - 1 || content.endsWith("\n")) throw error;
   partial = true;return [];
  }
 });
 const header = entries[0];
 if (!object(header) || header.type !== "session" || header.id !== expected || typeof header.cwd !== "string") throw Error("Captured transcript identity/header mismatch");
 if (header.version !== undefined && (!Number.isInteger(header.version) || header.version < 1 || header.version > CURRENT_SESSION_VERSION)) throw Error("Unsupported captured transcript version");
 return {entries, partial};
}
/** Snapshot data only. Never open a source file with a mutating SessionManager. */
export function captureSessions(inventory: CaptureInventory, root: string, current?: ExtensionContext["sessionManager"], signal?: AbortSignal, helperRoot?: string): {capture: Capture; directory: string} {
 inventory = parseCaptureInventory(structuredClone(inventory));
 if (!inventory.target || !inventory.samples) throw Error("Select a capture target first");
 signal?.throwIfAborted();
 const id = randomUUID(), directory = join(root, "captures", id);
 privateDirectory(directory, root);
 const capture: Capture = {version: 1, id, captured_at: new Date().toISOString(), context_mode: "full", target: inventory.target, host_id: inventory.host_id, workspaces: structuredClone(inventory.workspaces), topology_error: inventory.topology_error, unsupported: inventory.unsupported ?? [], unresolved: structuredClone(inventory.unresolved ?? []), source_files: sourceFiles(), samples: []};
 for (const [index, source] of inventory.samples.entries()) {
  signal?.throwIfAborted();
  const sample: FrozenSample = {...structuredClone(source), captured_at: new Date().toISOString(), warnings: [], helper_logs: []};
  capture.samples.push(sample);
  const live = source.host_id === inventory.host_id && current?.getSessionId() === source.native_session_id ? current : undefined;
  if (source.unavailable && !live) continue;
  try {
   if (live) {sample.unavailable = null;sample.open = true;}
   else if (source.open) sample.warnings.push("Evidence uses the persisted leaf, not another running process's unsaved branch selection.");
   const content = live ? [live.getHeader(), ...live.getEntries()].map(entry => JSON.stringify(entry)).join("\n") + "\n" : readFileSync(source.transcript_file!, "utf8");
   const parsed = transcript(content, source.native_session_id);
   const manager = live ?? SessionManager.inMemory(parsed.entries[0].cwd, undefined, parsed.entries);
   if (parsed.partial) sample.warnings.push("Incomplete final JSONL record excluded from projected evidence; raw snapshot retained.");
   const evidence = structuredClone({messages: manager.buildSessionProjection().messages, leafId: manager.getLeafId()}), snapshot_file = `session-${index}.jsonl`;
   const metadata = {cwd: manager.getCwd(), projection_source: live ? "current-session" as const : "persisted-leaf" as const, evidence, evidence_hash: digest(JSON.stringify(evidence)), snapshot_file, snapshot_hash: digest(content)};
   privateWrite(join(directory, snapshot_file), content, root);
   Object.assign(sample, metadata);
   for (const entry of parsed.entries) {
    if (entry.type !== "custom" || entry.customType !== "stream-helper-session") continue;
    const link = entry.data;
    if (!object(link) || !identity(link.session_id) || !identity(link.session_file)) {sample.warnings.push("Malformed helper log link omitted.");continue;}
    const log: FrozenSample["helper_logs"][number] = {session_id: link.session_id};sample.helper_logs.push(log);
    try {
     if (!helperRoot || resolve(dirname(link.session_file)) !== resolve(helperRoot)) throw Error("Helper log must be within the reporting sessions root");
     const raw = privateRead(helperRoot, basename(link.session_file), helperRoot), helper = transcript(raw, link.session_id);
     if (!helper.entries.some(e => e.type === "custom" && e.customType === "stream-helper-request" && e.data?.parent_session_id === source.native_session_id)) throw Error("Helper log parent identity mismatch");
     const file = `session-${index}-helper-${sample.helper_logs.length}.jsonl`;
     privateWrite(join(directory, file), raw, root);
     log.file = file;
    } catch (error) {log.error = String(error);}
   }
  } catch (error) {sample.unavailable = String(error);delete sample.evidence;}
 }
 signal?.throwIfAborted();
 privateWrite(join(directory, "capture.json"), JSON.stringify(capture, null, 2) + "\n", root);
 return {capture, directory};
}
const captureId = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/;
export function captureIds(root: string): string[] {
 let entries: string[];
 try {entries = readdirSync(privateDirectory(join(root, "captures"), root, false));} catch (error) {if ((error as NodeJS.ErrnoException).code === "ENOENT") return [];throw error;}
 return entries.filter(id => {
  if (!captureId.test(id)) return false;
  try {const directory = join(root, "captures", id);
   return !lstatSync(directory).isSymbolicLink() && !lstatSync(join(directory, "capture.json")).isSymbolicLink() && lstatSync(join(directory, "capture.json")).isFile();}
  catch (error) {if ((error as NodeJS.ErrnoException).code === "ENOENT") return false;throw error;}
 }).sort();
}
export function loadCapture(root: string, id: string): Capture {
 if (!captureId.test(id)) throw Error("Choose an exact capture ID");
 const directory = join(root, "captures", id);
 const value = JSON.parse(privateRead(directory, "capture.json", root));
 if (!object(value) || value.id !== id || !["trimmed", "full"].includes(value.context_mode) || !identity(value.captured_at) || !object(value.source_files) || !Array.isArray(value.samples)) throw Error("Invalid frozen capture");
 parseCaptureInventory({...value, streams: []});
 for (const sample of value.samples) {
  if (!Array.isArray(sample.warnings) || !Array.isArray(sample.helper_logs) || !(sample.evidence === undefined || object(sample.evidence) && Array.isArray(sample.evidence.messages) && (sample.evidence.leafId === null || identity(sample.evidence.leafId)))) throw Error("Invalid frozen evidence");
  if (sample.evidence && digest(JSON.stringify(sample.evidence)) !== sample.evidence_hash) throw Error("Frozen evidence hash mismatch");
  if (sample.evidence && (sample.unavailable !== null || !identity(sample.snapshot_file) || !/^session-[0-9]+[.]jsonl$/.test(sample.snapshot_file) || !identity(sample.cwd) || !["current-session", "persisted-leaf"].includes(sample.projection_source))) throw Error("Frozen evidence missing source identity");
  if (sample.snapshot_file !== undefined) {
   const raw = privateRead(directory, sample.snapshot_file, root);
   if (digest(raw) !== sample.snapshot_hash) throw Error("Frozen transcript hash mismatch");
   const parsed = transcript(raw, sample.native_session_id);
   if (parsed.entries[0].cwd !== sample.cwd) throw Error("Frozen transcript cwd mismatch");
   if (sample.evidence?.leafId && !parsed.entries.some(entry => entry.id === sample.evidence.leafId)) throw Error("Frozen transcript leaf identity mismatch");
  }
  for (const log of sample.helper_logs) {
   if (!object(log) || !identity(log.session_id) || !(log.error === undefined || typeof log.error === "string")) throw Error("Invalid frozen helper log");
   if (log.file !== undefined) {
    if (!identity(log.file)) throw Error("Invalid frozen helper log file");
    transcript(privateRead(directory, log.file, root), log.session_id);
   }
  }
 }
 return value as Capture;
}
