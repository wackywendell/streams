import { randomUUID } from "node:crypto";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { infer as modelInfer } from "./agent.ts";
import { automaticFields, editRequest } from "./core.ts";
import { receiptCanContinue } from "./streams.ts";
import type { Receipt, StreamCli } from "./streams.ts";
import { SessionScope, captureInputs } from "./session-scope.ts";
import type { SessionFence } from "./session-scope.ts";
import type { ReportingSettings } from "./settings.ts";

export type ResetTarget = "all" | "session" | "stream";
type Options = {scope?: SessionScope; cli(ctx: ExtensionContext): StreamCli; settings(): ReportingSettings; infer?: typeof modelInfer; notice(text: string): void; receipt(value: Receipt, deferNative?: boolean, quiet?: boolean): void; activity?(text?: string, completed?: boolean): void; onUpdateNative?(): void; onReclaimNames?(target: ResetTarget): Promise<void> | void};
type Pending = {ctx: ExtensionContext; force: boolean; instructions?: string; reset?: boolean; auto?: boolean; target?: ResetTarget};
export class Reporter {
 readonly scope: SessionScope;
 private count = 0;
 private pending?: Pending;
 private running?: Promise<void>;
 private suspended = false;
 constructor(private options: Options) {
  this.scope = options.scope ?? new SessionScope();
  this.scope.onInvalidate = () => {this.count = 0;this.pending = undefined;};
 }
 response(ctx: ExtensionContext): void {
  if (this.scope.stopped || this.options.settings().reporting !== "automatic") return;
  this.scope.use(ctx);
  if (++this.count % this.options.settings().frequency === 0) this.request(ctx, false);
 }
 reset(ctx: ExtensionContext, auto = false, target: ResetTarget = "all"): void {this.request(ctx, true, undefined, true, auto, target);}
 request(ctx: ExtensionContext, force = true, instructions?: string, reset = false, auto = false, target?: ResetTarget): void {
  if (this.scope.stopped) return;
  this.scope.use(ctx);
  const nextTarget = reset ? (this.pending?.reset && this.pending.target && this.pending.target !== (target ?? "all") ? "all" : (target ?? "all")) : this.pending?.target;
  this.pending = {ctx, force: force || !!this.pending?.force, instructions: instructions?.trim() || this.pending?.instructions, reset: Boolean(reset || this.pending?.reset), auto: Boolean(auto || this.pending?.auto), target: nextTarget};
  this.start();
 }
 invalidate(): void {this.scope.invalidate();}
 async idle(): Promise<void> {await this.running;}
 private start(): void {
  if (!this.running && !this.suspended && this.pending) this.running = this.drain().finally(() => {this.running = undefined;this.start();});
 }
 /** Card discussion shares the reporting helper slot, without losing newer report evidence. */
 async exclusive<T>(run: (signal: AbortSignal) => Promise<T>): Promise<T> {
  if (this.suspended) throw Error("A work card helper is already running");
  this.suspended = true;
  const fence = this.scope.fence();
  try {
   await this.idle();
   if (!fence.current()) throw Error("Session changed before card action");
   return await run(fence.signal);
  } finally {this.suspended = false;this.start();}
 }
 private async drain(): Promise<void> {
  while (this.pending && !this.suspended) {
   const pending = this.pending;this.pending = undefined;
   const fence = this.scope.fence();
   let completed = false;
   const label = pending.reset ? (pending.auto ? "⟳ Unlocking names & regenerating..." : "⟳ Regenerating fields...") : pending.force ? "⟳ Updating work card..." : "⟳ Updating...";
   this.options.activity?.(label);
   try {completed = await this.report(pending, fence);} catch (error) {if (fence.current()) this.options.notice(String(error));}
   finally {
    if (fence.live()) {
     this.options.activity?.(undefined, completed && !this.pending);
    }
   }
  }
 }
 private async report({ctx, force, instructions, reset, auto, target = "all"}: Pending, fence: SessionFence): Promise<boolean> {
  const cli = this.options.cli(ctx), signal = fence.signal, quiet = !force;
  const generate = this.options.infer ?? modelInfer;
  const captureNow = () => captureInputs(ctx, cli, this.options.settings().context, fence);
  let capture = await captureNow(), savedAny = false, reclaimReady = false;
  if (!capture.valid() || signal.aborted) return false;
  const inferAndApply = async (kind: "session" | "stream"): Promise<boolean> => {
   for (let attempt = 0; ; attempt++) {
    const inferred = await generate(ctx, {type: kind, catalog: capture.catalog, messages: capture.evidence.messages, instructions, reset}, signal, this.options.settings().model);
    if (signal.aborted || !await capture.attached()) return false;
    if (kind === "session" && reset && capture.catalog.session!.state_short !== null && !Object.hasOwn(inferred.changes, "state_short")) inferred.changes.state_short = null;
    if (kind === "stream" && reset && capture.catalog.stream!.goal_mode === "automatic" && capture.catalog.stream!.goal_short !== null && !Object.hasOwn(inferred.changes, "goal_short")) inferred.changes.goal_short = null;
    if (!Object.keys(inferred.changes).length) return true;
    const receipt = await cli.apply(editRequest(capture.catalog, kind, inferred.changes, "automatic", randomUUID(), true));
    if (attempt < 2 && receipt.save?.domain?.outcome === "superseded" && receipt.save.local === "unchanged" && !receipt.completion_error && receipt.publication?.local !== "repair-required") {
     const fresh = await captureNow();
     if (fresh.valid() && fresh.catalog.catalog_source === capture.catalog.catalog_source && fresh.catalog.stream?.id === capture.catalog.stream?.id && fresh.catalog.session?.id === capture.catalog.session?.id && JSON.stringify(fresh.catalog) !== JSON.stringify(capture.catalog)) {
      capture = fresh;
      if (kind === "stream" && (!capture.catalog.stream || !automaticFields("stream", capture.catalog.stream).length)) return true;
      continue;
     }
    }
    if (fence.live()) this.options.receipt(receipt, true, quiet);
    if (receiptCanContinue(receipt)) {savedAny = true;return true;}
    return false;
   }
  };
  try {
   if (!capture.catalog.session) {
    if (reset) {this.options.notice("Reset requires an attached work card. Use /streams attach first.");return false;}
    if (!force && this.options.settings().enrollment !== "automatic") return false;
    const operationId = randomUUID();
    const result = await generate(ctx, {type: "enroll", catalog: capture.catalog, messages: capture.evidence.messages, instructions}, signal, this.options.settings().model);
    if (signal.aborted || !await capture.attached() || (!result.meaningful && !force)) return false;
    if (!result.session?.title?.trim() || (!capture.catalog.stream && !result.stream?.title?.trim())) throw Error("Enrollment requires meaningful effort and session titles");
    const receipt = await cli.enroll({catalog_source: capture.catalog.catalog_source, operation_id: operationId, harness: "pi", harness_session_id: capture.native, ...(capture.catalog.stream ? {stream_id: capture.catalog.stream.id} : {stream: result.stream}), session: result.session, actor: "automatic", cwd: capture.cwd});
    if (fence.live()) this.options.receipt(receipt, true, quiet);
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
     const receipt = await cli.apply(editRequest(capture.catalog, kind, changes, "user", randomUUID(), false));
     if (fence.live()) this.options.receipt(receipt, true, quiet);
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
   if (reclaimReady && fence.live()) await this.options.onReclaimNames?.(target);
   else if (savedAny && fence.live()) this.options.onUpdateNative?.();
  }
 }
}
