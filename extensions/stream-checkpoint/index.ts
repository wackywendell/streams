import { join } from "node:path";
import { Type } from "typebox";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { getAgentDir } from "@earendil-works/pi-coding-agent";
import { infer as modelInfer, type DiscussionResult } from "./agent.ts";
import { registerArtifactTool } from "./artifacts.ts";
import { chooseInitial } from "./chooser.ts";
import { buildBeforeForChanges } from "./core.ts";
import { registerLabTools } from "./lab-tools.ts";
import { executeLab } from "./lab.ts";
import { runCapture, runReplay } from "./debug.ts";
import { namingCompletions, parseNamingEdit, saveNamingEdit, savePiNameEdit } from "./naming.ts";
import { NativeController } from "./native.ts";
import { compose, openCard, type CardState } from "./panel.ts";
import { Reporter, type ResetTarget } from "./reporter.ts";
import { projectSessionName } from "./session-naming.ts";
import { captureInputs, hasConversation } from "./session-scope.ts";
import {
  defaults,
  formatModel,
  parseModel,
  reportingSettingsFromSnapshot,
  settingsDialog,
  type ReportingSettings,
} from "./settings.ts";
import {
  StreamCli,
  formatNotWritten,
  sessionKeyOf,
  streamKeyOf,
  type ConfigSnapshot,
  type EditBody,
  type EditResult,
  type ProcessRunner,
} from "./streams.ts";

export type ExtensionOptions = {
  runner?: ProcessRunner;
  infer?: typeof modelInfer;
  native?: boolean;
  debugTools?: boolean;
};

const emptyCardState = (): CardState => ({
  discussion: { input: "", messages: [] },
  edits: new Map(),
});

function parseResetArgs(args: string): { target: ResetTarget; auto: boolean } {
  const tokens = args.trim().split(/\s+/).filter(Boolean);
  let target: ResetTarget = "all";
  let auto = false;
  let seenTarget = false;
  for (const token of tokens) {
    if (token === "--auto") {
      auto = true;
    } else if (!seenTarget && (token === "session" || token === "stream" || token === "all")) {
      target = token;
      seenTarget = true;
    } else {
      throw new Error("Usage: /streams reset [session|stream|all] [--auto]");
    }
  }
  return { target, auto };
}

