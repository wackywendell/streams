import { randomUUID } from "node:crypto";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { infer as modelInfer } from "./agent.ts";
import type { Inference, Inferred } from "./agent.ts";
import { automaticFields, editRequest } from "./core.ts";
import { receiptCanContinue } from "./streams.ts";
import type { Receipt, StreamCli } from "./streams.ts";
import { SessionScope, captureInputs } from "./session-scope.ts";
import type { SessionFence } from "./session-scope.ts";
import type { ReportingSettings } from "./settings.ts";

type Options = {scope?: SessionScope; cli(ctx: ExtensionContext): StreamCli; settings(): ReportingSettings; infer?: typeof modelInfer; notice(text: string): void; receipt(value: Receipt): void; activity?(text?: string): void; onReclaimNames?(): Promise<void> | void};
type Pending = {ctx: ExtensionContext; force: boolean; instructions?: string; reset?: boolean; auto?: boolean};
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
 reset(ctx: ExtensionContext, auto = false): void {this.request(ctx, true, undefined, true, auto);}
 request(ctx: ExtensionContext, force = true, instructions?: string, reset = false, auto = false): void {
  if (this.scope.stopped) return;
  this.scope.use(ctx);
  this.pending = {ctx, force: force || !!this.pending?.force, instructions: instructions?.trim() || this.pending?.instructions, reset: Boolean(reset || this.pending?.reset), auto: Boolean(auto || this.pending?.auto)};
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
   this.options.activity?.("Reporting");
   try {await this.report(pending, fence);} catch (error) {if (fence.current()) this.options.notice(String(error));}
   finally {if (fence.live()) this.options.activity?.();}
  }
 }
 private async generate(ctx: ExtensionContext, request: Inference, signal: AbortSignal): Promise<Inferred> {
  return (this.options.infer ?? modelInfer)(ctx, request, signal, this.options.settings().model);
 }
 private async report({ctx, force, instructions, reset, auto}: Pending, fence: SessionFence): Promise<void> {
  const cli = this.options.cli(ctx), signal = fence.signal;
  const captureNow = () => captureInputs(ctx, cli, this.options.settings().context, fence);
  let capture = await captureNow();
  if (!capture.valid() || signal.aborted) return;
  if (!capture.catalog.session) {
   if (reset) {this.options.notice("Reset requires an attached work card. Use /streams attach first.");return;}
   if (!force && this.options.settings().enrollment !== "automatic") return;
   const operationId = randomUUID();
   const result = await this.generate(ctx, {type: "enroll", catalog: capture.catalog, messages: capture.evidence.messages, instructions}, signal);
   if (signal.aborted || !await capture.attached() || (!result.meaningful && !force)) return;
   if (!result.session?.title?.trim() || (!capture.catalog.stream && !result.stream?.title?.trim())) throw Error("Enrollment requires meaningful effort and session titles");
   const receipt = await cli.enroll({catalog_source: capture.catalog.catalog_source, operation_id: operationId, harness: "pi", harness_session_id: capture.native, ...(capture.catalog.stream ? {stream_id: capture.catalog.stream.id} : {stream: result.stream}), session: result.session, actor: "automatic", cwd: capture.cwd});
   if (fence.live()) this.options.receipt(receipt);
   if (!capture.valid() || !receiptCanContinue(receipt)) return;
   const enrollmentSource = capture.catalog.catalog_source;
   capture = await captureNow();
   if (capture.catalog.catalog_source !== enrollmentSource) throw Error("Catalog source changed after enrollment; no further report was attempted");
   if (!capture.catalog.session || !capture.valid() || signal.aborted) return;
  }
  if (reset && auto) {
   const source = capture.catalog.catalog_source, sessionId = capture.catalog.session.id, streamId = capture.catalog.stream?.id;
   for (const kind of ["session", "stream"] as const) {
    const target = capture.catalog[kind];
    if (!target || kind === "stream" && !capture.catalog.authority) continue;
    const changes: Record<string, unknown> = {};
    if (target.title_mode === "manual") changes.title_mode = "automatic";
    if (kind === "stream" && capture.catalog.stream!.goal_mode === "manual") changes.goal_mode = "automatic";
    if (!Object.keys(changes).length) continue;
    if (signal.aborted || !await capture.attached()) return;
    const receipt = await cli.apply(editRequest(capture.catalog, kind, changes, "user", randomUUID(), true));
    if (fence.live()) this.options.receipt(receipt);
    if (!capture.valid() || !receiptCanContinue(receipt)) return;
    capture = await captureNow();
    if (!capture.valid() || capture.catalog.catalog_source !== source || capture.catalog.session?.id !== sessionId || capture.catalog.stream?.id !== streamId) throw Error("Attachment changed during naming reset");
   }
  }
  // A Session write is its own inference and guarded target; it never timestamps the effort.
  const sessionOperation = randomUUID();
  const progress = await this.generate(ctx, {type: "session", catalog: capture.catalog, messages: capture.evidence.messages, instructions, reset}, signal);
  if (signal.aborted || !await capture.attached()) return;
  if (reset && capture.catalog.session.state_short !== null && progress.changes && !Object.hasOwn(progress.changes, "state_short")) progress.changes.state_short = null;
  if (progress.changes && Object.keys(progress.changes).length) {
   const receipt = await cli.apply(editRequest(capture.catalog, "session", progress.changes, "automatic", sessionOperation, true));
   if (fence.live()) this.options.receipt(receipt);
   if (!receiptCanContinue(receipt)) return;
  }
  // Capture AGAIN before metadata inference, never after it or to make an old result fit.
  if (!capture.valid()) return;
  const sessionBase = capture.catalog;
  capture = await captureNow();
  if (capture.catalog.catalog_source !== sessionBase.catalog_source || capture.catalog.stream?.id !== sessionBase.stream?.id || capture.catalog.session?.id !== sessionBase.session?.id) throw Error("Catalog source or attachment changed; metadata report cancelled");
  if (!capture.valid() || signal.aborted) return;
  if (!capture.catalog.stream || !automaticFields("stream", capture.catalog.stream).length) {if (auto) await this.options.onReclaimNames?.();return;}
  const streamOperation = randomUUID();
  const metadata = await this.generate(ctx, {type: "stream", catalog: capture.catalog, messages: capture.evidence.messages, instructions, reset}, signal);
  if (signal.aborted || !await capture.attached()) return;
  if (reset && capture.catalog.stream.goal_mode === "automatic" && capture.catalog.stream.goal_short !== null && metadata.changes && !Object.hasOwn(metadata.changes, "goal_short")) metadata.changes.goal_short = null;
  if (metadata.changes && Object.keys(metadata.changes).length) {
   const receipt = await cli.apply(editRequest(capture.catalog, "stream", metadata.changes, "automatic", streamOperation, true));
   if (fence.live()) this.options.receipt(receipt);
   if (!receiptCanContinue(receipt)) return;
  }
  if (auto && capture.valid()) await this.options.onReclaimNames?.();
 }
}
