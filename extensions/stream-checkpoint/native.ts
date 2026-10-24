import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import type { StreamsChild } from "./child.ts";
import { StreamsCliError } from "./streams.ts";

export type NativeSnapshot = {
 scope: {catalogSource: string; hostId: string; streamId: string; sessionId: string; nativeSessionId: string};
 native: {pane_id: string; tab_id: string; workspace_id: string};
 session: {title: string; title_mode: "automatic" | "manual" | "inherit"};
};
export type NativeContext = {socket: string; paneId: string; terminalId: string; workspaceId: string; tabId: string};
export type NativeAdapter = {
 automatic: boolean;
 sessionNaming: boolean;
 subscribe?(listener: () => void): () => void;
 dispose?(): Promise<void>;
 read(signal?: AbortSignal): Promise<NativeSnapshot>;
 bootstrap(signal?: AbortSignal): Promise<NativeContext>;
 open(context: NativeContext, signal: AbortSignal, mode: "automatic" | "explicit"): Promise<unknown>;
 close(context: NativeContext, signal: AbortSignal): Promise<unknown>;
 capture?(snapshot: NativeSnapshot, signal: AbortSignal): Promise<unknown>;
 project(context: NativeContext, signal: AbortSignal, intent: 'automatic' | 'explicit' | 'explicit-tab' | 'explicit-workspace' | undefined): Promise<unknown>;
 sessionName(snapshot: NativeSnapshot, assertCurrent: () => void, intent?: 'automatic' | 'explicit'): void;
};
export type NativeFactory = (input: {pi: ExtensionAPI; ctx: ExtensionContext; sessionId: string; cwd: string; env: NodeJS.ProcessEnv; child?: StreamsChild; warning?: (text: string) => void}) => Promise<NativeAdapter>;

// Native dependencies are deliberately resolved only inside a gated session lifetime.
export const loadNative: NativeFactory = async input => {
 const {loadRustNative} = await import("./native-rust.ts");
 return loadRustNative(input);
};
const environmentKeys = ["HERDR_ENV", "HERDR_PANE_ID", "HERDR_TAB_ID", "HERDR_WORKSPACE_ID", "HERDR_SOCKET_PATH", "HERDR_BIN_PATH", "STREAMS_STREAM_ID", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "PI_CODING_AGENT_DIR", "HOME", "STREAMS_RUST_BIN", "STREAMS_BIN_PATH", "CARGO_TARGET_DIR", "WORK_CARDS_CONFIG_PATH", "PATH", "EDITOR", "HERDR_SESSION", "CODEX_HOME", "CLAUDE_CONFIG_DIR", "PI_OFFLINE"];
type Action = {kind: "open" | "close" | "reclaim"; target?: "all" | "session" | "stream"; resolve: () => void; reject: (error: unknown) => void};

