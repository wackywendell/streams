import { spawn } from "node:child_process";
import { resolveStreamsBin } from "./streams-bin.ts";
import {
  isSession,
  isStream,
  isUnreadableRecord,
  sessionKeyOf,
  streamKeyOf,
  type EditBody,
  type EditResult,
  type LocalContext,
  type Mode,
  type NamingSettings,
  type SessionWire,
  type Status,
  type StreamWire,
  type UnreadableRecord,
} from "./wire.ts";

export { isReferenceText, sessionKeyOf, streamKeyOf } from "./wire.ts";
export type {
  ArtifactEntry,
  DocumentFileState,
  EditBody,
  EditResult,
  Goal,
  LocalContext,
  Mode,
  NamingSettings,
  ReferenceEntry,
  ReferenceKind,
  ReferenceRole,
  ReferenceStatus,
  RepositoryInfo,
  SessionDirectory,
  SessionWire,
  Status,
  StreamWire,
  TextPair,
  Title,
  UnreadableRecord,
} from "./wire.ts";

export type CardMode = "automatic" | "manual";

export type ConfigSnapshot = {
  naming: NamingSettings;
  herdr: {
    viewer: { mode: "editor" | "less" | "hunk" | "native"; placement: "tab" | "pane-right" | "pane-down" | "floating" };
    naming: { tab: boolean; workspace: boolean };
    cards: { auto_open: CardMode };
    timeout_ms: number;
    pane_height_fraction: number;
  };
  pi: {
    naming: { session: boolean; import_edits: boolean };
    reporting: {
      mode: Mode;
      enrollment: Mode;
      frequency: number;
      context: "trimmed" | "full";
      model: { provider: string; id: string } | null;
      session_logging: boolean;
      debug_commands: boolean;
    };
  };
};

export type SyncResult = {
  local: "unchanged" | "committed" | "repair-required";
  sharing?: "local-only" | "confirmed" | "blocked" | "failed" | "uncertain";
  detail?: string;
  needs_attention?: boolean;
  [key: string]: unknown;
};

export type ArtifactRequest =
  | { action: "write"; name: string; description: string; text?: string; file?: string; description_only?: boolean }
  | { action: "read"; reference: string; offset?: number; limit?: number }
  | { action: "delete"; reference: string }
  | { action: "list_session"; session?: string }
  | { action: "list_stream"; stream: string };

export type StreamListing = {
  items: StreamWire[];
  unreadable: UnreadableRecord[];
  more: boolean;
};

export type ProcessOutput = {
  code: number;
  stdout: string;
  stderr: string;
};

export type ProcessRunner = (
  program: "streams" | "hc",
  args: string[],
  options?: { cwd?: string; stdin?: string; timeout?: number; signal?: AbortSignal }
) => Promise<ProcessOutput>;

export class StreamsCliError extends Error {
  constructor(
    message: string,
    readonly kind?: string,
    readonly candidates?: string[]
  ) {
    super(message);
  }
}

const object = (value: unknown): value is Record<string, any> =>
  !!value && typeof value === "object" && !Array.isArray(value);

const unsafeReferenceCharacter = /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u;

export function safeReferenceJson(value: unknown): string {
  return JSON.stringify(value).replace(/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/gu, (character) => {
    const code = character.codePointAt(0)!;
    const escape = (unit: number) => `\\u${unit.toString(16).padStart(4, "0")}`;
    return code <= 0xffff
      ? escape(code)
      : escape(0xd800 + ((code - 0x10000) >> 10)) + escape(0xdc00 + ((code - 0x10000) & 0x3ff));
  });
}

export function referenceText(value: string): string {
  return unsafeReferenceCharacter.test(value) ? safeReferenceJson(value) : value;
}

export function unreadableNotice(unreadable: UnreadableRecord[]): string {
  if (!unreadable.length) return "";
  return [
    `${unreadable.length} unreadable record${unreadable.length === 1 ? " is" : "s are"} not listed:`,
    ...unreadable.map((record) => `${record.path}: ${record.cause}\n ${record.repair}`),
  ].join("\n");
}

