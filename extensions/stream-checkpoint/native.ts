import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";

export type NativeSnapshot = {
 scope: {catalogSource: string; hostId: string; streamId: string; sessionId: string; nativeSessionId: string};
 native: {pane_id: string; tab_id: string; workspace_id: string};
 session: {title: string; title_mode: "automatic" | "manual"};
};
export type NativeContext = {socket: string; paneId: string; terminalId: string; workspaceId: string; tabId: string};
export type NativeAdapter = {
 automatic: boolean;
 sessionNaming: boolean;
 read(signal?: AbortSignal): Promise<NativeSnapshot>;
 bootstrap(signal?: AbortSignal): Promise<NativeContext>;
 open(context: NativeContext, signal: AbortSignal, mode: "automatic" | "explicit"): Promise<unknown>;
 close(context: NativeContext, signal: AbortSignal): Promise<unknown>;
 capture?(snapshot: NativeSnapshot, signal: AbortSignal): Promise<unknown>;
 project(context: NativeContext, signal: AbortSignal, intent: 'automatic' | 'explicit' | 'explicit-tab' | 'explicit-workspace' | undefined): Promise<unknown>;
 sessionName(snapshot: NativeSnapshot, assertCurrent: () => void, intent?: 'automatic' | 'explicit'): void;
};
export type NativeFactory = (input: {pi: ExtensionAPI; ctx: ExtensionContext; sessionId: string; cwd: string; env: NodeJS.ProcessEnv}) => Promise<NativeAdapter>;

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
 private timer?: ReturnType<typeof setTimeout>;
 private retries = 0;
 private readonly warnings = new Set<string>();
 constructor(private readonly options: {pi: ExtensionAPI; ctx: ExtensionContext; current?: () => ExtensionContext | undefined; factory?: NativeFactory; capture?: (streamId: string, signal?: AbortSignal) => Promise<unknown>; warning: (text: string) => void}) {
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
  return new Promise((resolve, reject) => {this.actions.push({kind, target, resolve, reject});this.drain();});
 }
 invalidate(): void {
  this.controller.abort();this.controller = new AbortController();
  for (const action of this.actions.splice(0)) action.reject(Error("Native work-card tree changed; retry the pane command"));
  this.update();
 }
 stop(): void {
  this.stopped = true;this.pending = false;clearTimeout(this.timer);this.controller.abort();
  for (const action of this.actions.splice(0)) action.reject(Error("Native work-card lifetime ended"));
 }
 async idle(): Promise<void> {while (this.running) await this.running;}
 private assertCurrent(signal: AbortSignal): void {
  signal.throwIfAborted();
  const current = this.options.current ? this.options.current() : this.options.ctx;
  if (this.stopped || !current?.hasUI || current.mode !== "tui" || current.cwd !== this.cwd || current.sessionManager.getSessionId() !== this.sessionId
   || environmentKeys.some(key => process.env[key] !== this.env[key])) throw Error("Native work-card session or environment changed; reload required");
 }
 private drain(): void {
  if (this.running) return;
  this.running = this.run().finally(() => {this.running = undefined;if (!this.stopped && (this.pending || this.actions.length)) this.drain();});
 }
 private async run(): Promise<void> {
  while (!this.stopped && (this.pending || this.actions.length)) {
   const action = this.actions.shift(), signal = this.controller.signal;
   if (!action) this.pending = false;
   try {
    const snapshot = await this.refresh(action, signal);
    this.retries = 0;clearTimeout(this.timer);this.timer = undefined;
    if (snapshot) await this.capture(snapshot, signal);
    action?.resolve();
   } catch (error) {
    action?.reject(error);
    if (signal.aborted || this.stopped) continue;
    // Missing/unready enrollment and inherited helper environments are normal at startup.
    // Surface unexpected failures once, but keep explicit commands actionable on every attempt.
    const message = String(error);
    if ((!action || action.kind === "reclaim") && !/unbound|context read failed|attestation|exact host\/pi\/native|exact Stream and Session/i.test(message) && !this.warnings.has(message)) {
     this.warnings.add(message);this.options.warning(`Native pane: ${message}`);
    }
    if (!action && this.retries++ < 4 && !this.timer) {
     this.timer = setTimeout(() => {this.timer = undefined;this.update();}, 15000);this.timer.unref();
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
  this.adapter ??= (this.options.factory ?? loadNative)({pi: this.options.pi, ctx: this.options.ctx, sessionId: this.sessionId, cwd: this.cwd, env: {...this.env}});
  const adapter = await this.adapter;check();
  const read = async () => {
   const snapshot = await adapter.read(signal);check();
   if (snapshot.scope.nativeSessionId !== this.sessionId || snapshot.native.pane_id !== this.env.HERDR_PANE_ID
    || this.env.HERDR_TAB_ID && snapshot.native.tab_id !== this.env.HERDR_TAB_ID
    || this.env.HERDR_WORKSPACE_ID && snapshot.native.workspace_id !== this.env.HERDR_WORKSPACE_ID) throw Error("Native attestation does not match captured Pi pane tuple");
   if (this.context && (snapshot.native.pane_id !== this.context.paneId || snapshot.native.tab_id !== this.context.tabId || snapshot.native.workspace_id !== this.context.workspaceId)) throw Error("Native attestation tuple changed; reload required");
   return snapshot;
  };
  await read();
  if (!this.context) {
   const context = await adapter.bootstrap(signal);check();
   if (context.paneId !== this.env.HERDR_PANE_ID || this.env.HERDR_SOCKET_PATH && context.socket !== this.env.HERDR_SOCKET_PATH) throw Error("Native frontend endpoint changed");
   this.context = context;
   await read();check();
  }
  if (kind === "close") {await adapter.close(this.context, signal);check();return;}
  if (kind === "open" || !kind && adapter.automatic) {await adapter.open(this.context, signal, kind ? "explicit" : "automatic");check();}
  const nameIntent = kind === "reclaim" ? (action?.target === "session" ? "explicit-tab" : action?.target === "stream" ? "explicit-workspace" : "explicit") : "automatic";
  await adapter.project(this.context, signal, nameIntent);check();
  const snapshot = await read();
  const sessionIntent = kind === "reclaim" && action?.target !== "stream" ? "explicit" : "automatic";
  if (adapter.sessionNaming || sessionIntent === "explicit") adapter.sessionName(snapshot, check, sessionIntent);
  check();
  // Capture only the attested existing association, never receipt/model text or membership.
  return snapshot;
 }
}
