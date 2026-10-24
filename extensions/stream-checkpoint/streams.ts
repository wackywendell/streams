import type { ChildRequest, DomainObject, Runner, TitleDefault } from "./child-protocol.ts";
export type { Runner } from "./child-protocol.ts";

import { parseLocalContext, isStream as stream, isSession as session, type StreamWire, type SessionWire, type LocalContext } from "../../integrations/herdr-work-cards/src/streams-context.ts";
export type { Status, Mode, Link, Document, StreamWire, SessionWire, LocalContext } from "../../integrations/herdr-work-cards/src/streams-context.ts";
export type LocalStatus = "unchanged" | "persisted" | "committed" | "repair-required";
export type SharingStatus = "local-only" | "pending" | "confirmed" | "blocked" | "failed" | "uncertain";
export type Result = { warnings?: string[]; local: LocalStatus; sharing: SharingStatus; domain: { outcome: "changes" | "no-change" | "superseded" | "conflict"; [key: string]: unknown } | null; detail?: string; [key: string]: unknown };
export type PublicationResult = Result & {domain: null};
export type Receipt = {warnings?: string[]; simulated?: true; stream?: StreamWire; session?: SessionWire; authority?: LocalContext["authority"]; publication?: PublicationResult; completion_error?: unknown} & ({operation_id: string; save: Result; saves?: never} | {operation_id?: never; save?: never; saves: {operation_id: string; save: Result}[]});
export type ApplyRequest = {catalog_source: string; operation_id: string; kind: "stream" | "session"; actor: "automatic" | "user"; before: StreamWire | SessionWire; changes: Record<string, unknown>; ownership_token: string | null; inputs: {kind: "stream" | "session"; record: StreamWire | SessionWire}[]};
export class StreamsCliError extends Error {
 constructor(message: string, readonly kind?: string, readonly receipt?: Receipt) { super(message); }
}
const object = (value: unknown): value is Record<string, any> => !!value && typeof value === "object" && !Array.isArray(value);
const text = (value: unknown): value is string => typeof value === "string" && !value.includes("\0");
const result = (v: unknown): v is Result => object(v) && ["unchanged", "persisted", "committed", "repair-required"].includes(v.local) && ["local-only", "pending", "confirmed", "blocked", "failed", "uncertain"].includes(v.sharing) && (v.domain === null || object(v.domain) && ["changes", "no-change", "superseded", "conflict"].includes(v.domain.outcome));
function receipt(value: unknown): value is Receipt {
 if (!object(value) || value.publication !== undefined && (!result(value.publication) || value.publication.domain !== null)) return false;
 if (value.save !== undefined) return value.saves === undefined && text(value.operation_id) && !!value.operation_id && result(value.save);
 if (value.operation_id !== undefined || !Array.isArray(value.saves) || !value.saves.every((s: unknown) => object(s) && text(s.operation_id) && !!s.operation_id && result(s.save))) return false;
 return value.saves.length > 0 || stream(value.stream) && session(value.session) && "authority" in value && (value.publication !== undefined || value.completion_error !== undefined);
}
export function resultNeedsAttention(value: Result): boolean {
 return !!value.warnings?.length || value.local === "repair-required" || ["failed", "blocked", "uncertain"].includes(value.sharing);
}
export function receiptCanContinue(value: Receipt): boolean {
 const saves = value.saves?.map(item => item.save) ?? (value.save ? [value.save] : []);
 return !value.completion_error && value.publication?.local !== "repair-required" && saves.every(save => save.local !== "repair-required" && !!save.domain && ["changes", "no-change"].includes(save.domain.outcome));
}
export function receiptHasWarning(value: Receipt): boolean {
 const saves = value.saves?.map(item => item.save) ?? (value.save ? [value.save] : []);
 return !!value.warnings?.length || !receiptCanContinue(value) || saves.some(resultNeedsAttention) || !!value.publication && resultNeedsAttention(value.publication);
}
export class StreamCli {
 constructor(readonly cwd: string, private readonly runner: Runner) {}
 private async run(request: ChildRequest, purpose: "read" | "save" | "publication" = "read", options?: {timeout: number; signal?: AbortSignal}): Promise<any> {
  options?.signal?.throwIfAborted();
  const outcome = await this.runner(request, options ?? (purpose === "publication" ? {timeout: 10000} : undefined));
  options?.signal?.throwIfAborted();
  if (outcome.status === "error") throw new StreamsCliError(outcome.error.message, outcome.error.kind);
  const value = outcome.result;
  // A warning cannot turn an accepted save into a failure or discard its receipt.
  if (purpose === "save" && !receipt(value) || purpose === "publication" && (!result(value) || value.domain !== null)) throw new StreamsCliError("Invalid catalog receipt");
  return outcome.status === "saved_with_warnings" ? {...value, warnings: outcome.warnings} : value;
 }
 async context(sessionId: string, streamId?: string): Promise<LocalContext> {
  const value = await this.run({kind:"context", ...(streamId ? {stream_id:streamId} : {})});
  let catalog: LocalContext;
  try {catalog = parseLocalContext(value);} catch {throw new StreamsCliError("Invalid local catalog context");}
  if (catalog.session && catalog.session.harness_session_id !== sessionId) throw new StreamsCliError("Catalog context does not match this Pi session");
  return catalog;
 }
 async list(): Promise<StreamWire[]> {
  const value = await this.run({kind:"list_streams"});
  const rows = value?.streams;
  if (!Array.isArray(rows) || !rows.every(stream)) throw new StreamsCliError("Invalid catalog list");
  return rows;
 }
 async captureInputs(target?: {kind: "stream" | "workspace"; id: string}, signal?: AbortSignal): Promise<unknown> {
  return this.run({kind:"capture_inputs", target:target ? target.kind === "stream" ? {kind:"stream", stream_id:target.id} : {kind:"workspace", workspace_id:target.id} : {kind:"inventory"}}, "read", {timeout:10000, signal});
 }
 async preview(input: {catalog: LocalContext; action: "apply" | "enroll"; request: unknown}, signal?: AbortSignal): Promise<{catalog: LocalContext; receipt: Receipt}> {
  const value = await this.run({kind:"preview", catalog:input.catalog, action:input.action, request:input.request as DomainObject}, "read", {timeout:10000, signal});
  if (!object(value) || !receipt(value.receipt) || value.receipt.simulated !== true) throw new StreamsCliError("Invalid simulated catalog receipt");
  return {catalog:parseLocalContext(value.catalog), receipt:value.receipt};
 }
 async captureArrangement(streamId: string, signal?: AbortSignal): Promise<unknown> {return this.run({kind:"capture_workspace", stream_id:streamId}, "read", {timeout:10000, ...(signal ? {signal} : {})});}
 async sync(event: "start" | "shutdown"): Promise<PublicationResult> {return this.run({kind:"sync", event}, "publication");}
 async enroll(request: unknown): Promise<Receipt> {return this.run({kind:"enroll", request:request as DomainObject}, "save");}
 async apply(request: ApplyRequest): Promise<Receipt> {return this.run({kind:"apply", request}, "save");}
 async setTitleDefault(target: "stream" | "session", mode: TitleDefault): Promise<{stream: TitleDefault; session: TitleDefault}> {
  const value = await this.run({kind:"set_title_default", target, mode});
  if (!object(value.title_defaults) || !["automatic", "manual"].includes(value.title_defaults.stream) || !["automatic", "manual"].includes(value.title_defaults.session)) throw new StreamsCliError("Invalid title defaults");
  return value.title_defaults;
 }
}
export function receiptText(value: Receipt): string {
 const saves = value.saves ?? (value.save ? [{operation_id: value.operation_id ?? "", save: value.save}] : []);
 if (!saves.length) return "Already enrolled; no new local saves" + (value.warnings?.length ? "\n" + value.warnings.join("\n") : "");
 if (receiptHasWarning(value)) {
  return (value.warnings?.length ? value.warnings.join("\n") + "\n" : "") + saves.map(({operation_id, save}) => `${operation_id}: ${save.domain?.outcome ?? "not reconciled; save outcome uncertain — inspect catalog repair state before another write"}; local=${save.local}; sharing=${save.sharing}${save.detail ? `; ${save.detail}` : ""}`).join("\n") + (value.publication ? `\nPublication: ${JSON.stringify(value.publication)}` : "") + (value.completion_error ? `\nSaved receipt retained; completion problem: ${JSON.stringify(value.completion_error)}. Do not blindly retry.` : "");
 }
 if (value.saves) {
  const label = [value.stream?.title, value.session?.title].filter(Boolean).join(" / ");
  return label ? `Work card enrolled: ${label}` : "Work card enrolled";
 }
 if (value.save?.domain?.outcome === "no-change") return "Work card up to date (no changes)";
 if (value.session) {
  const summary = value.session.state_short || value.session.state;
  return `Work card updated (${value.session.title})${summary ? `: ${summary}` : ""}`;
 }
 if (value.stream) {
  const summary = value.stream.goal_short || value.stream.goal;
  return `Work card effort updated (${value.stream.title})${summary ? `: ${summary}` : ""}`;
 }
 return "Work card updated";
}
