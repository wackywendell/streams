import { randomUUID } from "node:crypto";
import { receiptCanContinue, type Document, type ApplyRequest, type LocalContext, type Receipt, type SessionWire, type StreamWire, type Mode } from "./streams.ts";
export const statuses = ["active", "waiting", "blocked", "parked", "done"];
export const titleModes = ["automatic", "manual", "inherit"] as const;
export const fields = {
 stream: ["title", "title_mode", "goal", "goal_short", "goal_mode", "status", "archived", "documents", "issues", "prs"],
 session: ["title", "title_mode", "status", "recap", "recap_short", "state", "state_short", "remaining", "remaining_short"],
};
export function automaticFields(kind: "stream" | "session", before: StreamWire | SessionWire, defaultMode: Mode): string[] {
 const result = (before.title_mode === "inherit" ? defaultMode : before.title_mode) === "automatic" ? ["title"] : [];
 if (kind === "stream") return [...result, ...((before as StreamWire).goal_mode === "automatic" ? ["goal", "goal_short"] : [])];
 return [...result, "status", "recap", "recap_short", "state", "state_short", "remaining", "remaining_short"];
}
const nonempty = (value: unknown, field: string) => {if (typeof value !== "string" || !value.trim() || value.includes("\0")) throw Error(`${field} must be a nonempty string`);};
export function validateChanges(kind: "stream" | "session", before: StreamWire | SessionWire, changes: Record<string, unknown>, actor: "automatic" | "user", defaultMode: Mode): void {
 if (!changes || typeof changes !== "object" || Array.isArray(changes) || !Object.keys(changes).length) throw Error("An edit must contain at least one changed field");
 const goal = Object.hasOwn(changes, "goal") ? changes.goal : (before as StreamWire).goal;
 if (kind === "stream" && Object.hasOwn(changes, "goal_short") && changes.goal_short !== null && goal === null) throw Error("goal_short requires a non-null goal; set the goal first or clear the compact goal");
 const allowed = actor === "automatic" ? automaticFields(kind, before, defaultMode) : fields[kind];
 for (const [key, value] of Object.entries(changes)) {
  if (!allowed.includes(key)) throw Error(`${key} is not allowed for ${actor} ${kind} edits`);
  if (key === "title") nonempty(value, key);
  else if (key === "title_mode" && !titleModes.some(mode => mode === value)) throw Error("title_mode: choose automatic, manual or inherit");
  else if (key === "goal_mode" && !["automatic", "manual"].includes(String(value))) throw Error("goal_mode: choose automatic or manual");
  else if (key === "status" && !statuses.includes(String(value))) throw Error("Choose a valid status");
  else if (key === "archived" && typeof value !== "boolean") throw Error("archived must be boolean");
  else if (["goal", "goal_short", "recap_short", "state_short", "remaining_short"].includes(key) && value !== null) nonempty(value, key);
  else if (["recap", "state", "remaining"].includes(key) && (typeof value !== "string" || value.includes("\0"))) throw Error(`${key} must be text`);
  else if (["documents", "issues", "prs"].includes(key)) validateReferences(key, value, before as StreamWire);
 }
}
export function validateReferences(field: string, rows: unknown, stream: StreamWire): void {
 if (!Array.isArray(rows)) throw Error(`${field} must contain rows`);
 const seen = new Set<string>();
 for (const row of rows) {
  if (!row || typeof row !== "object" || Array.isArray(row)) throw Error("Invalid reference row");
  const keys = field === "documents" ? ["repository", "ref", "path", "description"] : ["url", "description"];
  const allowed = field === "documents" ? [...keys, "commit", "source_session_id"] : keys;
  if (Object.keys(row).some((key) => !allowed.includes(key)) || keys.some((key) => typeof row[key] !== "string" || row[key].includes("\0"))) throw Error("Reference rows require labelled text fields");
  if (field === "documents" && row.commit !== undefined && row.commit !== null) nonempty(row.commit, "commit");
  if (field === "documents" && row.source_session_id !== undefined && row.source_session_id !== null) nonempty(row.source_session_id, "source_session_id");
  let id: string;
  if (field === "documents") {
   if (!Object.hasOwn(stream.repositories, row.repository)) throw Error("Choose an existing repository alias");
   nonempty(row.ref, "ref");nonempty(row.path, "path");
   if (/^(\/|[a-z]:)/i.test(row.path) || row.path.includes("\\") || row.path.split("/").includes("..")) throw Error("Document path must be a portable relative path");
   id = `${row.repository}\0${row.path}`;
  } else {
   let url: URL;try {url = new URL(row.url);} catch {throw Error("Reference URL must be an absolute HTTP(S) URL");}
   if (!["http:", "https:"].includes(url.protocol)) throw Error("Reference URL must use HTTP(S)");
   id = row.url;
  }
  if (seen.has(id)) throw Error("Duplicate reference row");seen.add(id);
 }
}
export function editRequest(context: LocalContext, kind: "stream" | "session", changes: Record<string, unknown>, actor: "automatic" | "user", operationId: string = randomUUID()): ApplyRequest {
 const before = context[kind];if (!before) throw Error(`No attached ${kind}`);
 const host = kind === "session" ? (before as SessionWire).host_id : (before as StreamWire).owner.host_id;
 if (host !== context.host_id) throw Error("Foreign-host records are read-only");
 const title_default = context.title_defaults[kind];
 validateChanges(kind, before, changes, actor, title_default);
 const inputs: ApplyRequest["inputs"] = [];
 if (actor === "automatic" && kind === "stream") {
  if (!context.session) throw Error("Automatic Stream edits require the current Session");
  inputs.push({kind:"session", record:context.session});
 }
 return structuredClone({version:2, catalog_source:context.catalog_source, operation_id:operationId, kind, actor, before, changes, title_default, inputs});
}
export type DocumentProposal = Pick<Document, "repository" | "ref" | "path" | "description">;
export function documentAdditionsRequest(context: LocalContext, additions: DocumentProposal[], operationId: string): ApplyRequest {
 const {stream, session} = context;
 if (!stream || !session || session.stream_id !== stream.id || session.host_id !== context.host_id || stream.owner.host_id !== context.host_id) throw Error("Document additions require a local captured Stream and Session");
 validateReferences("documents", additions, stream);
 if (additions.some(row => Object.keys(row).some(key => !["repository", "ref", "path", "description"].includes(key)))) throw Error("Document provenance is assigned by the save, not the reporting model");
 return structuredClone({version:2, catalog_source:context.catalog_source, operation_id:operationId, kind:"stream", actor:"automatic", before:stream, changes:{document_additions:additions}, title_default:context.title_defaults.stream, inputs:[{kind:"session", record:session}]});
}
export class FieldEdit {
 readonly base: LocalContext;
 value: unknown;
 error?: string;
 constructor(readonly kind: "stream" | "session", context: LocalContext, readonly field: string) {
  this.base = structuredClone(context);this.value = structuredClone((context[kind] as any)?.[field]);
 }
 get dirty(): boolean {return JSON.stringify(this.value) !== JSON.stringify((this.base[this.kind] as any)?.[this.field]);}
 refresh(context: LocalContext): void {if (!this.dirty && !this.error) {Object.assign(this.base, structuredClone(context));this.value = structuredClone((context[this.kind] as any)?.[this.field]);}}
 rebase(context: LocalContext): void {Object.assign(this.base, structuredClone(context));this.error = undefined;}
 async save(cli: {apply(request: ApplyRequest): Promise<Receipt>}, onReceipt?: (receipt: Receipt) => void): Promise<Receipt | undefined> {
  try {
   if (!this.dirty) return;
   const result = await cli.apply(editRequest(this.base, this.kind, {[this.field]: this.value}, "user"));
   onReceipt?.(result);
   if (!result.save?.domain) throw Error("Save outcome is uncertain; receipt and input retained. Inspect catalog repair state before another write. Do not blindly retry.");
   if (result.save.domain.outcome === "conflict" || result.save.domain.outcome === "superseded") throw Error(`Edit ${result.save.domain.outcome}. Your input is retained. Compare the live field, then discard and reopen to use its new base.`);
   if (!receiptCanContinue(result)) throw Error("Local save receipt retained; completion or publication requires inspection before another write. Do not blindly retry.");
   this.error = undefined;return result;
  } catch (error) {this.error = String(error);throw error;}
 }
}