export function syncNeedsAttention(result: SyncResult): boolean {
  if (result.needs_attention === true) return true;
  if (result.local === "repair-required") return true;
  return (
    result.sharing !== undefined &&
    ["failed", "blocked", "uncertain"].includes(result.sharing)
  );
}

export function formatSyncOutcome(result: SyncResult): string {
  const summary = result.sharing
    ? `${result.sharing} (local: ${result.local})`
    : result.local;
  const detail = typeof result.detail === "string" ? result.detail.trim() : "";
  return detail ? `${summary} — ${detail}` : summary;
}

/** Default process runner: invokes `streams` (via `resolveStreamsBin`) or `hc` directly, one process per call. */
export function createProcessRunner(env: NodeJS.ProcessEnv = process.env): ProcessRunner {
  return (program, args, options = {}) => {
    const signal = options.signal;
    if (signal?.aborted) {
      return Promise.reject(
        signal.reason instanceof Error ? signal.reason : new Error("Command cancelled")
      );
    }
    const executable = program === "streams" ? resolveStreamsBin(env) : env.HC_BIN_PATH || "hc";
    return new Promise<ProcessOutput>((resolve, reject) => {
      const child = spawn(executable, args, {
        cwd: options.cwd,
        env: { ...env },
        stdio: ["pipe", "pipe", "pipe"],
      });
      let stdout = "";
      let stderr = "";
      let settled = false;
      let killTimer: ReturnType<typeof setTimeout> | undefined;
      const terminateChild = () => {
        if (child.killed || child.exitCode !== null || child.signalCode !== null) return;
        child.kill("SIGTERM");
        killTimer = setTimeout(() => {
          if (child.exitCode === null && child.signalCode === null) {
            child.kill("SIGKILL");
          }
        }, 500);
        killTimer.unref?.();
      };
      const done = (fn: () => void) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        if (killTimer) clearTimeout(killTimer);
        signal?.removeEventListener("abort", onAbort);
        fn();
      };
      const onAbort = () => {
        terminateChild();
        done(() =>
          reject(signal?.reason instanceof Error ? signal.reason : new Error("Command cancelled"))
        );
      };
      const timeout = options.timeout ?? 30_000;
      const timer = setTimeout(() => {
        terminateChild();
        done(() =>
          reject(new Error(`${program} ${args.slice(0, 2).join(" ")} timed out after ${timeout}ms`))
        );
      }, timeout);
      signal?.addEventListener("abort", onAbort, { once: true });

      child.stdout?.setEncoding("utf8").on("data", (chunk: string) => {
        stdout += chunk;
      });
      child.stderr?.setEncoding("utf8").on("data", (chunk: string) => {
        stderr += chunk;
      });
      child.on("error", (error) => done(() => reject(error)));
      child.on("close", (code) => {
        done(() => resolve({ code: code ?? 1, stdout, stderr }));
      });
      if (options.stdin !== undefined) {
        child.stdin?.end(options.stdin);
      } else {
        child.stdin?.end();
      }
    });
  };
}

function parseJson(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return undefined;
  }
}

function throwCliError(output: ProcessOutput, fallback: string): never {
  const parsed = parseJson(output.stdout);
  if (object(parsed) && object(parsed.error) && typeof parsed.error.message === "string") {
    throw new StreamsCliError(
      parsed.error.message,
      typeof parsed.error.kind === "string" ? parsed.error.kind : "error",
      Array.isArray(parsed.error.candidates) ? parsed.error.candidates : undefined
    );
  }
  const message = output.stderr.trim() || output.stdout.trim() || fallback;
  throw new StreamsCliError(message, "error");
}

