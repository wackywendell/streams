import { automaticFields, editRequest } from "./core.ts";
import { receiptCanContinue } from "./streams.ts";
import type { LocalContext, Receipt, ApplyRequest } from "./streams.ts";
import type { EnrollmentInference, ReportInference, InferenceResult } from "./agent.ts";

export type ResetTarget = "all" | "session" | "stream";
export type ReportAction = {kind: "report"; force: boolean; instructions?: string} | {kind: "reset"; target: ResetTarget; auto: boolean; instructions?: string};
export type ReportSnapshot = {native: string; cwd: string; evidence: {messages: any[]; leafId: string | null}; catalog: LocalContext; valid(): boolean; attached(): Promise<boolean>};
export type ReportPort = {
 signal: AbortSignal; automaticEnrollment: boolean; live(): boolean; operationId(): string;
 capture(): Promise<ReportSnapshot>;
 generate<T extends EnrollmentInference | ReportInference>(request: T): Promise<InferenceResult<T>>;
 apply(request: ApplyRequest): Promise<Receipt>; enroll(request: unknown): Promise<Receipt>;
 receipt(value: Receipt): void; notice(text: string): void;
 reclaimNames?(target: ResetTarget): Promise<void> | void; updateNative?(): void;
};
/** Production ordering and guards; adapters alone decide whether saves are real or simulated. */
export async function runReport(action: ReportAction, port: ReportPort): Promise<boolean> {
 const reset = action.kind === "reset", force = action.kind === "report" ? action.force : true;
 const auto = action.kind === "reset" && action.auto, target = action.kind === "reset" ? action.target : "all";
 const instructions = action.instructions, signal = port.signal, captureNow = () => port.capture();
  let capture = await captureNow(), savedAny = false, reclaimReady = false;
  if (!capture.valid() || signal.aborted) return false;
  const inferAndApply = async (kind: "session" | "stream"): Promise<boolean> => {
   for (let attempt = 0; ; attempt++) {
    const inferred = await port.generate({type: kind, catalog: capture.catalog, messages: capture.evidence.messages, instructions, reset});
    if (signal.aborted || !await capture.attached()) return false;
    if (kind === "session" && reset && capture.catalog.session!.state_short !== null && !Object.hasOwn(inferred.changes, "state_short")) inferred.changes.state_short = null;
    if (kind === "stream" && reset && capture.catalog.stream!.goal_mode === "automatic" && capture.catalog.stream!.goal_short !== null && !Object.hasOwn(inferred.changes, "goal_short")) inferred.changes.goal_short = null;
    if (!Object.keys(inferred.changes).length) return true;
    const receipt = await port.apply(editRequest(capture.catalog, kind, inferred.changes, "automatic", port.operationId(), true));
    if (attempt < 2 && receipt.save?.domain?.outcome === "superseded" && receipt.save.local === "unchanged" && !receipt.completion_error && receipt.publication?.local !== "repair-required") {
     const fresh = await captureNow();
     if (fresh.valid() && fresh.catalog.catalog_source === capture.catalog.catalog_source && fresh.catalog.stream?.id === capture.catalog.stream?.id && fresh.catalog.session?.id === capture.catalog.session?.id && JSON.stringify(fresh.catalog) !== JSON.stringify(capture.catalog)) {
      capture = fresh;
      if (kind === "stream" && (!capture.catalog.stream || !automaticFields("stream", capture.catalog.stream).length)) return true;
      continue;
     }
    }
    if (port.live()) port.receipt(receipt);
    if (receiptCanContinue(receipt)) {savedAny = true;return true;}
    return false;
   }
  };
  try {
   if (!capture.catalog.session) {
    if (reset) {port.notice("Reset requires an attached work card. Use /streams attach first.");return false;}
    if (!force && !port.automaticEnrollment) return false;
    const operationId = port.operationId();
    const result = await port.generate({type: "enroll", catalog: capture.catalog, messages: capture.evidence.messages, instructions});
    if (signal.aborted || !await capture.attached() || (!result.meaningful && !force)) return false;
    if (!result.session?.title?.trim() || (!capture.catalog.stream && !result.stream?.title?.trim())) throw Error("Enrollment requires meaningful effort and session titles");
    const receipt = await port.enroll({catalog_source: capture.catalog.catalog_source, operation_id: operationId, harness: "pi", harness_session_id: capture.native, ...(capture.catalog.stream ? {stream_id: capture.catalog.stream.id} : {stream: result.stream}), session: result.session, actor: "automatic", cwd: capture.cwd});
    if (port.live()) port.receipt(receipt);
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
     const receipt = await port.apply(editRequest(capture.catalog, kind, changes, "user", port.operationId(), false));
     if (port.live()) port.receipt(receipt);
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
   if (reclaimReady && port.live()) await port.reclaimNames?.(target);
   else if (savedAny && port.live()) port.updateNative?.();
  }
}
