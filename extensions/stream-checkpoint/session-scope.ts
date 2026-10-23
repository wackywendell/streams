import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { checkpointContext } from "./agent.ts";
import type { ReportingSettings } from "./settings.ts";
import type { StreamCli } from "./streams.ts";

export type SessionFence = {signal: AbortSignal; current(): boolean; live(): boolean};
/** Tree changes cancel inference; only start/shutdown retire transport outcomes. */
export class SessionScope {
 context?: ExtensionContext;
 stopped = false;
 private inference = new AbortController();
 private lifetime = {};
 onInvalidate?: () => void;
 use(ctx: ExtensionContext): void {if (!this.stopped) this.context = ctx;}
 reset(ctx: ExtensionContext, stopped = false): void {
  this.lifetime = {};this.context = ctx;this.stopped = stopped;this.invalidate();
 }
 invalidate(): void {this.inference.abort();this.inference = new AbortController();this.onInvalidate?.();}
 fence(): SessionFence {
  const lifetime = this.lifetime, signal = this.inference.signal;
  const native = this.context?.sessionManager.getSessionId(), cwd = this.context?.cwd;
  const live = () => lifetime === this.lifetime;
  return {signal, live, current: () => live() && !this.stopped && !signal.aborted && this.context?.sessionManager.getSessionId() === native && this.context?.cwd === cwd};
 }
}

/** Immutable inference inputs, with an attachment reread that never replaces the base. */
export async function captureInputs(ctx: ExtensionContext, cli: StreamCli, mode: ReportingSettings["context"], fence: SessionFence, purpose: "report" | "discussion" = "discussion") {
 const native = ctx.sessionManager.getSessionId(), cwd = ctx.cwd, source = process.env.STREAMS_STREAM_ID;
 const evidence = checkpointContext(ctx.sessionManager, mode);
 const catalog = structuredClone(await cli.context(native));
 const valid = () => fence.current() && ctx.sessionManager.getSessionId() === native && ctx.cwd === cwd && process.env.STREAMS_STREAM_ID === source && (evidence.leafId === null ? ctx.sessionManager.getLeafId() === null : ctx.sessionManager.getBranch().some((entry) => entry.id === evidence.leafId));
 const attached = async () => {
  if (!valid()) return false;
  const current = await cli.context(native);
  // Keep the captured base; the CLI admits local and shared results across membership changes.
  return valid() && current.catalog_source === catalog.catalog_source && current.session?.id === catalog.session?.id && (purpose === "report" || current.stream?.id === catalog.stream?.id);
 };
 return {native, cwd, evidence, catalog, valid, attached};
}