function validateConfigSnapshot(raw: unknown): ConfigSnapshot {
  if (!object(raw)) throw new StreamsCliError("Invalid configuration snapshot");
  const naming = raw.naming;
  if (
    !object(naming) ||
    !["automatic", "manual"].includes(naming.stream) ||
    !["automatic", "manual"].includes(naming.session)
  ) {
    throw new StreamsCliError("Invalid configuration snapshot");
  }
  const herdr = raw.herdr;
  if (
    !object(herdr) ||
    !object(herdr.viewer) ||
    !["editor", "less", "hunk", "native"].includes(herdr.viewer.mode) ||
    !["tab", "pane-right", "pane-down", "floating"].includes(herdr.viewer.placement) ||
    !object(herdr.naming) ||
    typeof herdr.naming.tab !== "boolean" ||
    typeof herdr.naming.workspace !== "boolean" ||
    !object(herdr.cards) ||
    !["automatic", "manual"].includes(herdr.cards.auto_open) ||
    !Number.isInteger(herdr.timeout_ms) ||
    herdr.timeout_ms < 1 ||
    typeof herdr.pane_height_fraction !== "number"
  ) {
    throw new StreamsCliError("Invalid configuration snapshot");
  }
  const pi = raw.pi;
  if (
    !object(pi) ||
    !object(pi.naming) ||
    typeof pi.naming.session !== "boolean" ||
    typeof pi.naming.import_edits !== "boolean"
  ) {
    throw new StreamsCliError("Invalid configuration snapshot");
  }
  const rep = pi.reporting;
  if (
    !object(rep) ||
    !["automatic", "manual"].includes(rep.mode) ||
    !["automatic", "manual"].includes(rep.enrollment) ||
    !Number.isInteger(rep.frequency) ||
    rep.frequency < 1 ||
    !["trimmed", "full"].includes(rep.context) ||
    typeof rep.session_logging !== "boolean" ||
    typeof rep.debug_commands !== "boolean"
  ) {
    throw new StreamsCliError("Invalid configuration snapshot");
  }
  if (
    rep.model !== null &&
    rep.model !== undefined &&
    (!object(rep.model) ||
      typeof rep.model.provider !== "string" ||
      !rep.model.provider.trim() ||
      typeof rep.model.id !== "string" ||
      !rep.model.id.trim())
  ) {
    throw new StreamsCliError("Invalid configuration snapshot");
  }
  return {
    naming: { stream: naming.stream, session: naming.session },
    herdr: herdr as ConfigSnapshot["herdr"],
    pi: {
      naming: pi.naming as ConfigSnapshot["pi"]["naming"],
      reporting: {
        ...rep,
        model: rep.model ?? null,
      } as ConfigSnapshot["pi"]["reporting"],
    },
  };
}

export class StreamCli {
  private namingDefaults: NamingSettings = { stream: "automatic", session: "automatic" };
  private readonly runner: ProcessRunner;
  private readonly env: NodeJS.ProcessEnv;

  constructor(
    readonly cwd: string,
    runner?: ProcessRunner,
    env: NodeJS.ProcessEnv = process.env
  ) {
    this.env = env;
    this.runner = runner ?? createProcessRunner(env);
  }

  private async runJson(
    args: string[],
    options: { stdin?: string; timeout?: number; signal?: AbortSignal; allowExit3?: boolean } = {}
  ): Promise<{ value: any; code: number }> {
    const out = await this.runner("streams", args, {
      cwd: this.cwd,
      stdin: options.stdin,
      timeout: options.timeout,
      signal: options.signal,
    });
    if (out.code !== 0 && !(options.allowExit3 && out.code === 3)) {
      throwCliError(out, `streams ${args.slice(0, 2).join(" ")} failed (exit ${out.code})`);
    }
    const parsed = parseJson(out.stdout);
    if (parsed === undefined) {
      throw new StreamsCliError(`Invalid JSON from streams ${args.slice(0, 2).join(" ")}`);
    }
    return { value: parsed, code: out.code };
  }

