import { randomUUID } from "node:crypto";
import { isDeepStrictEqual } from "node:util";
import { isReferenceText, receiptCanContinue, receiptHasUnappliedReferences, type ApplyRequest, type LocalContext, type Receipt, type Reference, type ReferenceValue, type ReferenceTarget, type ResourceKey, type SessionWire, type StreamWire, type Mode } from "./streams.ts";
import { isReferenceValue, isResourceKey, parseLocalContext, resourceKey } from "../../integrations/herdr-work-cards/src/streams-context.ts";
export const statuses = ["active", "waiting", "blocked", "parked", "done", "closed"];
export const titleModes = ["automatic", "manual", "inherit"] as const;
export const fields = {
 stream: ["title", "title_mode", "goal", "goal_short", "goal_mode", "status"],
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
  else if (["goal", "goal_short", "recap_short", "state_short", "remaining_short"].includes(key) && value !== null) nonempty(value, key);
  else if (["recap", "state", "remaining"].includes(key) && (typeof value !== "string" || value.includes("\0"))) throw Error(`${key} must be text`);
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
export type AutomaticReferenceValue = Omit<ReferenceValue, "target"> & {target: Exclude<ReferenceTarget, {kind:"document"}> | {kind:"document"; repository_url:string; path:string; ref?:string}};
export type ReferenceDraft = {key: ResourceKey; desired: ReferenceValue | AutomaticReferenceValue | null};
export type ReferenceActor = "automatic" | "user";
export function referenceSelector(key: ResourceKey): string {
 return JSON.stringify(key.kind === "url" ? ["url", key.url] : ["document", key.repository_url, key.path]);
}
export function isAutomaticReferenceValue(value: unknown): value is AutomaticReferenceValue {
 if (!isReferenceValue(value)) return false;
 const target = value.target;
 return target.kind !== "document" || !Object.hasOwn(target, "commit") && !Object.hasOwn(target, "source_session_id") && (!Object.hasOwn(target, "ref") || isReferenceText(target.ref));
}
function valueOf(reference: Reference): ReferenceValue {
 const {revision: _revision, ...value} = reference;return value;
}
function cleanValue(value: ReferenceValue): ReferenceValue {
 if (value.target.kind !== "document") return structuredClone(value);
 const {ref, commit, source_session_id, ...target} = value.target;
 return {...value, target:{...target, ...(ref == null ? {} : {ref}), ...(commit == null ? {} : {commit}), ...(source_session_id == null ? {} : {source_session_id})}};
}
/** Materialise automatic document hints without losing captured recovery context. */
export function materialiseReference(value: AutomaticReferenceValue, current: Reference | undefined, session: SessionWire | null): ReferenceValue {
 if (value.target.kind !== "document") return structuredClone(value);
 const old = current?.target.kind === "document" ? current.target : undefined;
 const ref = value.target.ref ?? old?.ref ?? (!current && session?.checkout?.repository_url === value.target.repository_url ? session.checkout.ref : undefined);
 return cleanValue({...value, target:{...value.target, ...(ref == null ? {} : {ref}), ...(old?.commit == null ? {} : {commit:old.commit}), ...(old?.source_session_id == null ? !current && session ? {source_session_id:session.id} : {} : {source_session_id:old.source_session_id})}});
}
export function referenceRequest(context: LocalContext, actor: ReferenceActor, drafts: ReferenceDraft[], operationId: string = randomUUID()): ApplyRequest | undefined {
 const captured = structuredClone(context);parseLocalContext(captured);
 const {stream, session} = captured;
 if (!stream || stream.owner.host_id !== captured.host_id) throw Error("Reference edits require a local captured Stream");
 if (actor !== "user" && actor !== "automatic") throw Error("Invalid reference actor");
 if (actor === "automatic" && (!session || session.stream_id !== stream.id || session.host_id !== stream.owner.host_id)) throw Error("Automatic reference edits require the captured Session");
 if (!isReferenceText(operationId)) throw Error("operation_id must be nonempty reference text");
 if (!Array.isArray(drafts) || !drafts.length) throw Error("Reference changes must contain at least one row");
 const capturedByKey = new Map(stream.references.map(reference => [referenceSelector(resourceKey(reference.target)), reference]));
 const occupied = new Set<string>();
 const changes: Record<string, unknown>[] = [];
 for (const draft of drafts) {
  if (!draft || typeof draft !== "object" || Object.keys(draft).some(key => key !== "key" && key !== "desired") || !Object.hasOwn(draft, "desired") || !isResourceKey(draft.key)) throw Error("Invalid reference draft");
  const oldKey = referenceSelector(draft.key), current = capturedByKey.get(oldKey);
  if (draft.desired === null) {
   if (actor !== "user" || !current) throw Error("Only a captured reference can be deleted by the user");
  } else if (!(actor === "automatic" ? isAutomaticReferenceValue(draft.desired) : isReferenceValue(draft.desired))) throw Error("Invalid reference value");
  const targetKey = draft.desired === null ? oldKey : referenceSelector(resourceKey(draft.desired.target));
  if ((!current || actor === "automatic") && oldKey !== targetKey) throw Error("Reference target does not match its selector");
  for (const key of new Set([oldKey, targetKey])) {if (occupied.has(key)) throw Error("Reference changes overlap");occupied.add(key);}
  const effective = draft.desired === null ? null : actor === "automatic" ? materialiseReference(draft.desired as AutomaticReferenceValue, current, session) : cleanValue(draft.desired as ReferenceValue);
  if (current && effective !== null && isDeepStrictEqual(cleanValue(valueOf(current)), effective)) continue;
  if (current?.revision === operationId && effective !== null) throw Error("next_revision must differ from the captured revision");
  changes.push(draft.desired === null ? {key:draft.key, desired:null} : {key:draft.key, desired:draft.desired, next_revision:operationId});
 }
 if (!changes.length) return undefined;
 return structuredClone({version:2, catalog_source:captured.catalog_source, operation_id:operationId, kind:"stream", actor, before:stream, changes:{reference_changes:changes}, title_default:captured.title_defaults.stream, inputs:actor === "automatic" ? [{kind:"session", record:session!}] : []});
}
export function automaticReferenceRequest(context: LocalContext, rawProposals: unknown, operationId: string): ApplyRequest | undefined {
 if (rawProposals === undefined) return undefined;
 if (!Array.isArray(rawProposals)) throw Error("reference_proposals must be an array");
 if (!rawProposals.length) return undefined;
 const drafts = rawProposals.map(value => {
  if (!isAutomaticReferenceValue(value)) throw Error("Invalid automatic reference proposal");
  return {key:resourceKey(value.target), desired:value};
 });
 return referenceRequest(context, "automatic", drafts, operationId);
}
export class ReferenceEdit {
 readonly base: LocalContext;
 error?: string;
 constructor(context: LocalContext, readonly key: ResourceKey | null, public actor: ReferenceActor, public desired: unknown) {
  this.base = structuredClone(context);this.key = structuredClone(key);this.desired = structuredClone(desired);
 }
 get dirty(): boolean {
  const current = this.key && this.base.stream?.references.find(row => referenceSelector(resourceKey(row.target)) === referenceSelector(this.key!));
  if (!current) return this.desired !== null;
  if (this.desired === null) return true;
  if (this.actor === "automatic" && isAutomaticReferenceValue(this.desired)) return !isDeepStrictEqual(cleanValue(valueOf(current)), materialiseReference(this.desired, current, this.base.session));
  return !isReferenceValue(this.desired) || !isDeepStrictEqual(cleanValue(valueOf(current)), cleanValue(this.desired));
 }
 rebase(context: LocalContext): void {Object.assign(this.base, structuredClone(context));this.error = undefined;}
 request(operationId: string = randomUUID()): ApplyRequest | undefined {
  const key = this.key ?? (isReferenceValue(this.desired) ? resourceKey(this.desired.target) : null);
  if (!key) throw Error("Choose a valid reference target");
  if (!this.key && this.base.stream?.references.some(row => referenceSelector(resourceKey(row.target)) === referenceSelector(key))) throw Error("Reference already exists; edit the saved row");
  return referenceRequest(this.base, this.actor, [{key, desired:this.desired as ReferenceDraft["desired"]}], operationId);
 }
 async save(cli: {apply(request: ApplyRequest): Promise<Receipt>}, onReceipt?: (receipt: Receipt) => void): Promise<Receipt | undefined> {
  try {
   const request = this.request();if (!request) return;
   const receipt = await cli.apply(request);onReceipt?.(receipt);
   if (receiptHasUnappliedReferences(receipt) || !receiptCanContinue(receipt)) throw Error("Reference edit not applied; input and receipt retained. Inspect the current row before reapplying.");
   this.error = undefined;return receipt;
  } catch (error) {this.error = String(error);throw error;}
 }
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
