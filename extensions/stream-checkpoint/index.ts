import { homedir } from "node:os";
import { join } from "node:path";
import { randomUUID } from "node:crypto";
import { Type } from "typebox";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { StreamCli, receiptText, receiptCanContinue, receiptHasWarning, resultNeedsAttention } from "./streams.ts";
import type { Receipt } from "./streams.ts";
import { Reporter } from "./reporter.ts";
import { infer } from "./agent.ts";
import { debugCommand } from "./debug.ts";
import { executeLab } from "./lab.ts";
import { registerLabTools } from "./lab-tools.ts";
import { SessionScope, captureInputs } from "./session-scope.ts";
import { editRequest } from "./core.ts";
import { loadSettings, saveSettings, settingsDialog } from "./settings.ts";
import type { ReportingSettings } from "./settings.ts";
import { openCard, compose } from "./panel.ts";
import type { CardState } from "./panel.ts";
import { chooseInitial } from "./chooser.ts";

import { NativeController } from "./native.ts";
import type { NativeFactory } from "./native.ts";

type Options = {agentDir?: string; cli?: StreamCli; infer?: typeof infer; settings?: ReportingSettings; native?: NativeFactory};
const result = (details: unknown) => ({content: [{type: "text" as const, text: JSON.stringify(details)}], details});
function parseResetArgs(instructions: string): {auto: boolean; target: "all" | "session" | "stream"} | null {
 if (!instructions) return {auto: false, target: "all"};
 let auto = false, target: "all" | "session" | "stream" | undefined;
 for (const token of instructions.split(/\s+/)) {
  if (token === "--auto" && !auto) auto = true;
  else if ((token === "all" || token === "session" || token === "stream") && !target) target = token;
  else return null;
 }
 return {auto, target: target ?? "all"};
}
export default function registerWorkCard(pi: ExtensionAPI, options: Options = {}): void {
 const agentDir = options.agentDir ?? process.env.PI_CODING_AGENT_DIR ?? join(homedir(), ".pi", "agent");
 let config = options.settings;
 const settings = () => config ??= loadSettings(agentDir);
 const logging = () => settings().session_logging ? {directory: join(agentDir, "stream-reporting-sessions"), onCreate: (value: {session_id: string; session_file: string}) => pi.appendEntry("stream-helper-session", value)} : undefined;
 const scope = new SessionScope();
 let panelOpen = false;
 let shutdown: Promise<void> | undefined;
 let native: NativeController | undefined;
 let nativeRetirement: Promise<void> | undefined;
 let statusTimer: ReturnType<typeof setTimeout> | undefined;
 const clearStatusTimer = () => {if (statusTimer) {clearTimeout(statusTimer);statusTimer = undefined;}};
 function retireNative(): Promise<void> | undefined {
  const previous = native;native = undefined;
  if (previous) {
   previous.stop();
   nativeRetirement = Promise.all([nativeRetirement, previous.idle()]).then(() => {});
  }
  return nativeRetirement;
 }
 const states = new Map<string, CardState>();
 const cli = (ctx: ExtensionContext) => options.cli ?? new StreamCli(ctx.cwd);
 const warning = (error: unknown) => {scope.context?.ui.notify(`Streams — ${error instanceof Error ? error.message : String(error)}`, "warning");};
 const receipt = (value: Receipt, deferNative = false, quiet = false) => {
  const text = receiptText(value), hasWarning = receiptHasWarning(value);
  // Receipts do not enter model context or count as fresh user work.
  pi.appendEntry("stream-catalog-receipt", value);
  if (!quiet || hasWarning) scope.context?.ui.notify(`Streams — ${text}`, hasWarning ? "warning" : "info");
  if (!deferNative && receiptCanContinue(value)) native?.update();
 };
 const showDoneStatus = (label = "Updated") => {
  clearStatusTimer();
  const check = (scope.context?.ui as any)?.theme?.fg?.("success", "✓") ?? "\x1b[32m✓\x1b[0m";
  scope.context?.ui.setStatus("stream-reporting", `Streams · ${check} ${label}`);
  statusTimer = setTimeout(() => {statusTimer = undefined;scope.context?.ui.setStatus("stream-reporting", undefined);}, 4000);
  statusTimer.unref?.();
 };
 const reporter = new Reporter({
  scope, cli, settings, logging, infer: options.infer, notice: warning, receipt,
  onUpdateNative: () => native?.update(),
  onReclaimNames: (target) => native?.reclaimNames(target).catch(() => {}),
  activity: (text, completed) => {
   clearStatusTimer();
   if (text) scope.context?.ui.setStatus("stream-reporting", `Streams · ${text}`);
   else if (completed) showDoneStatus("Updated");
   else scope.context?.ui.setStatus("stream-reporting", undefined);
  },
 });
 async function sync(ctx: ExtensionContext, event: "start" | "shutdown"): Promise<void> {
  const fence = scope.fence();
  let failure: {event: string; result?: unknown; error?: string} | undefined;
  let message: string | undefined;
  try {
   const value = await cli(ctx).sync(event);
   if (resultNeedsAttention(value)) failure = {event, result: value};
  } catch (error) {failure = {event, error: String(error)};message = error instanceof Error ? error.message : String(error);}
  if (failure && fence.live()) {
   pi.appendEntry("stream-catalog-sync", failure);
   ctx.ui.notify(`Streams — Catalog sync (${event}): ${message ?? JSON.stringify(failure)}. Inspect with streams sync; local saves and publication are separate.`, "warning");
  }
 }
 const configure = async (ctx: ExtensionContext) => settingsDialog(ctx, settings(), (value) => {saveSettings(agentDir, value);config = value;}, agentDir);
 async function discussion(ctx: ExtensionContext, state: CardState): Promise<void> {
  for (;;) {
   const fence = scope.fence();
   const input = await compose(ctx, state.discussion);if (input === undefined || !fence.current()) return;
   try {
    await reporter.exclusive(async (signal) => {
     const client = cli(ctx);
     const capture = await captureInputs(ctx, client, settings().context, fence);
     if (!capture.valid()) return;
     const {catalog, evidence} = capture;
     const operation = randomUUID();
     const prospective = structuredClone([...state.discussion.messages, {role: "user" as const, text: input}]);
     clearStatusTimer();
     ctx.ui.setStatus?.("stream-reporting", "Streams · ⟳ Thinking...");
     let response;
     try {
      response = await (options.infer ?? infer)(ctx, {type: "discussion", catalog, messages: evidence.messages, discussion: structuredClone(prospective)}, signal, settings().model, logging());
     } finally {
      if (fence.live()) ctx.ui.setStatus?.("stream-reporting", undefined);
     }
     if (!capture.valid()) return;
     if (typeof response.reply !== "string") throw Error("Discussion model did not return a typed reply");
     const edit = response.edit;
     const request = edit ? editRequest(catalog, edit.kind, edit.changes, edit.actor, operation, true) : undefined;
     if (!await capture.attached()) return;
     if (request) {
      const value = await client.apply(request);
      if (fence.live()) receipt(value);
      if (!capture.valid()) return;
      if (!receiptCanContinue(value)) throw Error("Discussion edit did not complete; input and save receipt retained. Inspect save and publication outcomes before another write.");
     }
     state.discussion.messages = [...prospective, {role: "assistant", text: response.reply}];state.discussion.input = "";
     state.discussion.error = undefined;
     if (fence.live()) showDoneStatus(request ? "Updated" : "Replied");
    });
   } catch(error) {if (fence.current()) {state.discussion.error = String(error);warning(error);}}
   if (!fence.live() || scope.stopped) return;
  }
 }
 async function show(ctx: ExtensionContext): Promise<void> {
  scope.use(ctx);
  if (!ctx.hasUI || ctx.mode !== "tui") {warning("The work card panel requires the interactive Pi terminal");return;}
  if (panelOpen) return;
  panelOpen = true;
  try {
   const catalog = await cli(ctx).context(ctx.sessionManager.getSessionId(), process.env.STREAMS_STREAM_ID);
   if (!catalog.stream) {ctx.ui.notify("Streams — No work card attached. Use /streams attach to choose or create an effort, or start meaningful work for automatic enrollment.", "info");return;}
   const key = JSON.stringify([ctx.sessionManager.getSessionId(), catalog.catalog_source, catalog.stream.id]);
   let state = states.get(key);if (!state) {state = {discussion: {input: "", messages: []}, edits: new Map()};states.set(key, state);}
   await openCard(ctx, cli(ctx), state, {receipt, discuss: () => discussion(ctx, state!), settings: () => configure(ctx), report: () => reporter.request(ctx), reset: (auto, target) => reporter.reset(ctx, auto, target)});
  } catch(error) {warning(error);} finally {panelOpen = false;}
 }
 async function attach(ctx: ExtensionContext, args: {streamId?: string; title?: string; reason?: string}) {
  scope.use(ctx);
  const before = await cli(ctx).context(ctx.sessionManager.getSessionId(), process.env.STREAMS_STREAM_ID);
  if (before.session) {native?.update();return {status: "already-bound; membership moves/forks are not implemented", stream_id: before.session.stream_id};}
  return reporter.exclusive((signal) => chooseInitial(ctx, cli(ctx), args, receipt, signal));
 }
 pi.registerCommand("streams", {
  description: "Open the work card, report or reset progress, configure reporting, attach a card, or open/close its native pane",
  getArgumentCompletions: (prefix) => {
   if (prefix === "debug" || prefix.startsWith("debug ")) {
    if (!settings().debug_commands) return null;
    const rest = prefix.slice(5).trimStart();
    const children = [{value: "debug capture", label: "capture", description: "Freeze a stream or workspace"}, {value: "debug replay", label: "replay", description: "Replay frozen samples without live writes"}];
    if (!/\s/.test(rest)) {
     const matches = children.filter(item => item.label.startsWith(rest));
     return matches.length ? matches : null;
    }
    const words = rest.split(/\s+/), child = words.shift();
    const partial = words.pop() ?? "", base = `debug ${child} ${words.length ? words.join(" ") + " " : ""}`;
    const choices = (items: {label: string; description: string}[]) => {
     const matches = items.filter(item => item.label.startsWith(partial)).map(item => ({...item, value: base + item.label}));
     return matches.length ? matches : null;
    };
    if (child === "capture") return words.length ? null : choices([{label: "stream", description: "Capture a stream by ID"}, {label: "workspace", description: "Capture a workspace by ID"}]);
    if (child !== "replay") return null;
    if (words[0] && !words[0].startsWith("--")) words.shift(); // A completed capture ID is positional, never a flag prefix.
    const used = new Set<string>();
    for (let i = 0; i < words.length; i++) {
     const flag = words[i];
     if (!["--model", "--reset", "--auto"].includes(flag) || used.has(flag)) return null;
     used.add(flag);
     if (flag === "--model") {
      if (i === words.length - 1) return choices((scope.context?.modelRegistry?.getAvailable() ?? []).map(model => ({label: `${model.provider}/${model.id}`, description: "Replay with this available model"})));
      if (words[++i].startsWith("--")) return null;
     }
     if (flag === "--reset" && i === words.length - 1 && !partial.startsWith("-")) return choices([{label: "session", description: "Reset session fields"}, {label: "stream", description: "Reset effort fields"}, {label: "all", description: "Reset both session and effort fields"}]);
     if (flag === "--reset" && ["session", "stream", "all"].includes(words[i + 1])) i++;
    }
    if (partial && !partial.startsWith("-")) return null;
    return choices([
     {label: "--model", description: "Choose an available replay model"},
     {label: "--reset", description: "Reset session, stream or all fields (default all)"},
     ...(used.has("--reset") ? [{label: "--auto", description: "Re-enable automatic naming for reset"}] : []),
    ].filter(item => !used.has(item.label)));
   }
   if (prefix.startsWith("reset ")) {
    const rest = prefix.slice(6);
    const resetItems = [
     {value: "reset --auto", label: "--auto", description: "Re-enable automatic naming and reset"},
     {value: "reset session", label: "session", description: "Reset only this session's fields"},
     {value: "reset session --auto", label: "session --auto", description: "Re-enable session auto-naming & reset"},
     {value: "reset stream", label: "stream", description: "Reset only the effort's fields"},
     {value: "reset stream --auto", label: "stream --auto", description: "Re-enable effort auto-naming & reset"},
     {value: "reset all", label: "all", description: "Reset both session and effort fields"},
     {value: "reset all --auto", label: "all --auto", description: "Re-enable all auto-naming and reset"},
    ].filter((item) => item.label.startsWith(rest));
    return resetItems.length ? resetItems : null;
   }
   if (/\s/.test(prefix)) return null;
   const matches = [
    {value: "card", label: "card", description: "Open the work card panel"},
    {value: "report", label: "report", description: "Save a progress update"},
    {value: "reset", label: "reset", description: "Regenerate automatic fields"},
    {value: "settings", label: "settings", description: "Configure reporting"},
    {value: "attach", label: "attach", description: "Attach an initial work card"},
    {value: "pane", label: "pane", description: "Open native pane; 'pane close' hides it"},
    ...(settings().debug_commands ? [{value: "debug", label: "debug", description: "Capture and replay private prompt samples"}] : []),
   ].filter((item) => item.value.startsWith(prefix));
   return matches.length ? matches : null;
  },
  handler: async (args, ctx) => {
   scope.use(ctx);
   const paneFence = scope.fence();
   const input = args.trim(), command = input.split(/\s+/, 1)[0], instructions = input.slice(command.length).trim();
   const resetParsed = command === "reset" ? parseResetArgs(instructions) : null;
   if (!["", "card", "report", "reset", "settings", "attach", "pane", "debug"].includes(command) || instructions && command !== "report" && command !== "debug" && !(command === "pane" && instructions === "close") && !(command === "reset" && resetParsed)) {
    warning("Usage: /streams [card]; /streams report [instructions]; /streams reset [session|stream|all] [--auto]; /streams settings; /streams attach; /streams pane [close]. Put report instructions after 'report'.");return;
   }
   try {
    if (command === "" || command === "card") await show(ctx);
    else if (command === "debug") {
     if (!settings().debug_commands) throw Error("Debug commands are disabled. Enable them in /streams settings → Debug → Debug commands.");
     if (!instructions) {ctx.ui.notify("Streams — Debug commands\n/streams debug capture — freeze a stream or workspace\n/streams debug replay — choose a capture and model\nReplay options: --model PROVIDER/ID; --reset [session|stream|all]; --auto (with --reset).", "info");return;}
     await reporter.exclusive(signal => debugCommand(instructions, ctx, cli(ctx), agentDir, settings(), signal));
    }
    else if (command === "pane") {
     if (!native) throw Error("Native work cards require session startup in an interactive Herdr pane.");
     clearStatusTimer();
     ctx.ui.setStatus?.("stream-reporting", instructions === "close" ? "Streams · ⟳ Closing pane..." : "Streams · ⟳ Opening pane...");
     let ok = false;
     try {
      if (instructions === "close") await native.close();else await native.open();
      ok = true;
     } finally {
      if (paneFence.current()) {
       if (ok) showDoneStatus(instructions === "close" ? "Pane closed" : "Pane open");
       else ctx.ui.setStatus?.("stream-reporting", undefined);
      }
     }
    }
    else if (command === "reset") reporter.reset(ctx, resetParsed!.auto, resetParsed!.target);
    else if (command === "settings") await configure(ctx);
    else if (command === "attach") await attach(ctx, {});
    else if (command === "report") {settings();reporter.request(ctx, true, instructions);}
   } catch(error) {
    if ((command === "pane" || command === "debug") && !paneFence.current()) return;
    warning(error);
   }
  },
 });
 registerLabTools(pi, {enabled: () => settings().debug_commands, execute: (operation, signal, ctx) => {
  scope.use(ctx);
  return reporter.exclusive(lifecycle => executeLab(operation, {root: join(agentDir, "stream-debug"), client: cli(ctx), runtime: ctx, current: ctx.sessionManager}, AbortSignal.any([signal, lifecycle])));
 }});
 pi.registerTool({name: "stream_read", label: "Read work card", description: "Read typed durable catalog state and recorded contributions from persisted local records. No fetch, scratch notes or pending drafts.", parameters: Type.Object({}), async execute(_id, _args, _signal, _update, ctx) {scope.use(ctx);const {authority, ...catalog} = await cli(ctx).context(ctx.sessionManager.getSessionId(), process.env.STREAMS_STREAM_ID);return result({...catalog, reporting_authority: !!authority});}});
 pi.registerTool({name: "checkpoint", label: "Report work card", description: "Request an immediate durable progress report. Local saves and publication are separate; this is not an approval draft.", parameters: Type.Object({instructions: Type.Optional(Type.String())}), async execute(_id, args, _signal, _update, ctx) {scope.use(ctx);settings();reporter.request(ctx, true, args.instructions);return result({status: "report-requested"});}});
 pi.registerTool({name: "select_stream", label: "Attach initial effort", description: "Ask the user to attach this unbound Pi conversation to an exact catalog stream ID. An enrolled conversation cannot switch; membership moves/forks are not implemented.", parameters: Type.Object({stream_id: Type.Optional(Type.String()), reason: Type.String()}), async execute(_id, args, _signal, _update, ctx) {return result(await attach(ctx, {streamId: args.stream_id, reason: args.reason}));}});
 pi.registerTool({name: "create_stream", label: "Create initial effort", description: "Ask the user to create an initial durable effort and enroll this conversation. Never switches existing membership or launches workspaces.", parameters: Type.Object({title: Type.String(), reason: Type.String()}), async execute(_id, args, _signal, _update, ctx) {return result(await attach(ctx, args));}});
 pi.on("session_start", async (_event, ctx) => {
  clearStatusTimer();ctx.ui.setStatus?.("stream-debug", undefined);
  const retired = retireNative();shutdown = undefined;scope.reset(ctx);
  const fence = scope.fence();
  if (retired) await retired;
  if (!fence.current()) return;
  const client = cli(ctx) as StreamCli & {captureArrangement?: (streamId: string, signal?: AbortSignal) => Promise<unknown>};
  native = new NativeController({pi, ctx, current: () => scope.context, factory: options.native, warning,
   capture: client.captureArrangement ? (streamId, signal) => client.captureArrangement!(streamId, signal) : undefined});
  native.start();
  await sync(ctx, "start");
 });
 pi.on("agent_end", (event, ctx) => {
  if (scope.stopped) return;
  scope.use(ctx);
  if (!event.messages.some((message) => message.role === "assistant" && ["stop", "length"].includes(message.stopReason))) return;
  try {reporter.response(ctx);} catch(error) {warning(error);}
 });
 pi.on("session_tree", (_event, ctx) => {clearStatusTimer();ctx.ui.setStatus?.("stream-debug", undefined);scope.invalidate();native?.invalidate();});
 pi.on("session_shutdown", (_event, ctx) => shutdown ??= (async () => {
  clearStatusTimer();ctx.ui.setStatus?.("stream-debug", undefined);
  const retired = retireNative();
  scope.reset(ctx, true);
  const fence = scope.fence();
  // Flush already-saved catalog changes only, never infer a final summary.
  await Promise.all([sync(ctx, "shutdown"), retired]);
  if (fence.live()) scope.context = undefined;
 })());
}