  /** Read `session show pi/ID --json`. Returns `null` when not enrolled (`not-found`). */
  async showSession(sessionId: string, signal?: AbortSignal): Promise<SessionWire | null> {
    const target = sessionId.includes("/") ? sessionId : `pi/${sessionId}`;
    const out = await this.runner("streams", ["session", "show", target, "--json"], {
      cwd: this.cwd,
      signal,
    });
    if (out.code !== 0) {
      const parsed = parseJson(out.stdout);
      if (object(parsed) && object(parsed.error) && parsed.error.kind === "not-found") {
        return null;
      }
      throwCliError(out, `session show ${target} failed`);
    }
    const parsed = parseJson(out.stdout);
    if (!isSession(parsed)) throw new StreamsCliError("Invalid Session record from CLI");
    return parsed;
  }

  /** Read `stream show STREAM --json`. */
  async showStream(streamKey: string, signal?: AbortSignal): Promise<StreamWire> {
    const { value } = await this.runJson(["stream", "show", streamKey, "--json"], { signal });
    if (!isStream(value)) throw new StreamsCliError("Invalid Stream record from CLI");
    return value;
  }

  /** Read this Pi conversation's Session and its Stream (or nulls if unenrolled). */
  async context(sessionId: string, signal?: AbortSignal): Promise<LocalContext> {
    const session = await this.showSession(sessionId, signal);
    if (!session) {
      return { session: null, stream: null, naming: { ...this.namingDefaults } };
    }
    const stream = await this.showStream(`${session.host_id}/${session.stream_id}`, signal);
    return { session, stream, naming: { ...this.namingDefaults } };
  }

  /** List local Streams for the initial-attachment chooser or inspection. */
  async listStreams(options: { all?: boolean; signal?: AbortSignal } = {}): Promise<StreamListing> {
    const args = [
      "stream",
      "list",
      "--local",
      "--fields",
      "key,host_id,id,title,goal,status,revision,updated_at_ms",
      "--json",
    ];
    if (options.all) args.push("--all");
    const { value } = await this.runJson(args, { signal: options.signal });
    if (
      !object(value) ||
      !Array.isArray(value.items) ||
      !Array.isArray(value.unreadable) ||
      !value.unreadable.every(isUnreadableRecord)
    ) {
      throw new StreamsCliError("Invalid stream list response from CLI");
    }
    return {
      items: value.items as StreamWire[],
      unreadable: value.unreadable as UnreadableRecord[],
      more: Boolean(value.more),
    };
  }

  /** List Sessions of a Stream (used by prompt-lab capture). */
  async listStreamSessions(
    streamKey: string,
    signal?: AbortSignal
  ): Promise<{ items: SessionWire[]; unreadable: UnreadableRecord[] }> {
    const { value } = await this.runJson(
      [
        "session",
        "list",
        "--stream",
        streamKey,
        "--all",
        "--fields",
        "key,host_id,harness,id,stream_id,title,status,recap,next,directory,references,artifacts,revision,updated_at_ms",
        "--json",
      ],
      { signal }
    );
    if (!object(value) || !Array.isArray(value.items)) {
      throw new StreamsCliError("Invalid session list response from CLI");
    }
    return {
      items: value.items as SessionWire[],
      unreadable: Array.isArray(value.unreadable) ? (value.unreadable as UnreadableRecord[]) : [],
    };
  }

  /** Create a new Stream (`stream create ... --json`). */
  async createStream(
    input: {
      title: string;
      titleMode?: Mode;
      goal?: string;
      goalShort?: string;
      goalMode?: Mode;
    },
    signal?: AbortSignal
  ): Promise<{ key: string; files: string[] }> {
    const args = ["stream", "create", "--title", input.title];
    if (input.titleMode) args.push("--title-mode", input.titleMode);
    if (input.goal) {
      args.push("--goal", input.goal);
      if (input.goalShort) args.push("--goal-short", input.goalShort);
      if (input.goalMode) args.push("--goal-mode", input.goalMode);
    }
    args.push("--json");
    const { value } = await this.runJson(args, { signal });
    if (!object(value) || typeof value.key !== "string") {
      throw new StreamsCliError("Invalid stream create response from CLI");
    }
    return { key: value.key, files: Array.isArray(value.files) ? value.files : [] };
  }

