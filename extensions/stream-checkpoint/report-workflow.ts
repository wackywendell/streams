import { automaticFields, automaticReferenceRequest, editRequest, referenceSelector } from "./core.ts";
import { receiptCanContinue, receiptHasUnappliedReferences, referenceText } from "./streams.ts";
import type { DiscardedCard, LocalContext, Receipt, ApplyRequest } from "./streams.ts";
import type { EnrollmentInference, InferenceResult, ReportInference } from "./agent.ts";

export type ResetTarget = "all" | "session" | "stream";
export type ReportAction =
 | {kind: "report"; force: boolean; instructions?: string}
 | {kind: "reset"; target: ResetTarget; auto: boolean; instructions?: string};
export type ReportSnapshot = {
 native: string; cwd: string; evidence: {messages: any[]; leafId: string | null}; catalog: LocalContext;
 valid(): boolean; attached(): Promise<boolean>;
};
export type ReportGenerator = <T extends EnrollmentInference | ReportInference>(request: T) => Promise<InferenceResult<T>>;
export type ReportEnvironment = {
 signal: AbortSignal; automaticEnrollment: boolean; operationId(): string;
 capture(): Promise<ReportSnapshot>; generate: ReportGenerator;
 /** Before a reset captures anything, so a card that cannot be decoded is rebuilt instead. */
 discardUnreadable(): Promise<DiscardedCard>;
 apply(request: ApplyRequest): Promise<Receipt>; enroll(request: unknown): Promise<Receipt>;
 receipt(receipt: Receipt): void; notice(text: string, level?: "info" | "warning"): void;
};

