import { randomUUID } from "node:crypto";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { checkpointContext, infer as modelInfer } from "./agent.ts";
import type { Inference, Inferred } from "./agent.ts";
import { automaticFields, editRequest } from "./core.ts";
import type { LocalContext, Receipt, StreamCli } from "./streams.ts";
import type { ReportingSettings } from "./settings.ts";

type Options = {cli(ctx: ExtensionContext): StreamCli; settings(): ReportingSettings; infer?: typeof modelInfer; notice(text: string): void; receipt(value: Receipt): void; activity?(text?: string): void};
type Pending = {ctx: ExtensionContext; force: boolean; instructions?: string};
export class Reporter {
 private epoch = 0;
 private count = 0;
 private pending?: Pending;
 private running?: Promise<void>;
 private controller?: AbortController;
 private suspended = false;
 constructor(private options: Options) {}
 response(ctx: ExtensionContext): void {
  if (this.options.settings().reporting !== "automatic") return;
  if (++this.count % this.options.settings().frequency === 0) this.request(ctx, false);
 }
 request(ctx: ExtensionContext, force = true, instructions?: string): void {
  this.pending = {ctx, force: force || !!this.pending?.force, instructions: instructions?.trim() || this.pending?.instructions};
  this.start();
 }
 invalidate(): void {this.epoch++;this.count = 0;this.pending = undefined;this.controller?.abort();}
 async idle(): Promise<void> {await this.running;}
 private start(): void {
  if (!this.running && !this.suspended && this.pending) this.running = this.drain().finally(() => {this.running = undefined;this.start();});
 }
 /** Card discussion shares the reporting helper slot, without losing newer report evidence. */
 async exclusive<T>(run: (signal: AbortSignal) => Promise<T>): Promise<T> {
  if (this.suspended) throw Error("A work card helper is already running");
  this.suspended = true;
  const epoch = this.epoch;
  try {
   await this.idle();
   if (epoch !== this.epoch) throw Error("Session changed before card action");
   this.controller = new AbortController();return await run(this.controller.signal);
  } finally {this.suspended = false;this.start();}
 }
 private async drain(): Promise<void> {
  while (this.pending && !this.suspended) {
   const pending = this.pending;this.pending = undefined;
   this.controller = new AbortController();
   this.options.activity?.("Reporting");
   try {await this.report(pending, this.controller.signal);} catch (error) {if (!this.controller.signal.aborted) this.options.notice(String(error));}
   finally {this.options.activity?.();}
  }
 }
 private async capture(ctx: ExtensionContext, cli: StreamCli) {
  const epoch = this.epoch, native = ctx.sessionManager.getSessionId(), cwd = ctx.cwd, source = process.env.STREAMS_STREAM_ID;
  const evidence = checkpointContext(ctx.sessionManager, this.options.settings().context);
  const catalog = structuredClone(await cli.context(native, source));
  const valid = () => epoch === this.epoch && ctx.sessionManager.getSessionId() === native && ctx.cwd === cwd && process.env.STREAMS_STREAM_ID === source && (evidence.leafId === null ? ctx.sessionManager.getLeafId() === null : ctx.sessionManager.getBranch().some((entry) => entry.id === evidence.leafId));
  const attached = async () => {
   if (!valid()) return false;
   const current = await cli.context(native, source);
   return valid() && current.catalog_source === catalog.catalog_source && current.stream?.id === catalog.stream?.id && current.session?.id === catalog.session?.id;
  };
  return {native, cwd, evidence, catalog, valid, attached};
 }
 private async generate(ctx: ExtensionContext, request: Inference, signal: AbortSignal): Promise<Inferred> {
  return (this.options.infer ?? modelInfer)(ctx, request, signal, this.options.settings().model);
 }
 private async report({ctx, force, instructions}: Pending, signal: AbortSignal): Promise<void> {
  const cli = this.options.cli(ctx);
  let capture = await this.capture(ctx, cli);
  if (!capture.valid() || signal.aborted) return;
  if (!capture.catalog.session) {
   if (!force && this.options.settings().enrollment !== "automatic") return;
   const operationId = randomUUID();
   const result = await this.generate(ctx, {type: "enroll", catalog: capture.catalog, messages: capture.evidence.messages, instructions}, signal);
   if (signal.aborted || !await capture.attached() || (!result.meaningful && !force)) return;
   if (!result.session?.title?.trim() || (!capture.catalog.stream && !result.stream?.title?.trim())) throw Error("Enrollment requires meaningful effort and session titles");
   const receipt = await cli.enroll({catalog_source: capture.catalog.catalog_source, operation_id: operationId, harness: "pi", harness_session_id: capture.native, ...(capture.catalog.stream ? {stream_id: capture.catalog.stream.id} : {stream: result.stream}), session: result.session, actor: "automatic", cwd: capture.cwd});
   this.options.receipt(receipt);
   if (receipt.completion_error || receipt.saves?.some(({save}) => !save.domain || ["superseded", "conflict"].includes(save.domain.outcome))) return;
   const enrollmentSource = capture.catalog.catalog_source;
   capture = await this.capture(ctx, cli);
   if (capture.catalog.catalog_source !== enrollmentSource) throw Error("Catalog source changed after enrollment; no further report was attempted");
   if (!capture.catalog.session || !capture.valid() || signal.aborted) return;
  }
  // A Session write is its own inference and guarded target; it never timestamps the effort.
  const sessionOperation = randomUUID();
  const progress = await this.generate(ctx, {type: "session", catalog: capture.catalog, messages: capture.evidence.messages, instructions}, signal);
  if (signal.aborted || !await capture.attached()) return;
  if (progress.changes && Object.keys(progress.changes).length) {
   const receipt = await cli.apply(editRequest(capture.catalog, "session", progress.changes, "automatic", sessionOperation, true));
   this.options.receipt(receipt);
   if (receipt.completion_error || !receipt.save?.domain || ["superseded", "conflict"].includes(receipt.save.domain.outcome)) return;
  }
  // Capture AGAIN before metadata inference, never after it or to make an old result fit.
  const sessionBase = capture.catalog;
  capture = await this.capture(ctx, cli);
  if (capture.catalog.catalog_source !== sessionBase.catalog_source || capture.catalog.stream?.id !== sessionBase.stream?.id || capture.catalog.session?.id !== sessionBase.session?.id) throw Error("Catalog source or attachment changed; metadata report cancelled");
  if (!capture.catalog.stream || !capture.valid() || signal.aborted || !automaticFields("stream", capture.catalog.stream).length) return;
  const streamOperation = randomUUID();
  const metadata = await this.generate(ctx, {type: "stream", catalog: capture.catalog, messages: capture.evidence.messages, instructions}, signal);
  if (signal.aborted || !await capture.attached()) return;
  if (metadata.changes && Object.keys(metadata.changes).length) this.options.receipt(await cli.apply(editRequest(capture.catalog, "stream", metadata.changes, "automatic", streamOperation, true)));
 }
}
