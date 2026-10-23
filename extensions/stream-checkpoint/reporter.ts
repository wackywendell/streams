import { randomUUID } from "node:crypto";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { infer as modelInfer } from "./agent.ts";
import type { InferenceLogOptions } from "./agent.ts";
import { runReport } from "./report-workflow.ts";
import type { ResetTarget } from "./report-workflow.ts";
export type { ResetTarget } from "./report-workflow.ts";
import type { Receipt, StreamCli } from "./streams.ts";
import { SessionScope, captureInputs } from "./session-scope.ts";
import type { SessionFence } from "./session-scope.ts";
import type { ReportingSettings } from "./settings.ts";

type Options = {scope?: SessionScope; cli(ctx: ExtensionContext): StreamCli; settings(): ReportingSettings; infer?: typeof modelInfer; logging?(): InferenceLogOptions | undefined; notice(text: string): void; receipt(value: Receipt, deferNative?: boolean, quiet?: boolean): void; activity?(text?: string, completed?: boolean): void; onUpdateNative?(): void; onReclaimNames?(target: ResetTarget): Promise<void> | void};
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
   const label = pending.reset ? "Regenerating…" : "Updating…";
   this.options.activity?.(label);
   try {completed = await this.report(pending, fence);} catch (error) {if (fence.current()) this.options.notice(error instanceof Error ? error.message : String(error));}
   finally {
    if (fence.live()) {
     this.options.activity?.(undefined, completed && !this.pending);
    }
   }
  }
 }
 private async report({ctx, force, instructions, reset, auto, target = "all"}: Pending, fence: SessionFence): Promise<boolean> {
  const cli = this.options.cli(ctx), generate = this.options.infer ?? modelInfer;
  return runReport(reset ? {kind: "reset", target, auto: Boolean(auto), instructions} : {kind: "report", force, instructions}, {
   signal: fence.signal, operationId: randomUUID, automaticEnrollment: this.options.settings().enrollment === "automatic",
   capture: () => captureInputs(ctx, cli, this.options.settings().context, fence),
   generate: request => generate(ctx, request, fence.signal, this.options.settings().model, this.options.logging?.()),
   apply: (request) => cli.apply(request), enroll: (request) => cli.enroll(request),
   receipt: (receipt) => {if (fence.live()) this.options.receipt(receipt, true, !force);},
   notice: (text) => this.options.notice(text),
   completed: async ({savedAny, reclaimReady, target}) => {
    if (reclaimReady && fence.live()) await this.options.onReclaimNames?.(target);
    else if (savedAny && fence.live()) this.options.onUpdateNative?.();
   },
  });
 }}
