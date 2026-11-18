import { isDeepStrictEqual } from "node:util";
import { receiptCanContinue, receiptHasUnappliedReferences, type ApplyRequest, type LocalContext, type Receipt, type ReferenceValue, type ResourceKey, type StreamCli } from "./streams.ts";
import { resourceKey, type FieldRow, type FieldSchema } from "./wire.ts";
type Kind = "stream" | "session";
export type ReferenceActor = "automatic" | "user";
/** The fields `actor` may change in the captured record, in display order, as Rust reports them. */
export function patchFields(context: LocalContext, kind: Kind, actor: ReferenceActor): FieldRow[] {
 return context.patch_schemas[kind]?.[actor] ?? [];
}
/** The choices a field offers, or undefined for free text. */
export function fieldChoices(context: LocalContext, kind: Kind, field: string): string[] | undefined {
 return patchFields(context, kind, "user").find(row => row.field === field)?.schema.enum;
}
/** Whether a field's schema accepts null, which clears it. */
export function allowsNull(schema: FieldSchema): boolean {
 return (schema.anyOf ?? [schema]).some(option => option.type === "null");
}
/** The free-text form a field's schema accepts, if it accepts any. */
export function textSchema(schema: FieldSchema): FieldSchema | undefined {
 return (schema.anyOf ?? [schema]).find(option => option.type === "string" && !option.enum);
}
/** Whether a field can be cleared to null. */
export function fieldClears(context: LocalContext, kind: Kind, field: string): boolean {
 const row = patchFields(context, kind, "user").find(row => row.field === field);
 return !!row && allowsNull(row.schema);
}
/** A captured edit of one record; Rust validates the changes when it applies them. */
export function editRequest(context: LocalContext, kind: Kind, changes: Record<string, unknown>, actor: ReferenceActor): ApplyRequest {
 const before = context[kind];if (!before) throw Error(`No attached ${kind}`);
 const inputs: ApplyRequest["inputs"] = actor === "automatic" && kind === "stream" && context.session ? [{kind:"session", record:context.session}] : [];
 return structuredClone({version:2, catalog_source:context.catalog_source, kind, actor, before, changes, title_default:context.title_defaults[kind], inputs});
}
export type ReferenceDraft = {key: ResourceKey; desired: unknown};
export function referenceSelector(key: ResourceKey): string {
 return JSON.stringify(key.kind === "url" ? ["url", key.url] : ["document", key.repository_url, key.path]);
}
export function referenceRequest(context: LocalContext, actor: ReferenceActor, drafts: ReferenceDraft[]): ApplyRequest {
 return editRequest(context, "stream", {reference_changes:drafts}, actor);
}
/** A proposal's key is its target's; Rust refuses a malformed one. */
const proposalKey = (value: any): ResourceKey => resourceKey(value?.target ?? {});
export function automaticReferenceRequest(context: LocalContext, rawProposals: unknown): ApplyRequest | undefined {
 if (rawProposals === undefined) return undefined;
 if (!Array.isArray(rawProposals)) throw Error("reference_proposals must be an array");
 if (!rawProposals.length) return undefined;
 return referenceRequest(context, "automatic", rawProposals.map(value => ({key:proposalKey(value), desired:value})));
}
export class ReferenceEdit {
 readonly base: LocalContext;
 error?: string;
 constructor(context: LocalContext, readonly key: ResourceKey | null, public actor: ReferenceActor, public desired: unknown) {
  this.base = structuredClone(context);this.key = structuredClone(key);this.desired = structuredClone(desired);
 }
 get dirty(): boolean {
  const current = this.key && this.base.stream?.references.find(row => referenceSelector(resourceKey(row.target)) === referenceSelector(this.key!));
  return current ? !isDeepStrictEqual(current, this.desired) : this.desired !== null;
 }
 rebase(context: LocalContext): void {Object.assign(this.base, structuredClone(context));this.error = undefined;}
 request(context: LocalContext = this.base): ApplyRequest {
  const key = this.key ?? proposalKey(this.desired);
  if (!this.key && context.stream?.references.some(row => referenceSelector(resourceKey(row.target)) === referenceSelector(key))) throw Error("Reference already exists; edit the saved row");
  return referenceRequest(context, this.actor, [{key, desired:this.desired}]);
 }
 /** Turn a retained automatic proposal into a user edit of the value Rust would save against `context`. */
 async adopt(cli: Pick<StreamCli, "preview">, context: LocalContext = this.base): Promise<void> {
  if (this.actor !== "automatic") return;
  const {receipt} = await cli.preview({catalog:context, action:"apply", request:this.request(context)});
  const desired = receipt.save?.domain.reference_results?.[0]?.desired;
  if (!desired) throw Error("Retained input is invalid; edit it before reapplying.");
  this.desired = structuredClone(desired);this.actor = "user";
 }
 async save(cli: {apply(request: ApplyRequest): Promise<Receipt>}, onReceipt?: (receipt: Receipt) => void): Promise<Receipt | undefined> {
  try {
   if (!this.dirty) return;
   const receipt = await cli.apply(this.request());onReceipt?.(receipt);
   if (receiptHasUnappliedReferences(receipt) || !receiptCanContinue(receipt)) throw Error("Reference edit not applied; input kept. Inspect the current row before reapplying.");
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
   if (!result.save?.domain) throw Error("Invalid catalog receipt");
   if (result.save.domain.outcome === "conflict" || result.save.domain.outcome === "superseded") throw Error(`Edit ${result.save.domain.outcome}. Your input is retained. Compare the live field, then discard and reopen to use its new base.`);
   if (!receiptCanContinue(result)) throw Error("Save did not complete; inspect its completion or publication outcome. Resubmitting the same captured request is safe; superseded or conflict means capture again.");
   this.error = undefined;return result;
  } catch (error) {this.error = String(error);throw error;}
 }
}