/** One pinned source and one serial effect lane per native Pi lifetime. */
export class NativeController {
 private readonly env = {...process.env};
 private readonly sessionId: string;
 private readonly cwd: string;
 private adapter?: Promise<NativeAdapter>;
 private context?: NativeContext;
 private controller = new AbortController();
 private stopped = false;
 private pending = false;
 private running?: Promise<void>;
 private actions: Action[] = [];
 private unsubscribe?: () => void;
 private retirement?: Promise<void>;
 private deferred: Action[] = [];
 private scopeRevision = 0;
 private automaticOpen = true;
 private readonly warnings = new Set<string>();
 constructor(private readonly options: {pi: ExtensionAPI; ctx: ExtensionContext; current?: () => ExtensionContext | undefined; factory?: NativeFactory; child?: StreamsChild; capture?: (streamId: string, signal?: AbortSignal) => Promise<unknown>; warning: (text: string) => void}) {
  this.sessionId = options.ctx.sessionManager.getSessionId();this.cwd = options.ctx.cwd;
 }
 private eligible(): boolean {
  return this.options.ctx.hasUI && this.options.ctx.mode === "tui" && this.env.HERDR_ENV === "1" && !!this.env.HERDR_PANE_ID
   && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(this.sessionId);
 }
 start(): void {if (this.eligible()) this.update();}
 update(): void {
  if (this.stopped || !this.eligible()) return;
  this.pending = true;this.drain();
 }
 open(): Promise<void> {return this.action("open");}
 close(): Promise<void> {return this.action("close");}
 reclaimNames(target: "all" | "session" | "stream" = "all"): Promise<void> {return this.eligible() ? this.action("reclaim", target) : Promise.resolve();}
 private action(kind: Action["kind"], target?: Action["target"]): Promise<void> {
  if (this.stopped || !this.eligible()) return Promise.reject(Error("Native work cards require an interactive Pi session in its exact Herdr pane."));
  if (kind === "close") this.automaticOpen = false;
  return new Promise((resolve, reject) => {this.actions.push({kind, target, resolve, reject});this.drain();});
 }
 invalidate(): void {
  this.controller.abort();this.controller = new AbortController();
  for (const action of [...this.actions.splice(0), ...this.deferred.splice(0)]) action.reject(Error("Native work-card tree changed; retry the pane command"));
  this.update();
 }
 stop(): void {
  this.stopped = true;this.pending = false;this.controller.abort();this.unsubscribe?.();
  for (const action of [...this.actions.splice(0), ...this.deferred.splice(0)]) action.reject(Error("Native work-card lifetime ended"));
  this.retirement ??= this.adapter?.then(adapter => adapter.dispose?.()).catch(() => {});
 }
 async idle(): Promise<void> {while (this.running) await this.running;await this.retirement;}
 private assertCurrent(signal: AbortSignal): void {
  signal.throwIfAborted();
  const current = this.options.current ? this.options.current() : this.options.ctx;
  if (this.stopped || !current?.hasUI || current.mode !== "tui" || current.cwd !== this.cwd || current.sessionManager.getSessionId() !== this.sessionId
   || environmentKeys.some(key => process.env[key] !== this.env[key])) throw Error("Native work-card session or environment changed; reload required");
 }
 private drain(): void {
  if (this.running || this.deferred.length) return;
  this.running = this.run().finally(() => {this.running = undefined;if (!this.stopped && !this.deferred.length && (this.pending || this.actions.length)) this.drain();});
 }
 private async run(): Promise<void> {
  while (!this.stopped && !this.deferred.length && (this.pending || this.actions.length)) {
   const action = this.actions.shift(), signal = this.controller.signal, revision = this.scopeRevision;
   if (!action) this.pending = false;
   try {
    const snapshot = await this.refresh(action, signal);
    if (snapshot) await this.capture(snapshot, signal);
    action?.resolve();
   } catch (error) {
    if (error instanceof StreamsCliError && error.kind === "native-context-changed" && !signal.aborted && !this.stopped) {
     if (action) {
      if (revision === this.scopeRevision) this.deferred.push(action);else this.actions.unshift(action);
     }
     continue;
    }
    action?.reject(error);
    if (signal.aborted || this.stopped) continue;
    // Missing/unready enrollment and inherited helper environments are normal at startup.
    // Surface unexpected failures once, but keep explicit commands actionable on every attempt.
    const message = String(error);
    if ((!action || action.kind === "reclaim") && !/unbound|context read failed|attestation|exact host\/pi\/native|exact Stream and Session/i.test(message) && !this.warnings.has(message)) {
     this.warnings.add(message);this.options.warning(`Native pane: ${message}`);
    }
   }
  }
 }
 // Capture is best effort and recovers on the next actual update, not by replaying native effects.
 private async capture(snapshot: NativeSnapshot, signal: AbortSignal): Promise<void> {
  this.assertCurrent(signal);
  try {
   const adapter = await this.adapter!;
   if (adapter.capture) await adapter.capture(snapshot, signal);
   else await this.options.capture?.(snapshot.scope.streamId, signal);
  } catch (error) {
   this.assertCurrent(signal);
   const message = `Arrangement capture deferred; last saved layout retained: ${String(error)}`;
   if (!this.warnings.has(message)) {this.warnings.add(message);this.options.warning(message);}
  }
  this.assertCurrent(signal);
 }
 private async refresh(action: Action | undefined, signal: AbortSignal): Promise<NativeSnapshot | undefined> {
  const kind = action?.kind;
  const check = () => this.assertCurrent(signal);check();
  this.adapter ??= (this.options.factory ?? loadNative)({pi: this.options.pi, ctx: this.options.ctx, sessionId: this.sessionId, cwd: this.cwd, env: {...this.env}, child:this.options.child, warning:this.options.warning}).then(adapter => {
   if (!this.stopped) this.unsubscribe = adapter.subscribe?.(() => {
    this.scopeRevision++;this.actions.unshift(...this.deferred.splice(0));this.update();
   });
   return adapter;
  });
  const adapter = await this.adapter;check();
  const read = async () => {
   const snapshot = await adapter.read(signal);check();
   if (snapshot.scope.nativeSessionId !== this.sessionId) throw Error("Native attestation does not match the Pi session");
   return snapshot;
  };
  await read();
  const context = await adapter.bootstrap(signal);check();
  this.context = context;
  if (kind === "close") {await adapter.close(this.context, signal);check();return;}
  const automatic = !kind && this.automaticOpen && adapter.automatic;
  if (!kind || kind === "open") this.automaticOpen = false;
  if (kind === "open" || automatic) {await adapter.open(this.context, signal, kind ? "explicit" : "automatic");check();}
  const nameIntent = kind === "reclaim" ? (action?.target === "session" ? "explicit-tab" : action?.target === "stream" ? "explicit-workspace" : "explicit") : "automatic";
  await adapter.project(this.context, signal, nameIntent);check();
  const snapshot = await read();
  const sessionIntent = kind === "reclaim" && action?.target !== "stream" ? "explicit" : "automatic";
  if (adapter.sessionNaming) adapter.sessionName(snapshot, check, sessionIntent);
  check();
  // Capture only the attested existing association, never receipt/model text or membership.
  return snapshot;
 }
}
