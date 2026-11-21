import { join } from "node:path";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { getAgentDir } from "@earendil-works/pi-coding-agent";
import type { ModelSelection } from "./agent.ts";
import {
  buildCaptureInventory,
  captureIds,
  captureSessions,
  loadCapture,
  privateDirectory,
} from "./capture.ts";
import { confirmCapture } from "./capture-dialog.ts";
import { replayCapture } from "./replay.ts";
import type { ReportAction, ResetTarget } from "./report-workflow.ts";
import type { StreamCli } from "./streams.ts";
import { parseModel } from "./settings.ts";

export type DebugDeps = {
  cli(ctx: ExtensionContext): StreamCli;
  exclusive<T>(run: (signal: AbortSignal) => Promise<T>): Promise<T>;
  enabled(): boolean;
  defaultModel(): ModelSelection | undefined;
  helperRoot?: () => string | undefined;
  root?: () => string;
};

const debugRoot = (deps: DebugDeps) =>
  deps.root?.() ?? join(getAgentDir(), "stream-debug");

function ensureEnabled(deps: DebugDeps): void {
  if (!deps.enabled()) {
    throw new Error(
      "Streams debug commands are disabled; enable them with: streams config set pi.reporting.debug_commands true"
    );
  }
}

export async function runCapture(ctx: ExtensionContext, deps: DebugDeps): Promise<void> {
  ensureEnabled(deps);
  if (!ctx.hasUI) throw new Error("Capture requires an interactive UI");
  await deps.exclusive(async (signal) => {
    const cli = deps.cli(ctx);
    const list = await buildCaptureInventory(cli, undefined, getAgentDir(), signal);
    if (!list.streams.length) {
      ctx.ui.notify("No streams available to capture.", "info");
      return;
    }
    const labels = list.streams.map((s) => `${s.id} · ${s.title}`);
    const chosen = await ctx.ui.select("Select Stream to capture", [...labels, "Cancel"], { signal });
    if (!chosen || chosen === "Cancel") return;
    const stream = list.streams[labels.indexOf(chosen)];
    if (!stream) return;
    const inventory = await buildCaptureInventory(cli, stream.key, getAgentDir(), signal);
    const root = debugRoot(deps);
    if (!(await confirmCapture(ctx, inventory, root, signal))) return;
    privateDirectory(root, root);
    const { capture, directory } = captureSessions(
      inventory,
      root,
      ctx.sessionManager,
      signal,
      deps.helperRoot?.()
    );
    ctx.ui.notify(
      `Captured ${capture.samples.length} sample(s) to ${directory} (${capture.id})`,
      "info"
    );
  });
}

function parseReplayArgs(args: string): {
  captureId?: string;
  model?: ModelSelection;
  action: ReportAction;
} {
  const tokens = args.trim().split(/\s+/).filter(Boolean);
  let captureId: string | undefined;
  let model: ModelSelection | undefined;
  let reset: ResetTarget | undefined;
  let auto = false;
  for (let i = 0; i < tokens.length; i++) {
    const token = tokens[i];
    if (token === "--model") {
      const value = tokens[++i];
      if (!value) throw new Error("--model requires PROVIDER/ID");
      model = parseModel(value);
      if (!model) throw new Error("--model requires PROVIDER/ID");
    } else if (token === "--reset") {
      const next = tokens[i + 1];
      if (next && ["session", "stream", "all"].includes(next)) {
        reset = tokens[++i] as ResetTarget;
      } else {
        reset = "all";
      }
    } else if (token === "--auto") {
      auto = true;
    } else if (!captureId && !token.startsWith("-")) {
      captureId = token;
    } else {
      throw new Error(`Unknown replay argument: ${token}`);
    }
  }
  if (auto && !reset) throw new Error("--auto requires --reset");
  const action: ReportAction = reset
    ? { kind: "reset", target: reset, auto }
    : { kind: "report", force: true };
  return { captureId, model, action };
}

export async function runReplay(
  ctx: ExtensionContext,
  args: string,
  deps: DebugDeps
): Promise<void> {
  ensureEnabled(deps);
  const parsed = parseReplayArgs(args);
  await deps.exclusive(async (signal) => {
    const root = debugRoot(deps);
    let id = parsed.captureId;
    if (!id) {
      const ids = captureIds(root);
      if (!ids.length) {
        ctx.ui.notify("No captures found. Run /streams capture first.", "info");
        return;
      }
      if (!ctx.hasUI) throw new Error("Pass a capture ID to replay non-interactively");
      const chosen = await ctx.ui.select("Select capture to replay", [...ids, "Cancel"], { signal });
      if (!chosen || chosen === "Cancel") return;
      id = chosen;
    }
    const model =
      parsed.model ??
      deps.defaultModel() ??
      (ctx.model ? { provider: ctx.model.provider, id: ctx.model.id } : undefined);
    if (!model) throw new Error("Choose a model with --model PROVIDER/ID");
    const capture = loadCapture(root, id);
    const { run, directory } = await replayCapture(
      capture,
      root,
      ctx,
      model,
      parsed.action,
      signal
    );
    ctx.ui.notify(
      `Replay ${run.id} completed (${directory}): ${run.samples.filter((s) => s.status === "completed").length}/${run.samples.length} succeeded`,
      "info"
    );
  });
}
