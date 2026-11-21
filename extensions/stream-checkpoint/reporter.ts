import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { infer as modelInfer } from "./agent.ts";
import type { InferenceLogOptions } from "./agent.ts";
import { runReport } from "./report-workflow.ts";
import type { ResetTarget } from "./report-workflow.ts";
export type { ResetTarget } from "./report-workflow.ts";
import type { EditBody, EditResult, StreamCli } from "./streams.ts";
import { SessionScope, captureInputs, hasConversation } from "./session-scope.ts";
import type { SessionFence } from "./session-scope.ts";
import type { ReportingSettings } from "./settings.ts";

type Options = {
  scope?: SessionScope;
  prepare?(): Promise<void> | undefined;
  cli(ctx: ExtensionContext): StreamCli;
  settings(): ReportingSettings;
  infer?: typeof modelInfer;
  logging?(): InferenceLogOptions | undefined;
  notice(text: string, level?: "info" | "warning"): void;
  onEnrolled?(ctx: ExtensionContext, info: { streamKey: string; sessionKey: string; sessionTitle: string }): Promise<void> | void;
  onEditResult?(ctx: ExtensionContext, kind: "session" | "stream", body: EditBody, result: EditResult, quiet?: boolean): void;
  activity?(text?: string, completed?: boolean): void;
};

type Pending = {
  ctx: ExtensionContext;
  force: boolean;
  instructions?: string;
  reset?: boolean;
  auto?: boolean;
  target?: ResetTarget;
};

export class Reporter {
  readonly scope: SessionScope;
  private count = 0;
  private pending?: Pending;
  private running?: Promise<void>;
  private suspended = false;

  constructor(private options: Options) {
    this.scope = options.scope ?? new SessionScope();
    this.scope.onInvalidate = () => {
      this.count = 0;
      this.pending = undefined;
    };
  }

  response(ctx: ExtensionContext): void {
    if (this.scope.stopped || this.options.settings().reporting !== "automatic") return;
    this.scope.use(ctx);
    if (++this.count % this.options.settings().frequency === 0) this.request(ctx, false);
  }

  reset(ctx: ExtensionContext, auto = false, target: ResetTarget = "all"): void {
    this.request(ctx, true, undefined, true, auto, target);
  }

  request(
    ctx: ExtensionContext,
    force = true,
    instructions?: string,
    reset = false,
    auto = false,
    target?: ResetTarget
  ): void {
    if (this.scope.stopped) return;
    this.scope.use(ctx);
    const nextTarget = reset
      ? this.pending?.reset && this.pending.target && this.pending.target !== (target ?? "all")
        ? "all"
        : (target ?? "all")
      : this.pending?.target;
    this.pending = {
      ctx,
      force: force || !!this.pending?.force,
      instructions: instructions?.trim() || this.pending?.instructions,
      reset: Boolean(reset || this.pending?.reset),
      auto: Boolean(auto || this.pending?.auto),
      target: nextTarget,
    };
    this.start();
  }

  async reportNow(ctx: ExtensionContext): Promise<void> {
    this.scope.use(ctx);
    const fence = this.scope.fence();
    await this.exclusive(async () => {
      await this.report({ ctx, force: true }, fence);
    });
  }

  invalidate(): void {
    this.scope.invalidate();
  }

  async idle(): Promise<void> {
    await this.running;
  }

  private start(): void {
    if (!this.running && !this.suspended && this.pending) {
      this.running = this.drain().finally(() => {
        this.running = undefined;
        this.start();
      });
    }
  }

  /** Card discussion shares the reporting helper slot, without losing newer report evidence. */
  async exclusive<T>(run: (signal: AbortSignal) => Promise<T>): Promise<T> {
    if (this.suspended) throw new Error("A work card helper is already running");
    this.suspended = true;
    const fence = this.scope.fence();
    try {
      await this.idle();
      if (!fence.current()) throw new Error("Session changed before card action");
      return await run(fence.signal);
    } finally {
      this.suspended = false;
      this.start();
    }
  }

  private async drain(): Promise<void> {
    while (this.pending && !this.suspended) {
      const pending = this.pending;
      this.pending = undefined;
      const fence = this.scope.fence();
      let completed = false;
      const label = pending.reset ? "Regenerating…" : "Updating…";
      this.options.activity?.(label);
      try {
        completed = await this.report(pending, fence);
      } catch (error) {
        if (fence.current()) {
          this.options.notice(error instanceof Error ? error.message : String(error));
        }
      } finally {
        if (fence.live()) {
          this.options.activity?.(undefined, completed && !this.pending);
        }
      }
    }
  }

  private async report(
    { ctx, force, instructions, reset, auto, target = "all" }: Pending,
    fence: SessionFence
  ): Promise<boolean> {
    if (!fence.current()) return false;
    if (!hasConversation(ctx.sessionManager)) {
      if (force) this.options.notice("Start a conversation before reporting a work card.");
      return false;
    }
    const cli = this.options.cli(ctx);
    const generate = this.options.infer ?? modelInfer;
    return runReport(
      reset
        ? { kind: "reset", target, auto: Boolean(auto), instructions }
        : { kind: "report", force, instructions },
      {
        signal: fence.signal,
        automaticEnrollment: this.options.settings().enrollment === "automatic",
        capture: async () => {
          await this.options.prepare?.();
          return captureInputs(ctx, cli, this.options.settings().context, fence, "report");
        },
        generate: (request) =>
          generate(
            ctx,
            request,
            fence.signal,
            this.options.settings().model,
            this.options.logging?.()
          ),
        createStream: (input) => cli.createStream(input, fence.signal),
        createSession: (id, input) => cli.createSession(id, input, fence.signal),
        editSession: (targetKey, body) => cli.editSession(targetKey, body, fence.signal),
        editStream: (targetKey, body) => cli.editStream(targetKey, body, fence.signal),
        onEnrolled: async (info) => {
          if (fence.live()) await this.options.onEnrolled?.(ctx, info);
        },
        onEditResult: (kind, body, result) => {
          if (fence.live()) this.options.onEditResult?.(ctx, kind, body, result, !force);
        },
        notice: (text, level) => {
          if (fence.live()) this.options.notice(text, level);
        },
      }
    );
  }
}