  /** Create a Session record for this Pi conversation (`session create pi/ID ... --json`). */
  async createSession(
    sessionId: string,
    input: {
      stream: string;
      title: string;
      titleMode?: Mode;
      directory?: string;
    },
    signal?: AbortSignal
  ): Promise<{ key: string; files: string[] }> {
    const pair = sessionId.includes("/") ? sessionId : `pi/${sessionId}`;
    const args = [
      "session",
      "create",
      pair,
      "--stream",
      input.stream,
      "--title",
      input.title,
    ];
    if (input.titleMode) args.push("--title-mode", input.titleMode);
    if (input.directory) args.push("--directory", input.directory);
    args.push("--json");
    const { value } = await this.runJson(args, { signal });
    if (!object(value) || typeof value.key !== "string") {
      throw new StreamsCliError("Invalid session create response from CLI");
    }
    return { key: value.key, files: Array.isArray(value.files) ? value.files : [] };
  }

  /** Apply an edit body (`{ changes, before? }`) to a Session (`session edit TARGET --input - --json`). */
  async editSession(
    target: string,
    body: EditBody,
    signal?: AbortSignal
  ): Promise<EditResult> {
    const selector = target.includes("/") ? target : `pi/${target}`;
    const { value, code } = await this.runJson(
      ["session", "edit", selector, "--input", "-", "--json"],
      { stdin: JSON.stringify(body), signal, allowExit3: true }
    );
    if (!object(value) || typeof value.key !== "string" || !object(value.not_written)) {
      throw new StreamsCliError("Invalid session edit response from CLI");
    }
    return { key: value.key, not_written: value.not_written, partial: code === 3 };
  }

  /** Apply an edit body (`{ changes, before? }`) to a Stream (`stream edit TARGET --input - --json`). */
  async editStream(
    streamKey: string,
    body: EditBody,
    signal?: AbortSignal
  ): Promise<EditResult> {
    const { value, code } = await this.runJson(
      ["stream", "edit", streamKey, "--input", "-", "--json"],
      { stdin: JSON.stringify(body), signal, allowExit3: true }
    );
    if (!object(value) || typeof value.key !== "string" || !object(value.not_written)) {
      throw new StreamsCliError("Invalid stream edit response from CLI");
    }
    return { key: value.key, not_written: value.not_written, partial: code === 3 };
  }

  /** Mark this Pi Session `active` on startup or `parked` on shutdown, guarded by `before`. */
  async markLifecycle(
    sessionId: string,
    action: "resume" | "park",
    signal?: AbortSignal
  ): Promise<{ changed: boolean; session: SessionWire | null }> {
    const session = await this.showSession(sessionId, signal);
    if (!session) return { changed: false, session: null };
    const next: Status | null =
      action === "resume"
        ? session.status === "parked"
          ? "active"
          : null
        : session.status === "active"
          ? "parked"
          : null;
    if (!next) return { changed: false, session };
    const res = await this.editSession(
      sessionKeyOf(session),
      { changes: { status: next }, before: { status: session.status } },
      signal
    );
    const changed = !Object.hasOwn(res.not_written, "status");
    return { changed, session };
  }

  async getConfig(signal?: AbortSignal): Promise<ConfigSnapshot> {
    const { value } = await this.runJson(["config", "get", "--json"], { signal });
    const snapshot = validateConfigSnapshot(value);
    this.namingDefaults = { ...snapshot.naming };
    return snapshot;
  }

  async setConfig(key: string, val: string, signal?: AbortSignal): Promise<ConfigSnapshot> {
    await this.runJson(["config", "set", key, val, "--json"], { signal });
    return this.getConfig(signal);
  }

