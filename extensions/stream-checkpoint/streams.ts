import type { ArtifactRequest, ChildRequest, ConfigSnapshot, DomainObject, Runner, TitleDefault, CardMode, CardSettings } from "./child-protocol.ts";
export type { CardMode, CardSettings, ConfigSnapshot } from "./child-protocol.ts";
export type { Runner } from "./child-protocol.ts";

import { parseLocalContext, isResourceKey, isStream as stream, isSession as session, type ResourceKey, type StreamWire, type SessionWire, type LocalContext, type Mode } from "../../integrations/herdr-work-cards/src/streams-context.ts";
export { isReferenceText } from "../../integrations/herdr-work-cards/src/streams-context.ts";
export type { Status, Mode, Reference, ReferenceValue, ReferenceTarget, ResourceKey, StreamWire, SessionWire, LocalContext } from "../../integrations/herdr-work-cards/src/streams-context.ts";
export type ReferenceResult = {key: ResourceKey; action: "add" | "update" | "drop" | "restore" | "delete"; outcome: "changes" | "no-change" | "superseded" | "conflict"; reason: string};
export type LocalStatus = "unchanged" | "persisted" | "committed" | "repair-required" | "uncertain";
export type SharingStatus = "local-only" | "pending" | "confirmed" | "blocked" | "failed" | "uncertain";
export type Result = { warnings?: string[]; local: LocalStatus; sharing?: SharingStatus; domain: { outcome: "changes" | "no-change" | "superseded" | "conflict"; reference_results?: ReferenceResult[]; [key: string]: unknown } | null; detail?: string; [key: string]: unknown };
export type PublicationResult = Result & {domain: null; sharing: SharingStatus};
export type Receipt = {warnings?: string[]; simulated?: true; stream?: StreamWire; session?: SessionWire; publication?: PublicationResult; completion_error?: unknown} & ({operation_id: string; save: Result; saves?: never} | {operation_id?: never; save?: never; saves: {operation_id: string; save: Result}[]});
export type ApplyRequest = {version: 2; catalog_source: string; operation_id: string; kind: "stream" | "session"; actor: "automatic" | "user"; before: StreamWire | SessionWire; changes: Record<string, unknown>; title_default: Mode; inputs: {kind: "stream" | "session"; record: StreamWire | SessionWire}[]};
export class StreamsCliError extends Error {
 constructor(message: string, readonly kind?: string, readonly receipt?: Receipt) { super(message); }
}
const object = (value: unknown): value is Record<string, any> => !!value && typeof value === "object" && !Array.isArray(value);
const text = (value: unknown): value is string => typeof value === "string" && !value.includes("\0");
const unsafeReferenceCharacter = /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u;
export function safeReferenceJson(value: unknown): string {
 return JSON.stringify(value).replace(/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/gu, character => {
  const code = character.codePointAt(0)!;
  const escape = (unit: number) => `\\u${unit.toString(16).padStart(4, "0")}`;
  return code <= 0xffff ? escape(code) : escape(0xd800 + ((code - 0x10000) >> 10)) + escape(0xdc00 + ((code - 0x10000) & 0x3ff));
 });
}
export function referenceText(value: string): string {return unsafeReferenceCharacter.test(value) ? safeReferenceJson(value) : value;}
function referenceKeyJson(key: ResourceKey): string {
 return safeReferenceJson(key.kind === "url" ? {kind:key.kind, url:key.url} : {kind:key.kind, repository_url:key.repository_url, path:key.path});
}
const referenceResult = (value: unknown): value is ReferenceResult => object(value)
 && Object.keys(value).length === 4 && ["key", "action", "outcome", "reason"].every(key => Object.hasOwn(value, key))
 && isResourceKey(value.key) && ["add", "update", "drop", "restore", "delete"].includes(value.action)
 && ["changes", "no-change", "superseded", "conflict"].includes(value.outcome) && typeof value.reason === "string";
