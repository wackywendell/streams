import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import type { StreamCli } from "./streams.ts";
import { hasConversation } from "./session-scope.ts";

export type NativeAdapter = {
  start(signal: AbortSignal): Promise<void>;
  open(signal: AbortSignal): Promise<unknown>;
  close(signal: AbortSignal): Promise<unknown>;
  dispose(): Promise<void>;
};

export type NativeFactory = (input: {
  pi: ExtensionAPI;
  ctx: ExtensionContext;
  sessionId: string;
  cwd: string;
  env: NodeJS.ProcessEnv;
  cli: StreamCli;
  piNamingSession: () => boolean;
  assertCurrent: () => void;
  warning: (text: string) => void;
}) => Promise<NativeAdapter>;

export const loadNative: NativeFactory = async (input) => {
  const { loadRustNative } = await import("./native-rust.ts");
  return loadRustNative(input);
};

const environmentKeys = [
  "HC_TERMINAL_ID",
  "HC_SOCKET",
  "HC_SESSION",
  "XDG_RUNTIME_DIR",
  "HERDR_ENV",
  "HERDR_PANE_ID",
  "HERDR_TAB_ID",
  "HERDR_WORKSPACE_ID",
  "HERDR_SOCKET_PATH",
  "HERDR_BIN_PATH",
  "XDG_CONFIG_HOME",
  "XDG_DATA_HOME",
  "XDG_STATE_HOME",
  "PI_CODING_AGENT_DIR",
  "HOME",
  "STREAMS_BIN_PATH",
  "CARGO_TARGET_DIR",
  "PATH",
  "EDITOR",
  "HERDR_SESSION",
  "CODEX_HOME",
  "CLAUDE_CONFIG_DIR",
  "PI_OFFLINE",
];

/** Pi fences its own UI lifetime; `hc label` and `card watch` are driven through public commands. */
export class NativeController {
  private readonly env = { ...process.env };
  private readonly sessionId: string;
  private readonly cwd: string;
  private readonly controller = new AbortController();
  private adapter?: Promise<NativeAdapter>;
  private startup?: Promise<void>;
  private retirement?: Promise<void>;

  constructor(
    private readonly options: {
      pi: ExtensionAPI;
      ctx: ExtensionContext;
      cli: StreamCli;
      piNamingSession?: () => boolean;
      current?: () => ExtensionContext | undefined;
      factory?: NativeFactory;
      warning: (text: string) => void;
    }
  ) {
    this.sessionId = options.ctx.sessionManager.getSessionId();
    this.cwd = options.ctx.cwd;
  }

  private eligible(): boolean {
    return (
      this.options.ctx.hasUI &&
      this.options.ctx.mode === "tui" &&
      ((this.env.HERDR_ENV === "1" && !!this.env.HERDR_PANE_ID) || !!this.env.HC_TERMINAL_ID) &&
      /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(this.sessionId)
    );
  }

  private assertCurrent = (): void => {
    this.controller.signal.throwIfAborted();
    const current = this.options.current ? this.options.current() : this.options.ctx;
    if (
      !current?.hasUI ||
      current.mode !== "tui" ||
      current.cwd !== this.cwd ||
      current.sessionManager.getSessionId() !== this.sessionId ||
      environmentKeys.some((key) => process.env[key] !== this.env[key])
    ) {
      throw new Error("Native work-card session or environment changed; reload required");
    }
  };

  start(): Promise<void> | undefined {
    if (!this.eligible()) return;
    const ctx = this.options.current?.() ?? this.options.ctx;
    if (
      !this.startup &&
      this.env.HERDR_ENV === "1" &&
      !this.env.HC_TERMINAL_ID &&
      !hasConversation(ctx.sessionManager)
    ) {
      return;
    }
    return (this.startup ??= (async () => {
      this.assertCurrent();
      this.adapter ??= (this.options.factory ?? loadNative)({
        pi: this.options.pi,
        ctx,
        sessionId: this.sessionId,
        cwd: this.cwd,
        env: { ...this.env },
        cli: this.options.cli,
        piNamingSession: this.options.piNamingSession ?? (() => true),
        assertCurrent: this.assertCurrent,
        warning: this.options.warning,
      });
      const adapter = await this.adapter;
      this.assertCurrent();
      await adapter.start(this.controller.signal);
      this.assertCurrent();
    })());
  }

  prepareCatalog(): Promise<void> | undefined {
    const starting = this.start();
    return this.env.HC_TERMINAL_ID && this.env.HERDR_ENV !== "1"
      ? starting?.catch(() => {})
      : starting;
  }

  async ready(): Promise<void> {
    if (!this.eligible()) {
      throw new Error("Native work cards require session startup in an interactive Pi terminal.");
    }
    const starting = this.start();
    if (!starting) throw new Error("Start a conversation before using a native work card.");
    await starting;
    this.assertCurrent();
  }

  private async action(kind: "open" | "close"): Promise<unknown> {
    await this.ready();
    const adapter = await this.adapter!;
    this.assertCurrent();
    const result = await adapter[kind](this.controller.signal);
    this.assertCurrent();
    return result;
  }

  open(): Promise<unknown> {
    return this.action("open");
  }

  close(): Promise<unknown> {
    return this.action("close");
  }

  stop(): void {
    this.controller.abort();
    this.retirement ??= this.adapter?.then((adapter) => adapter.dispose()).catch(() => {});
  }

  async idle(): Promise<void> {
    await this.startup?.catch(() => {});
    await this.retirement;
  }
}