export function createStreamExtension(options: ExtensionOptions = {}) {
  return function streamCheckpointExtension(pi: ExtensionAPI): void {
    const clients = new Map<string, StreamCli>();
    const client = (ctx: Pick<ExtensionContext, "cwd">): StreamCli => {
      let existing = clients.get(ctx.cwd);
      if (!existing) {
        existing = new StreamCli(ctx.cwd, options.runner);
        clients.set(ctx.cwd, existing);
      }
      return existing;
    };

    let configCache: ConfigSnapshot | undefined;
    let settings: ReportingSettings = { ...defaults };
    let cardState: CardState = emptyCardState();
    let currentCtx: ExtensionContext | undefined;
    let native: NativeController | undefined;

    const startNative = (ctx: ExtensionContext): NativeController | undefined => {
      if (options.native === false) return undefined;
      native?.stop();
      native = new NativeController({
        pi,
        ctx,
        cli: client(ctx),
        piNamingSession: () => configCache?.pi.naming.session ?? true,
        current: () => currentCtx,
        warning: (text) => notice(text, "warning"),
      });
      void native.prepareCatalog();
      return native;
    };

    const projectPiName = (ctx: ExtensionContext, title: string) => {
      if (!(configCache?.pi.naming.session ?? true)) return;
      const nativeId = ctx.sessionManager.getSessionId();
      try {
        projectSessionName(pi, ctx.sessionManager, nativeId, title, () => {
          if (currentCtx?.sessionManager.getSessionId() !== nativeId) {
            throw new Error("Session changed");
          }
        });
      } catch {
        // Non-fatal Pi session name projection failure
      }
    };

    const notice = (text: string, level: "info" | "warning" = "warning") => {
      if (currentCtx?.hasUI) currentCtx.ui.notify(text, level);
    };

    const refreshConfig = async (ctx: ExtensionContext): Promise<ConfigSnapshot | undefined> => {
      try {
        configCache = await client(ctx).getConfig();
        settings = reportingSettingsFromSnapshot(configCache);
        return configCache;
      } catch {
        return configCache;
      }
    };

    const helperRoot = () => join(getAgentDir(), "stream-helpers");

    const onEnrolled = async (
      ctx: ExtensionContext,
      info: { streamKey: string; sessionKey: string; sessionTitle: string }
    ) => {
      try {
        await client(ctx).writeSessionLabel(info.sessionKey);
      } catch {
        // Non-fatal label write
      }
      projectPiName(ctx, info.sessionTitle);
      void native?.prepareCatalog();
    };

    const onEditResult = (
      ctx: ExtensionContext,
      kind: "session" | "stream",
      body: EditBody,
      _result: EditResult
    ) => {
      if (kind === "session" && body.changes.title && typeof body.changes.title === "object") {
        const text = (body.changes.title as { text?: string }).text;
        if (text) projectPiName(ctx, text);
      }
    };

    const reporter = new Reporter({
      cli: client,
      settings: () => settings,
      infer: options.infer,
      prepare: async () => {
        if (currentCtx) await refreshConfig(currentCtx);
      },
      logging: () =>
        settings.session_logging
          ? {
              directory: helperRoot(),
              onCreate: (value) => {
                pi.appendEntry("stream-helper-session", value);
              },
            }
          : undefined,
      notice,
      onEnrolled,
      onEditResult,
      activity: (text) => {
        if (currentCtx?.hasUI) {
          currentCtx.ui.setStatus("streams", text ? `Streams: ${text}` : undefined);
        }
      },
    });

    const syncAttachedSession = async (ctx: ExtensionContext, markResumed: boolean) => {
      currentCtx = ctx;
      reporter.scope.use(ctx);
      cardState = emptyCardState();
      await refreshConfig(ctx);
      startNative(ctx);
      const nativeId = ctx.sessionManager.getSessionId();
      try {
        const catalog = await client(ctx).context(nativeId);
        if (catalog.session && markResumed) {
          try {
            await client(ctx).markLifecycle(nativeId, "resume");
          } catch {
            // Best-effort lifecycle update
          }
        }
      } catch {
        // Unreadable or unavailable catalog does not block Pi startup
      }
    };

    pi.on("session_start", async (_event, ctx) => {
      await syncAttachedSession(ctx, true);
    });

    pi.on("session_switch", async (_event, ctx) => {
      reporter.invalidate();
      await syncAttachedSession(ctx, true);
    });

    pi.on("session_shutdown", async (_event, ctx) => {
      reporter.scope.stop();
      const nativeId = ctx.sessionManager.getSessionId();
      native?.stop();
      await native?.idle();
      try {
        await client(ctx).markLifecycle(nativeId, "park");
      } catch {
        // Best-effort park on shutdown
      }
    });

    pi.on("agent_end", (_event, ctx) => {
      currentCtx = ctx;
      reporter.response(ctx);
    });

    pi.on("session_name" as any, async (event: any, ctx: ExtensionContext) => {
      const name = typeof event?.name === "string" ? event.name : undefined;
      const nativeId = ctx.sessionManager.getSessionId();
      try {
        await savePiNameEdit(client(ctx), nativeId, name, () => {
          if (ctx.sessionManager.getSessionId() !== nativeId) {
            throw new Error("Session changed");
          }
        });
      } catch {
        // Ignore non-fatal Pi rename sync errors
      }
    });

    const discuss = async (ctx: ExtensionContext): Promise<void> => {
      if (!ctx.hasUI) throw new Error("Card discussion requires an interactive UI");
      for (;;) {
        const message = await compose(ctx, cardState.discussion);
        if (!message) return;
        cardState.discussion.input = "";
        cardState.discussion.error = undefined;
        cardState.discussion.messages.push({ role: "user", text: message });
        try {
          const result = await reporter.exclusive<DiscussionResult>(async (signal) => {
            const fence = reporter.scope.fence();
            const capture = await captureInputs(
              ctx,
              client(ctx),
              settings.context,
              fence,
              "discussion"
            );
            const generate = options.infer ?? modelInfer;
            const inferred = (await generate(
              ctx,
              {
                type: "discussion",
                catalog: capture.catalog,
                messages: capture.evidence.messages,
                discussion: cardState.discussion.messages,
              },
              signal,
              settings.model,
              settings.session_logging
                ? {
                    directory: helperRoot(),
                    onCreate: (v) => pi.appendEntry("stream-helper-session", v),
                  }
                : undefined
            )) as DiscussionResult;

            if (inferred.edit && capture.valid() && !signal.aborted) {
              const target = capture.catalog[inferred.edit.kind];
              if (target) {
                const before = buildBeforeForChanges(target, inferred.edit.changes);
                const body: EditBody = { changes: inferred.edit.changes, before };
                const res =
                  inferred.edit.kind === "session"
                    ? await client(ctx).editSession(sessionKeyOf(capture.catalog.session!), body, signal)
                    : await client(ctx).editStream(streamKeyOf(capture.catalog.stream!), body, signal);
                onEditResult(ctx, inferred.edit.kind, body, res);
                for (const skipped of formatNotWritten(res.not_written)) {
                  notice(`Discussion edit skipped: ${skipped}`, "warning");
                }
              }
            }
            return inferred;
          });
          cardState.discussion.messages.push({ role: "assistant", text: result.reply });
        } catch (error) {
          cardState.discussion.error = error instanceof Error ? error.message : String(error);
        }
      }
    };

    const runSettings = async (ctx: ExtensionContext): Promise<void> => {
      const cfg = await refreshConfig(ctx);
      if (!cfg) {
        ctx.ui.notify("Could not read Streams configuration.", "error");
        return;
      }
      await settingsDialog(ctx, cfg, {
        setConfig: async (key, value) => {
          const updated = await client(ctx).setConfig(key, value);
          configCache = updated;
          settings = reportingSettingsFromSnapshot(updated);
          return updated;
        },
        unsetConfig: async (key) => {
          const updated = await client(ctx).unsetConfig(key);
          configCache = updated;
          settings = reportingSettingsFromSnapshot(updated);
          return updated;
        },
      });
    };

    // --- Tools ---

    pi.registerTool({
      name: "stream_read",
      label: "Read Stream state",
      description:
        "Read typed durable catalog state and recorded contributions from persisted local records. No fetch, scratch notes or pending drafts.",
      parameters: Type.Object({}),
      async execute(_id, _args, signal, _update, ctx) {
        const catalog = await client(ctx).context(ctx.sessionManager.getSessionId(), signal);
        return {
          content: [{ type: "text" as const, text: JSON.stringify(catalog, null, 2) }],
          details: catalog,
        };
      },
    });

    pi.registerTool({
      name: "checkpoint",
      label: "Request progress report",
      description:
        "Request an immediate durable progress report. Local saves and publication are separate; this is not an approval draft.",
      parameters: Type.Object({
        instructions: Type.Optional(Type.String()),
      }),
      async execute(_id, args, _signal, _update, ctx) {
        currentCtx = ctx;
        if (!hasConversation(ctx.sessionManager)) {
          return {
            content: [
              { type: "text" as const, text: "Start a conversation before reporting a work card." },
            ],
            details: { status: "skipped" },
          };
        }
        reporter.request(ctx, true, args.instructions);
        return {
          content: [{ type: "text" as const, text: "Work card report requested." }],
          details: { status: "requested" },
        };
      },
    });

    pi.registerTool({
      name: "select_stream",
      label: "Attach to Stream",
      description:
        "Ask the user to attach this unbound Pi conversation to an exact catalog stream ID. An enrolled conversation cannot switch; membership moves/forks are not implemented.",
      parameters: Type.Object({
        stream_id: Type.String(),
        reason: Type.String(),
      }),
      async execute(_id, args, signal, _update, ctx) {
        const result = await chooseInitial(
          ctx,
          client(ctx),
          { streamId: args.stream_id, reason: args.reason },
          (info) => {
            void onEnrolled(ctx, info);
          },
          signal
        );
        return {
          content: [{ type: "text" as const, text: JSON.stringify(result) }],
          details: result,
        };
      },
    });

    pi.registerTool({
      name: "create_stream",
      label: "Create Stream",
      description:
        "Ask the user to create an initial durable Stream and enroll this conversation. Never switches existing membership or launches workspaces.",
      parameters: Type.Object({
        title: Type.String(),
        reason: Type.String(),
      }),
      async execute(_id, args, signal, _update, ctx) {
        const result = await chooseInitial(
          ctx,
          client(ctx),
          { title: args.title, reason: args.reason },
          (info) => {
            void onEnrolled(ctx, info);
          },
          signal
        );
        return {
          content: [{ type: "text" as const, text: JSON.stringify(result) }],
          details: result,
        };
      },
    });

    registerArtifactTool(pi, client);

    const debugDeps = {
      cli: client,
      exclusive: <T>(run: (signal: AbortSignal) => Promise<T>) => reporter.exclusive(run),
      enabled: () => settings.debug_commands,
      defaultModel: () => settings.model,
      helperRoot,
    };
    registerLabTools(pi, {
      enabled: () => options.debugTools ?? settings.debug_commands,
      execute: (operation, signal, ctx) =>
        executeLab(
          operation,
          {
            root: join(getAgentDir(), "stream-debug"),
            agentDir: getAgentDir(),
            client: client(ctx),
            runtime: ctx,
            current: ctx.sessionManager,
            helperRoot: helperRoot(),
          },
          signal
        ),
    });

    // --- Slash command ---

    const subcommands = [
      { value: "card", label: "card", description: "Open the work card dialog" },
      { value: "report", label: "report", description: "Run an immediate progress report" },
      { value: "reset", label: "reset", description: "Regenerate automatic fields [session|stream|all] [--auto]" },
      { value: "attach", label: "attach", description: "Attach this conversation to a Stream" },
      { value: "discuss", label: "discuss", description: "Discuss work card fields with the helper" },
      { value: "name", label: "name", description: "Set saved title [session|stream] <name>" },
      { value: "title-mode", label: "title-mode", description: "Set title mode [session|stream] automatic|manual" },
      { value: "settings", label: "settings", description: "Configure reporting, naming, and model" },
      { value: "sync", label: "sync", description: "Synchronize catalog with remote" },
      { value: "open", label: "open", description: "Open the classic Herdr work card pane" },
      { value: "close", label: "close", description: "Close the classic Herdr work card pane" },
    ];

    pi.registerCommand("streams", {
      description: "Manage Streams work cards, progress reports, naming, and settings",
      getArgumentCompletions: (prefix) => {
        const naming = namingCompletions(prefix);
        if (naming) return naming;
        const trimmed = prefix.trimStart();
        if (!trimmed.includes(" ")) {
          const matches = subcommands.filter((item) => item.label.startsWith(trimmed));
          return matches.length ? matches : null;
        }
        return null;
      },
      handler: async (rawArgs, ctx) => {
        currentCtx = ctx;
        const trimmed = rawArgs.trim();
        const [command = "", ...restParts] = trimmed ? trimmed.split(/\s+/) : [];
        const rest = restParts.join(" ");
        try {
          if (!command || command === "card") {
            await openCard(ctx, client(ctx), cardState, {
              onResult: (res) => {
                void res;
              },
              discuss: () => discuss(ctx),
              settings: () => runSettings(ctx),
              report: () => reporter.request(ctx, true),
              reset: (auto, target) => reporter.reset(ctx, auto, target),
            });
          } else if (command === "report") {
            reporter.request(ctx, true, rest || undefined);
          } else if (command === "reset") {
            const { target, auto } = parseResetArgs(rest);
            reporter.reset(ctx, auto, target);
          } else if (command === "attach") {
            const res = await chooseInitial(ctx, client(ctx), {}, (info) => {
              void onEnrolled(ctx, info);
            });
            if (res.status === "enrolled") {
              ctx.ui.notify(`Attached to Stream ${res.stream_id}`, "info");
            }
          } else if (command === "discuss") {
            await discuss(ctx);
          } else if (command === "name" || command === "title-mode") {
            const edit = parseNamingEdit(command, rest);
            const nativeId = ctx.sessionManager.getSessionId();
            const res = await saveNamingEdit(client(ctx), nativeId, edit, () => {
              if (ctx.sessionManager.getSessionId() !== nativeId) {
                throw new Error("Session changed");
              }
            });
            if (edit.target === "session" && edit.kind === "name") {
              projectPiName(ctx, edit.text);
            }
            const skipped = formatNotWritten(res.not_written);
            if (skipped.length) {
              ctx.ui.notify(`Naming edit skipped: ${skipped.join("; ")}`, "warning");
            } else {
              ctx.ui.notify(`Saved ${edit.target} ${edit.kind}.`, "info");
            }
          } else if (command === "settings") {
            if (rest) {
              const [key, ...valParts] = rest.split(/\s+/);
              const val = valParts.join(" ");
              if (key === "model" && val) {
                const parsed = parseModel(val);
                if (!parsed) throw new Error("Use provider/model-id");
                const updated = await client(ctx).setConfig(
                  "pi.reporting.model",
                  formatModel(parsed)
                );
                configCache = updated;
                settings = reportingSettingsFromSnapshot(updated);
                ctx.ui.notify(`Saved reporting model ${formatModel(parsed)}`, "info");
                return;
              }
            }
            await runSettings(ctx);
          } else if (command === "sync") {
            const outcome = await client(ctx).sync();
            ctx.ui.notify(
              `Catalog sync: ${JSON.stringify(outcome.summary ?? outcome.status ?? "done")}`,
              "info"
            );
          } else if (command === "open") {
            await (native ?? startNative(ctx))?.open();
          } else if (command === "close") {
            await (native ?? startNative(ctx))?.close();
          } else if (command === "capture") {
            await runCapture(ctx, debugDeps);
          } else if (command === "replay") {
            await runReplay(ctx, rest, debugDeps);
          } else {
            ctx.ui.notify(
              `Unknown /streams command: ${command}. Try card, report, reset, attach, discuss, name, title-mode, settings, sync, open, close.`,
              "warning"
            );
          }
        } catch (error) {
          ctx.ui.notify(error instanceof Error ? error.message : String(error), "error");
        }
      },
    });
  };
}

export default createStreamExtension();