  async unsetConfig(key: string, signal?: AbortSignal): Promise<ConfigSnapshot> {
    await this.runJson(["config", "unset", key, "--json"], { signal });
    return this.getConfig(signal);
  }

  async sync(
    signal?: AbortSignal,
    timeoutMs = 30_000,
    options: { ifConfigured?: boolean; background?: boolean } = {}
  ): Promise<SyncResult> {
    const timeoutSeconds = Math.max(1, Math.floor(timeoutMs / 1000));
    const args = ["catalog", "sync", "--json", "--timeout", String(timeoutSeconds)];
    if (options.ifConfigured) args.push("--if-configured");
    if (options.background) args.push("--background");
    // Give the Rust CLI ownership of the deadline so it reaps child Git process groups,
    // persists last-sync.json, and releases locks before Node's backstop fires.
    const runnerTimeoutMs = timeoutMs + 5_000;
    const output = await this.runner("streams", args, {
      cwd: this.cwd,
      timeout: runnerTimeoutMs,
      signal,
    });
    if (output.code !== 0 && output.code !== 3) {
      throwCliError(output, `streams catalog sync failed (exit ${output.code})`);
    }
    if (options.background && output.code === 0 && output.stdout.trim() === "") {
      return {
        local: "unchanged",
        sharing: "local-only",
        detail: "",
        needs_attention: false,
      };
    }
    const value = parseJson(output.stdout);
    if (!object(value) || typeof value.local !== "string") {
      throw new StreamsCliError("Invalid catalog sync response from CLI");
    }
    return {
      ...(value as SyncResult),
      needs_attention: output.code === 3 || syncNeedsAttention(value as SyncResult),
    };
  }

  async artifact(
    sessionId: string,
    request: ArtifactRequest,
    signal?: AbortSignal
  ): Promise<Record<string, any>> {
    const pair = `pi/${sessionId}`;
    if (request.action === "write") {
      const args = [
        "artifact",
        "write",
        request.name,
        "--session",
        pair,
        "--description",
        request.description,
      ];
      if (request.file !== undefined) {
        args.push("--file", request.file);
      } else if (request.text === undefined || request.description_only) {
        args.push("--description-only");
      }
      args.push("--json");
      const { value } = await this.runJson(args, {
        stdin: request.text,
        signal,
      });
      return value;
    }
    if (request.action === "delete") {
      const { value } = await this.runJson(
        ["artifact", "delete", request.reference, "--confirm", "--json"],
        { signal, allowExit3: true }
      );
      return value;
    }
    if (request.action === "read") {
      const out = await this.runner("streams", ["artifact", "read", request.reference], {
        cwd: this.cwd,
        signal,
      });
      if (out.code !== 0) throwCliError(out, `artifact read ${request.reference} failed`);
      const buf = Buffer.from(out.stdout, "utf8");
      const offset = request.offset ?? 0;
      const limit = request.limit ?? 32_768;
      const slice = buf.subarray(offset, offset + limit);
      const content = slice.toString("utf8");
      const nextOffset = offset + slice.length < buf.length ? offset + slice.length : null;
      return {
        reference: request.reference,
        total_bytes: buf.length,
        offset,
        bytes: slice.length,
        ...(nextOffset !== null ? { next_offset: nextOffset } : {}),
        content,
      };
    }
    if (request.action === "list_stream") {
      const { value } = await this.runJson(
        ["artifact", "list", "--stream", request.stream, "--json"],
        { signal }
      );
      return value;
    }
    const targetSession = request.session ?? pair;
    const { value } = await this.runJson(
      ["artifact", "list", "--session", targetSession, "--json"],
      { signal }
    );
    return value;
  }

  /** Classic Herdr card commands (`card open`, `card close`, `card mode`). */
  async cardOpen(sessionId: string, pluginRoot?: string, signal?: AbortSignal): Promise<Record<string, any>> {
    const args = ["card", "open", `pi/${sessionId}`];
    if (pluginRoot) args.push("--plugin-root", pluginRoot);
    args.push("--json");
    const { value } = await this.runJson(args, { signal });
    return value;
  }