const result = (v: unknown): v is Result => object(v) && ["unchanged", "persisted", "committed", "repair-required", "uncertain"].includes(v.local) && (v.sharing === undefined || ["local-only", "pending", "confirmed", "blocked", "failed", "uncertain"].includes(v.sharing)) && (v.domain === null || object(v.domain) && ["changes", "no-change", "superseded", "conflict"].includes(v.domain.outcome) && (v.domain.reference_results === undefined || Array.isArray(v.domain.reference_results) && v.domain.reference_results.every(referenceResult)));
const publication = (v: unknown): v is PublicationResult => result(v) && v.domain === null && v.local !== "uncertain" && v.sharing !== undefined;
function receipt(value: unknown): value is Receipt {
 if (!object(value) || Object.hasOwn(value, "authority") || value.publication !== undefined && !publication(value.publication)) return false;
 if (value.save !== undefined) return value.saves === undefined && text(value.operation_id) && !!value.operation_id && result(value.save);
 if (value.operation_id !== undefined || !Array.isArray(value.saves) || !value.saves.every((s: unknown) => object(s) && text(s.operation_id) && !!s.operation_id && result(s.save))) return false;
 return value.saves.length > 0 || stream(value.stream) && session(value.session) && (value.publication !== undefined || value.completion_error !== undefined);
}
function unappliedReferences(value: Result): ReferenceResult[] {
 const rows = value.domain?.reference_results;
 return Array.isArray(rows) ? rows.filter((row: ReferenceResult) => row.outcome === "conflict" || row.outcome === "superseded") : [];
}
export function receiptHasUnappliedReferences(value: Receipt): boolean {
 const saves = value.saves?.map(item => item.save) ?? (value.save ? [value.save] : []);
 return saves.some(save => unappliedReferences(save).length > 0);
}
export function resultNeedsAttention(value: Result): boolean {
 return !!value.warnings?.length || unappliedReferences(value).length > 0 || ["repair-required", "uncertain"].includes(value.local) || value.sharing !== undefined && ["failed", "blocked", "uncertain"].includes(value.sharing);
}
export function receiptCanContinue(value: Receipt): boolean {
 const saves = value.saves?.map(item => item.save) ?? (value.save ? [value.save] : []);
 return !value.completion_error && value.publication?.local !== "repair-required" && saves.every(save => !["repair-required", "uncertain"].includes(save.local) && !!save.domain && ["changes", "no-change"].includes(save.domain.outcome));
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
  if (purpose === "save" && !receipt(value) || purpose === "publication" && !publication(value)) throw new StreamsCliError("Invalid catalog receipt");
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
 async artifact(request: ArtifactRequest, signal?: AbortSignal): Promise<DomainObject> {
  return this.run({kind:"artifact", request}, "read", {timeout:10000, signal});
 }
 async sync(): Promise<PublicationResult> {return this.run({kind:"sync"}, "publication");}
 async enroll(request: unknown): Promise<Receipt> {return this.run({kind:"enroll", request:request as DomainObject}, "save");}
 async apply(request: ApplyRequest): Promise<Receipt> {return this.run({kind:"apply", request}, "save");}
 private cardSettingsValue(value: any): CardSettings {
  if (!object(value) || !["automatic", "manual"].includes(value.global) || !["inherit", "automatic", "manual"].includes(value.session) || value.effective !== (value.session === "inherit" ? value.global : value.session)) throw new StreamsCliError("Invalid card settings");
  return value;
 }
 async cardSettings(): Promise<CardSettings> {return this.cardSettingsValue(await this.run({kind:"card_settings"}));}
 async setCardMode(target: "global" | "session", mode: CardMode): Promise<CardSettings> {return this.cardSettingsValue(await this.run({kind:"set_card_mode", target, mode}));}
 async setTitleDefault(target: "stream" | "session", mode: TitleDefault): Promise<{stream: TitleDefault; session: TitleDefault}> {
  const value = await this.run({kind:"set_title_default", target, mode});
  if (!object(value.title_defaults) || !["automatic", "manual"].includes(value.title_defaults.stream) || !["automatic", "manual"].includes(value.title_defaults.session)) throw new StreamsCliError("Invalid title defaults");
  return value.title_defaults;
 }
 private configSnapshotValue(value: any): ConfigSnapshot {
  const cfg = value?.config;
  if (!object(cfg) || !object(cfg.naming) || !["automatic", "manual"].includes(cfg.naming.stream) || !["automatic", "manual"].includes(cfg.naming.session)) throw new StreamsCliError("Invalid configuration snapshot");
  if (!object(cfg.herdr) || !object(cfg.herdr.viewer) || !["editor", "less", "hunk", "native"].includes(cfg.herdr.viewer.mode) || !["pane-right", "pane-down", "tab", "floating"].includes(cfg.herdr.viewer.placement)) throw new StreamsCliError("Invalid configuration snapshot");
  if (!object(cfg.herdr.naming) || typeof cfg.herdr.naming.tab !== "boolean" || typeof cfg.herdr.naming.workspace !== "boolean" || !object(cfg.herdr.cards) || !["automatic", "manual"].includes(cfg.herdr.cards.auto_open) || !Number.isInteger(cfg.herdr.timeout_ms) || typeof cfg.herdr.pane_height_fraction !== "number") throw new StreamsCliError("Invalid configuration snapshot");
  const pi = cfg.pi;
  if (!object(pi) || !object(pi.naming) || typeof pi.naming.session !== "boolean" || typeof pi.naming.import_edits !== "boolean") throw new StreamsCliError("Invalid configuration snapshot");
  const rep = pi.reporting;
  if (!object(rep) || !["automatic", "manual"].includes(rep.mode) || !["automatic", "manual"].includes(rep.enrollment) || !Number.isInteger(rep.frequency) || rep.frequency < 1 || !["trimmed", "full"].includes(rep.context) || typeof rep.session_logging !== "boolean" || typeof rep.debug_commands !== "boolean") throw new StreamsCliError("Invalid configuration snapshot");
  if (rep.model !== null && (!object(rep.model) || typeof rep.model.provider !== "string" || !rep.model.provider.trim() || typeof rep.model.id !== "string" || !rep.model.id.trim())) throw new StreamsCliError("Invalid configuration snapshot");
  return cfg as ConfigSnapshot;
 }
 async getConfig(): Promise<ConfigSnapshot> {return this.configSnapshotValue(await this.run({kind:"get_config"}));}
 async setConfig(key: string, value: string): Promise<ConfigSnapshot> {return this.configSnapshotValue(await this.run({kind:"set_config", key, value}));}
 async unsetConfig(key: string): Promise<ConfigSnapshot> {return this.configSnapshotValue(await this.run({kind:"unset_config", key}));}
 async lifecycle(action: "resume" | "park"): Promise<DomainObject> {
  const value = await this.run({kind:"lifecycle", action});
  if (!object(value) || typeof value.session_changed !== "boolean" || typeof value.stream_changed !== "boolean") throw new StreamsCliError("Invalid lifecycle result");
  return value;
 }
}
export function receiptText(value: Receipt): string {
 const saves = value.saves ?? (value.save ? [{operation_id: value.operation_id ?? "", save: value.save}] : []);
 if (!saves.length) return "Already enrolled; no new local saves" + (value.warnings?.length ? "\n" + value.warnings.join("\n") : "");
 if (receiptHasWarning(value)) {
  const diagnostics = saves.flatMap(({save}) => unappliedReferences(save).map(row => `Reference not applied: ${referenceKeyJson(row.key)}; ${referenceText(row.reason)}`)).filter(line => !value.warnings?.includes(line));
  return (value.warnings?.length ? value.warnings.join("\n") + "\n" : "") + saves.map(({operation_id, save}) => `${save.domain?.reference_results === undefined ? operation_id : referenceText(operation_id)}: ${save.domain?.outcome ?? "not reconciled; save outcome uncertain — inspect catalog repair state before another write"}; local=${save.local}${save.sharing ? `; sharing=${save.sharing}` : ""}${save.detail ? `; ${save.detail}` : ""}`).join("\n") + (diagnostics.length ? `\n${diagnostics.join("\n")}` : "") + (value.publication ? `\nPublication: ${JSON.stringify(value.publication)}` : "") + (value.completion_error ? `\nSaved receipt retained; completion problem: ${JSON.stringify(value.completion_error)}. Do not blindly retry.` : "");
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
  return `Work card Stream updated (${value.stream.title})${summary ? `: ${summary}` : ""}`;
 }
 return "Work card updated";
}
