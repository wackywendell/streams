import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { checkpointContext } from "./agent.ts";
import type { ReportingSettings } from "./settings.ts";
import { sessionKeyOf, streamKeyOf, type StreamCli } from "./streams.ts";

export function hasConversation(manager: ExtensionContext["sessionManager"]): boolean {
  return manager
    .buildSessionProjection()
    .messages.some((message) =>
      ["user", "assistant", "compactionSummary", "branchSummary"].includes(message.role)
    );
}

export type SessionFence = { signal: AbortSignal; current(): boolean; live(): boolean };

/** Tree changes cancel inference; only start/shutdown retire transport outcomes. */
export class SessionScope {
  context?: ExtensionContext;
  stopped = false;
  private inference = new AbortController();
  private lifetime = {};
  onInvalidate?: () => void;

  use(ctx: ExtensionContext): void {
    if (!this.stopped) this.context = ctx;
  }

  reset(ctx: ExtensionContext, stopped = false): void {
    this.lifetime = {};
    this.context = ctx;
    this.stopped = stopped;
    this.invalidate();
  }

  stop(): void {
    this.lifetime = {};
    this.stopped = true;
    this.invalidate();
  }

  invalidate(): void {
    this.inference.abort();
    this.inference = new AbortController();
    this.onInvalidate?.();
  }

  fence(): SessionFence {
    const lifetime = this.lifetime;
    const signal = this.inference.signal;
    const native = this.context?.sessionManager.getSessionId();
    const cwd = this.context?.cwd;
    const live = () => lifetime === this.lifetime;
    return {
      signal,
      live,
      current: () =>
        live() &&
        !this.stopped &&
        !signal.aborted &&
        this.context?.sessionManager.getSessionId() === native &&
        this.context?.cwd === cwd,
    };
  }
}

/** Immutable inference inputs, with an attachment reread that never replaces the base. */
export async function captureInputs(
  ctx: ExtensionContext,
  cli: StreamCli,
  mode: ReportingSettings["context"],
  fence: SessionFence,
  purpose: "report" | "discussion" = "discussion"
) {
  const native = ctx.sessionManager.getSessionId();
  const cwd = ctx.cwd;
  const evidence = checkpointContext(ctx.sessionManager, mode);
  const catalog = structuredClone(await cli.context(native, fence.signal));
  const valid = () =>
    fence.current() &&
    ctx.sessionManager.getSessionId() === native &&
    ctx.cwd === cwd &&
    (evidence.leafId === null
      ? ctx.sessionManager.getLeafId() === null
      : ctx.sessionManager.getBranch().some((entry) => entry.id === evidence.leafId));
  const attached = async (requireSameStream = purpose !== "report") => {
    if (!valid()) return false;
    const current = await cli.context(native, fence.signal);
    const sameSession =
      (current.session ? sessionKeyOf(current.session) : null) ===
      (catalog.session ? sessionKeyOf(catalog.session) : null);
    const sameStream =
      (current.stream ? streamKeyOf(current.stream) : null) ===
      (catalog.stream ? streamKeyOf(catalog.stream) : null);
    return valid() && sameSession && (!requireSameStream || sameStream);
  };
  return { native, cwd, evidence, catalog, valid, attached };
}