  async cardClose(sessionId: string, signal?: AbortSignal): Promise<Record<string, any>> {
    const { value } = await this.runJson(["card", "close", `pi/${sessionId}`, "--json"], {
      signal,
    });
    return value;
  }

  async cardMode(
    sessionId: string,
    mode?: CardMode,
    signal?: AbortSignal
  ): Promise<Record<string, any>> {
    const args = ["card", "mode", `pi/${sessionId}`];
    if (mode) args.push(mode);
    args.push("--json");
    const { value } = await this.runJson(args, { signal });
    return value;
  }

  /**
   * Write `streams.session=<sessionKey>` (and optionally `streams.stream=<streamKey>`) on
   * `HC_TERMINAL_ID` via `hc label`. No-op when `HC_TERMINAL_ID` is unset.
   */
  async writeSessionLabel(
    sessionKey: string | null,
    signal?: AbortSignal,
    streamKey?: string | null
  ): Promise<void> {
    const terminalId = this.env.HC_TERMINAL_ID;
    if (!terminalId) return;
    const pairs = [`streams.session=${sessionKey ?? ""}`];
    if (streamKey !== undefined) {
      pairs.push(`streams.stream=${streamKey ?? ""}`);
    }
    const out = await this.runner("hc", ["label", terminalId, ...pairs], {
      cwd: this.cwd,
      timeout: 5_000,
      signal,
    });
    if (out.code !== 0) {
      throw new StreamsCliError(
        out.stderr.trim() || `hc label ${terminalId} streams.session failed`,
        "error"
      );
    }
  }

  private async readHcTerminal(
    address: string,
    signal?: AbortSignal
  ): Promise<{ labels?: Record<string, string>; parent?: string | null } | null> {
    try {
      const out = await this.runner("hc", ["get", address, "--json"], {
        cwd: this.cwd,
        timeout: 3_000,
        signal,
      });
      if (out.code !== 0) return null;
      const parsed = parseJson(out.stdout);
      return object(parsed) ? (parsed as { labels?: Record<string, string>; parent?: string | null }) : null;
    } catch {
      return null;
    }
  }