/** Shared ordering and card rules; the environment owns persistence and liveness. */
export async function runReport(action: ReportAction, environment: ReportEnvironment): Promise<boolean> {
 const reset = action.kind === "reset", auto = action.kind === "reset" && action.auto;
 const target = action.kind === "reset" ? action.target : "all";
 const force = action.kind === "reset" || action.force, instructions = action.instructions;
 const {signal, generate} = environment, captureNow = () => environment.capture();
 let savedAny = false, reenroll = false;
 if (reset) {
  const discarded = await environment.discardUnreadable();
  for (const reason of [discarded.session, discarded.stream]) if (reason !== null) {
   savedAny = true;environment.notice(`${reason}. Reset discarded it.`, "warning");
  }
  // An unreadable Session named no Stream we can keep; the conversation enrolls again.
  reenroll = discarded.session !== null;
  if (signal.aborted) return savedAny;
 }
 let capture = await captureNow();
 if (!capture.valid() || signal.aborted) return savedAny;
 const inferAndApply = async (kind: "session" | "stream"): Promise<boolean> => {
  const inferred = await generate({type: kind, catalog: capture.catalog, messages: capture.evidence.messages, instructions, reset});
  if (signal.aborted || !await capture.attached()) return false;
  if (kind === "session" && reset) for (const short of ["recap_short", "state_short", "remaining_short"] as const) {
   if (capture.catalog.session![short] != null && !Object.hasOwn(inferred.changes, short)) inferred.changes[short] = null;
  }
  if (kind === "stream" && reset && capture.catalog.stream!.goal_mode === "automatic" && capture.catalog.stream!.goal_short !== null && !Object.hasOwn(inferred.changes, "goal_short")) inferred.changes.goal_short = null;
  const recordReceipt = (receipt: Receipt, request?: ApplyRequest): boolean => {
   environment.receipt(receipt);
   const accepted = receiptCanContinue(receipt);
   if (accepted) savedAny = true;
   if (request && request.actor === "automatic" && request.changes.reference_changes) {
    const drafts = request.changes.reference_changes as {key: import("./streams.ts").ResourceKey; desired: {name:string} | null}[];
    const saves = receipt.saves?.map(item => item.save) ?? (receipt.save ? [receipt.save] : []);
    for (const save of saves) if (receipt.simulated || ["persisted", "committed"].includes(save.local)) {
     for (const row of save.domain?.reference_results ?? []) {
      if (row.outcome !== "changes" || row.action !== "add" && row.action !== "drop") continue;
      const draft = drafts.find(item => referenceSelector(item.key) === referenceSelector(row.key));
      if (draft?.desired) environment.notice(`Reference ${row.action === "add" ? "added" : "dropped"}: ${referenceText(draft.desired.name)}`, "info");
     }
    }
   }
   return accepted && !receiptHasUnappliedReferences(receipt);
  };
  if (Object.keys(inferred.changes).length) {
   if (signal.aborted || !await capture.attached()) return false;
   const receipt = await environment.apply(editRequest(capture.catalog, kind, inferred.changes, "automatic", environment.operationId()));
   if (!recordReceipt(receipt)) return false;
  }
  if (kind === "session") {
   let request: ApplyRequest | undefined, receipt: Receipt | undefined;
   try {
    if (inferred.reference_proposals !== undefined && (!Array.isArray(inferred.reference_proposals) || inferred.reference_proposals.length)) request = automaticReferenceRequest(capture.catalog, inferred.reference_proposals, environment.operationId());
    if (request) {
     if (signal.aborted || !await capture.attached()) return false;
     receipt = await environment.apply(request);
    }
   } catch (error) {
    environment.notice(`Reference update failed: ${referenceText(error instanceof Error ? error.message : String(error))}`, "warning");
    return false;
   }
   if (receipt && !recordReceipt(receipt, request)) return false;
  }
  return true;
 };
 if (!capture.catalog.session) {
  if (reset && !reenroll) {environment.notice("Reset requires an attached work card. Use /streams attach first.");return false;}
  if (!force && !environment.automaticEnrollment) return false;
  const operationId = environment.operationId();
  const result = await generate({type: "enroll", catalog: capture.catalog, messages: capture.evidence.messages, instructions});
  if (signal.aborted || !await capture.attached() || (!result.meaningful && !force)) return false;
  if (!result.session?.title?.trim() || (!capture.catalog.stream && !result.stream?.title?.trim())) throw Error("Enrollment requires meaningful effort and session titles");
  const receipt = await environment.enroll({version: 2, catalog_source: capture.catalog.catalog_source, operation_id: operationId, harness: "pi", harness_session_id: capture.native, ...(capture.catalog.stream ? {stream_id: capture.catalog.stream.id} : {stream: result.stream}), session: result.session, actor: "automatic", cwd: capture.cwd});
  environment.receipt(receipt);
  if (receiptCanContinue(receipt)) savedAny = true;
  if (!capture.valid() || !receiptCanContinue(receipt)) return savedAny;
  const enrollmentSource = capture.catalog.catalog_source;
  capture = await captureNow();
  if (capture.catalog.catalog_source !== enrollmentSource) throw Error("Catalog source changed after enrollment; no further report was attempted");
  if (!capture.catalog.session || !capture.valid() || signal.aborted) return savedAny;
 }
 const resetTargets: readonly ("session" | "stream")[] = !reset || target === "all" ? ["session", "stream"] : [target];
 if (reset && auto) {
  const source = capture.catalog.catalog_source, sessionId = capture.catalog.session.id, streamId = capture.catalog.stream?.id;
  for (const kind of resetTargets) {
   const record = capture.catalog[kind];
   if (!record) continue;
   const changes: Record<string, unknown> = {};
   if (record.title_mode === "manual" || record.title_mode === "inherit" && capture.catalog.title_defaults[kind] === "manual") changes.title_mode = "automatic";
   if (kind === "stream" && capture.catalog.stream!.goal_mode === "manual") changes.goal_mode = "automatic";
   if (!Object.keys(changes).length) continue;
   if (signal.aborted || !await capture.attached()) return savedAny;
   const receipt = await environment.apply(editRequest(capture.catalog, kind, changes, "user", environment.operationId()));
   environment.receipt(receipt);
   if (receiptCanContinue(receipt)) savedAny = true;
   if (!capture.valid() || !receiptCanContinue(receipt)) return savedAny;
   capture = await captureNow();
   if (!capture.valid() || capture.catalog.catalog_source !== source || capture.catalog.session?.id !== sessionId || capture.catalog.stream?.id !== streamId) throw Error("Attachment changed during naming reset");
  }
 }
 if (resetTargets.includes("session")) {
  // A Session write is its own inference and guarded target; it never timestamps the effort.
  if (!await inferAndApply("session")) return savedAny;
  if (!resetTargets.includes("stream")) return savedAny;
  // Capture AGAIN before metadata inference, never after it or to make an old result fit.
  if (!capture.valid()) return savedAny;
  const sessionBase = capture.catalog;
  capture = await captureNow();
  if (capture.catalog.catalog_source !== sessionBase.catalog_source || capture.catalog.stream?.id !== sessionBase.stream?.id || capture.catalog.session?.id !== sessionBase.session?.id) throw Error("Catalog source or attachment changed; metadata report cancelled");
  if (!capture.valid() || signal.aborted) return savedAny;
 }
 if (!capture.catalog.stream || !automaticFields("stream", capture.catalog.stream, capture.catalog.title_defaults.stream).length) return savedAny;
 if (!await inferAndApply("stream")) return savedAny;
 return savedAny;
}
