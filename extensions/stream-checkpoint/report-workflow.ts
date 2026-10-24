import { automaticFields, editRequest } from "./core.ts";
import { receiptCanContinue } from "./streams.ts";
import type { LocalContext, Receipt, ApplyRequest } from "./streams.ts";
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
 apply(request: ApplyRequest): Promise<Receipt>; enroll(request: unknown): Promise<Receipt>;
 receipt(receipt: Receipt): void; notice(text: string): void;
 completed?(outcome: {savedAny: boolean; reclaimReady: boolean; target: ResetTarget}): Promise<void> | void;
};

/** Shared ordering and card rules; the environment owns persistence and liveness. */
export async function runReport(action: ReportAction, environment: ReportEnvironment): Promise<boolean> {
 const reset = action.kind === "reset", auto = action.kind === "reset" && action.auto;
 const target = action.kind === "reset" ? action.target : "all";
 const force = action.kind === "reset" || action.force, instructions = action.instructions;
 const {signal, generate} = environment, captureNow = () => environment.capture();
 let capture = await captureNow(), savedAny = false, reclaimReady = false;
 if (!capture.valid() || signal.aborted) return false;
 const inferAndApply = async (kind: "session" | "stream"): Promise<boolean> => {
  const inferred = await generate({type: kind, catalog: capture.catalog, messages: capture.evidence.messages, instructions, reset});
  if (signal.aborted || !await capture.attached()) return false;
  if (kind === "session" && reset && capture.catalog.session!.state_short !== null && !Object.hasOwn(inferred.changes, "state_short")) inferred.changes.state_short = null;
  if (kind === "stream" && reset && capture.catalog.stream!.goal_mode === "automatic" && capture.catalog.stream!.goal_short !== null && !Object.hasOwn(inferred.changes, "goal_short")) inferred.changes.goal_short = null;
  if (!Object.keys(inferred.changes).length) return true;
  const receipt = await environment.apply(editRequest(capture.catalog, kind, inferred.changes, "automatic", environment.operationId(), true));
  environment.receipt(receipt);
  if (receiptCanContinue(receipt)) {savedAny = true;return true;}
  return false;
 };
 try {
  if (!capture.catalog.session) {
   if (reset) {environment.notice("Reset requires an attached work card. Use /streams attach first.");return false;}
   if (!force && !environment.automaticEnrollment) return false;
   const operationId = environment.operationId();
   const result = await generate({type: "enroll", catalog: capture.catalog, messages: capture.evidence.messages, instructions});
   if (signal.aborted || !await capture.attached() || (!result.meaningful && !force)) return false;
   if (!result.session?.title?.trim() || (!capture.catalog.stream && !result.stream?.title?.trim())) throw Error("Enrollment requires meaningful effort and session titles");
   const receipt = await environment.enroll({version: 2, catalog_source: capture.catalog.catalog_source, operation_id: operationId, harness: "pi", harness_session_id: capture.native, ...(capture.catalog.stream ? {stream_id: capture.catalog.stream.id} : {stream: result.stream}), session: result.session, actor: "automatic", cwd: capture.cwd});
   environment.receipt(receipt);
   if (receiptCanContinue(receipt)) savedAny = true;
   if (!capture.valid() || !receiptCanContinue(receipt)) return false;
   const enrollmentSource = capture.catalog.catalog_source;
   capture = await captureNow();
   if (capture.catalog.catalog_source !== enrollmentSource) throw Error("Catalog source changed after enrollment; no further report was attempted");
   if (!capture.catalog.session || !capture.valid() || signal.aborted) return false;
  }
  const resetTargets: readonly ("session" | "stream")[] = !reset || target === "all" ? ["session", "stream"] : [target];
  if (reset && auto) {
   const source = capture.catalog.catalog_source, sessionId = capture.catalog.session.id, streamId = capture.catalog.stream?.id;
   for (const kind of resetTargets) {
    const record = capture.catalog[kind];
    if (!record || kind === "stream" && !capture.catalog.authority) continue;
    const changes: Record<string, unknown> = {};
    if (record.title_mode === "manual") changes.title_mode = "automatic";
    if (kind === "stream" && capture.catalog.stream!.goal_mode === "manual") changes.goal_mode = "automatic";
    if (!Object.keys(changes).length) continue;
    if (signal.aborted || !await capture.attached()) return false;
    const receipt = await environment.apply(editRequest(capture.catalog, kind, changes, "user", environment.operationId(), false));
    environment.receipt(receipt);
    if (receiptCanContinue(receipt)) savedAny = true;
    if (!capture.valid() || !receiptCanContinue(receipt)) return false;
    capture = await captureNow();
    if (!capture.valid() || capture.catalog.catalog_source !== source || capture.catalog.session?.id !== sessionId || capture.catalog.stream?.id !== streamId) throw Error("Attachment changed during naming reset");
   }
  }
  if (resetTargets.includes("session")) {
   // A Session write is its own inference and guarded target; it never timestamps the effort.
   if (!await inferAndApply("session")) return false;
   if (!resetTargets.includes("stream")) {reclaimReady = Boolean(auto && capture.valid());return savedAny;}
   // Capture AGAIN before metadata inference, never after it or to make an old result fit.
   if (!capture.valid()) return false;
   const sessionBase = capture.catalog;
   capture = await captureNow();
   if (capture.catalog.catalog_source !== sessionBase.catalog_source || capture.catalog.stream?.id !== sessionBase.stream?.id || capture.catalog.session?.id !== sessionBase.session?.id) throw Error("Catalog source or attachment changed; metadata report cancelled");
   if (!capture.valid() || signal.aborted) return false;
  }
  if (!capture.catalog.stream || !automaticFields("stream", capture.catalog.stream).length) {reclaimReady = Boolean(auto && capture.valid());return savedAny;}
  if (!await inferAndApply("stream")) return false;
  reclaimReady = Boolean(auto && capture.valid());
  return savedAny;
 } finally {
  await environment.completed?.({savedAny, reclaimReady, target});
 }
}