  /**
   * Discover an explicit pre-bound Stream (from `streams.stream` on `HC_TERMINAL_ID`) or candidate
   * local Streams for automatic enrollment, annotated with whether each Stream is open in the
   * current Herdr space or owned by this terminal's parent.
   */
  async enrollmentCandidates(signal?: AbortSignal): Promise<{
    attachedStream: StreamWire | null;
    candidates: {
      key: string;
      title: string;
      goal?: string;
      status: string;
      in_current_space?: boolean;
      from_parent_terminal?: boolean;
    }[];
  }> {
    const spaceStreamKeys = new Set<string>();
    const parentStreamKeys = new Set<string>();
    const terminalId = this.env.HC_TERMINAL_ID;

    if (terminalId) {
      const self = await this.readHcTerminal(terminalId, signal);
      const selfStream = self?.labels?.["streams.stream"]?.trim();
      if (selfStream) {
        try {
          const attachedStream = await this.showStream(selfStream, signal);
          return { attachedStream, candidates: [] };
        } catch {
          // Stale label; continue with candidate discovery.
        }
      }

      const parentAddr = typeof self?.parent === "string" && self.parent.trim() ? self.parent.trim() : null;
      const spaceId = self?.labels?.["herdr.space"]?.trim() || null;

      if (parentAddr) {
        const parentInfo = await this.readHcTerminal(parentAddr, signal);
        const parentStream = parentInfo?.labels?.["streams.stream"]?.trim();
        if (parentStream) {
          parentStreamKeys.add(parentStream);
          if (spaceId && parentInfo?.labels?.["herdr.space"]?.trim() === spaceId) {
            spaceStreamKeys.add(parentStream);
          }
        }
      }

      if (spaceId) {
        try {
          const lsOut = await this.runner(
            "hc",
            ["ls", "--label", `herdr.space=${spaceId}`, "--json"],
            { cwd: this.cwd, timeout: 3_000, signal }
          );
          if (lsOut.code === 0) {
            const parsed = parseJson(lsOut.stdout);
            const rawItems = Array.isArray(parsed)
              ? parsed
              : object(parsed) && Array.isArray(parsed.terminals)
                ? parsed.terminals
                : [];
            const siblingAddresses = rawItems
              .map((entry) => (object(entry) && typeof entry.address === "string" ? entry.address : null))
              .filter((addr): addr is string => Boolean(addr && addr !== terminalId))
              .slice(0, 16);
            const siblings = await Promise.all(
              siblingAddresses.map((addr) => this.readHcTerminal(addr, signal))
            );
            for (const sib of siblings) {
              const sibStream = sib?.labels?.["streams.stream"]?.trim();
              if (sibStream) spaceStreamKeys.add(sibStream);
            }
          }
        } catch {
          // Non-fatal: fall back to catalog listing only.
        }
      }
    }

    const byKey = new Map<string, StreamWire>();
    try {
      const listed = await this.listStreams({ signal });
      for (const stream of listed.items) {
        byKey.set(streamKeyOf(stream), stream);
      }
    } catch {
      // Non-fatal: still include any Stream explicitly found via hc labels.
    }

    // Ensure Streams referenced by current-space siblings or parent are present even if marked done.
    const extraKeys = [...new Set([...parentStreamKeys, ...spaceStreamKeys])].filter(
      (key) => !byKey.has(key)
    );
    if (extraKeys.length > 0) {
      const fetched = await Promise.all(
        extraKeys.map(async (key) => {
          try {
            return await this.showStream(key, signal);
          } catch {
            return null;
          }
        })
      );
      for (const stream of fetched) {
        if (stream) byKey.set(streamKeyOf(stream), stream);
      }
    }

    const ranked = [...byKey.entries()]
      .sort(([keyA, a], [keyB, b]) => {
        const parentDiff = Number(parentStreamKeys.has(keyB)) - Number(parentStreamKeys.has(keyA));
        if (parentDiff !== 0) return parentDiff;
        const spaceDiff = Number(spaceStreamKeys.has(keyB)) - Number(spaceStreamKeys.has(keyA));
        if (spaceDiff !== 0) return spaceDiff;
        const aTime = (a as { updated_at_ms?: number }).updated_at_ms ?? 0;
        const bTime = (b as { updated_at_ms?: number }).updated_at_ms ?? 0;
        if (bTime !== aTime) return bTime - aTime;
        return a.title.text.localeCompare(b.title.text);
      })
      .slice(0, 25)
      .map(([key, stream]) => ({
        key,
        title: stream.title.text,
        ...(stream.goal?.text ? { goal: stream.goal.text } : {}),
        status: stream.status,
        ...(spaceStreamKeys.has(key) ? { in_current_space: true } : {}),
        ...(parentStreamKeys.has(key) ? { from_parent_terminal: true } : {}),
      }));

    return { attachedStream: null, candidates: ranked };
  }
}

export function formatNotWritten(notWritten: Record<string, unknown>): string[] {
  const lines: string[] = [];
  for (const [field, value] of Object.entries(notWritten)) {
    if (field === "references" && object(value)) {
      for (const [refKey, current] of Object.entries(value)) {
        lines.push(`reference ${referenceText(refKey)} not written (current: ${safeReferenceJson(current)})`);
      }
    } else {
      lines.push(`${field} not written (current: ${safeReferenceJson(value)})`);
    }
  }
  return lines;
}

export function formatEditSummary(label: string, result: EditResult): string {
  const skipped = formatNotWritten(result.not_written);
  if (skipped.length === 0) return label;
  return `${label}\n${skipped.join("\n")}`;
}
