import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { resolveStreamsBin } from "./streams-bin.ts";

const defaultPluginRoot = fileURLToPath(
  new URL("../../integrations/herdr-work-cards", import.meta.url)
);

/**
 * Long-lived classic Herdr `streams card watch pi/<ID>` process.
 * Keeps stdin open while Pi runs; closing stdin signals clean shutdown.
 */
export class CardWatchChild {
  private process?: ReturnType<typeof spawn>;
  private stopped = false;
  private closing?: Promise<void>;
  private closed?: Promise<void>;

  constructor(
    private readonly options: {
      sessionId: string;
      cwd: string;
      env?: NodeJS.ProcessEnv;
      pluginRoot?: string;
      warning?: (message: string) => void;
      spawn?: typeof spawn;
      stopTimeoutMs?: number;
    }
  ) {}

  start(): void {
    if (this.stopped || this.process) return;
    const env = this.options.env ?? process.env;
    const binary = resolveStreamsBin(env);
    const pluginRoot = this.options.pluginRoot ?? defaultPluginRoot;
    const selector = `pi/${this.options.sessionId}`;
    const child = (this.options.spawn ?? spawn)(
      binary,
      ["card", "watch", selector, "--plugin-root", pluginRoot],
      {
        cwd: this.options.cwd,
        env: { ...env, STREAMS_BIN_PATH: binary },
        stdio: ["pipe", "ignore", "pipe"],
      }
    );
    this.process = child;
    let resolveClose!: () => void;
    this.closed = new Promise((resolve) => {
      resolveClose = resolve;
    });
    let stderr = "";
    child.stderr?.setEncoding("utf8").on("data", (chunk: string) => {
      stderr += chunk;
    });
    child.stdin?.on("error", () => {});
    child.on("error", (error) => {
      if (!this.stopped) {
        this.options.warning?.(`card watch failed: ${error.message}`);
      }
    });
    child.on("close", (code, signal) => {
      resolveClose();
      if (this.process === child) {
        this.process = undefined;
        this.closed = undefined;
      }
      if (!this.stopped && (signal || (code !== null && code !== 0))) {
        const detail = stderr.trim();
        this.options.warning?.(
          `card watch exited (${signal ?? code})${detail ? `: ${detail}` : ""}`
        );
      }
    });
  }

  get running(): boolean {
    return this.process !== undefined;
  }

  stop(): Promise<void> {
    if (this.closing) return this.closing;
    this.stopped = true;
    const child = this.process;
    if (!child) return Promise.resolve();
    const closed = this.closed ?? Promise.resolve();
    const timeoutMs = this.options.stopTimeoutMs ?? 2000;
    const timer = setTimeout(() => {
      if (this.process === child && !child.killed) {
        try {
          child.kill("SIGTERM");
        } catch {
          // Already exited
        }
      }
    }, timeoutMs);
    timer.unref?.();
    this.closing = closed.finally(() => clearTimeout(timer));
    try {
      child.stdin?.end();
    } catch {
      // Already closed
    }
    return this.closing;
  }
}

export { defaultPluginRoot as cardPluginRoot };
