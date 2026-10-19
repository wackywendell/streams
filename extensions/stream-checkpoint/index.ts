import { homedir } from "node:os";
import { join } from "node:path";
import { randomUUID } from "node:crypto";
import { Type } from "typebox";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { StreamCli, receiptText, receiptCanContinue, receiptHasWarning, resultNeedsAttention } from "./streams.ts";
import type { Receipt } from "./streams.ts";
import { Reporter } from "./reporter.ts";
import { infer } from "./agent.ts";
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
export default function registerWorkCard(pi: ExtensionAPI, options: Options = {}): void {
 const agentDir = options.agentDir ?? process.env.PI_CODING_AGENT_DIR ?? join(homedir(), ".pi", "agent");
 let config = options.settings;
 const settings = () => config ??= loadSettings(agentDir);
 const scope = new SessionScope();
 let panelOpen = false;
 let shutdown: Promise<void> | undefined;
 let native: NativeController | undefined;
 const states = new Map<string, CardState>();
 const cli = (ctx: ExtensionContext) => options.cli ?? new StreamCli(ctx.cwd);
 const warning = (text: string) => {scope.context?.ui.notify(`Work card: ${text}`, "warning");};
 const receipt = (value: Receipt) => {
  const text = receiptText(value);
  // Receipts do not enter model context or count as fresh user work.
  pi.appendEntry("stream-catalog-receipt", value);
  scope.context?.ui.notify(text, receiptHasWarning(value) ? "warning" : "info");
  if (receiptCanContinue(value)) native?.update();
 };
 const reporter = new Reporter({scope, cli, settings, infer: options.infer, notice: warning, receipt, activity: (text) => scope.context?.ui.setStatus("stream-reporting", text ? `Work card · ${text} · Alt+P` : undefined)});
 async function sync(ctx: ExtensionContext, event: "start" | "shutdown"): Promise<void> {
  const fence = scope.fence();
  let failure: {event: string; result?: unknown; error?: string} | undefined;
  try {
   const value = await cli(ctx).sync(event);
   if (resultNeedsAttention(value)) failure = {event, result: value};
  } catch (error) {failure = {event, error: String(error)};}
  if (failure && fence.live()) {
   pi.appendEntry("stream-catalog-sync", failure);
   ctx.ui.notify(`Catalog sync (${event}): ${JSON.stringify(failure)}. Inspect with streams sync; local saves and publication are separate.`, "warning");
  }
 }
 const configure = async (ctx: ExtensionContext) => settingsDialog(ctx, settings(), (value) => {saveSettings(agentDir, value);config = value;});
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
     const response = await (options.infer ?? infer)(ctx, {type: "discussion", catalog, messages: evidence.messages, discussion: structuredClone(prospective)}, signal, settings().model);
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
    });
   } catch(error) {if (fence.current()) {state.discussion.error = String(error);warning(String(error));}}
   if (!fence.live() || scope.stopped) return;
  }
 }
 async function show(ctx: ExtensionContext): Promise<void> {
  scope.use(ctx);
  if (!ctx.hasUI || ctx.mode !== "tui") {warning("Alt+P requires the interactive Pi terminal");return;}
  if (panelOpen) return;
  panelOpen = true;
  try {
   const catalog = await cli(ctx).context(ctx.sessionManager.getSessionId(), process.env.STREAMS_STREAM_ID);
   if (!catalog.stream) {ctx.ui.notify("No work card attached. Use /streams attach to choose or create an effort, or start meaningful work for automatic enrollment.", "info");return;}
   const key = JSON.stringify([ctx.sessionManager.getSessionId(), catalog.catalog_source, catalog.stream.id]);
   let state = states.get(key);if (!state) {state = {discussion: {input: "", messages: []}, edits: new Map()};states.set(key, state);}
   await openCard(ctx, cli(ctx), state, {receipt, discuss: () => discussion(ctx, state!), settings: () => configure(ctx), report: () => reporter.request(ctx)});
  } catch(error) {warning(String(error));} finally {panelOpen = false;}
 }
 async function attach(ctx: ExtensionContext, args: {streamId?: string; title?: string; reason?: string}) {
  scope.use(ctx);
  const before = await cli(ctx).context(ctx.sessionManager.getSessionId(), process.env.STREAMS_STREAM_ID);
  if (before.session) {native?.update();return {status: "already-bound; membership moves/forks are not implemented", stream_id: before.session.stream_id};}
  return reporter.exclusive((signal) => chooseInitial(ctx, cli(ctx), args, receipt, signal));
 }
 pi.registerCommand("streams", {
  description: "Report progress, configure reporting, attach an initial work card, or open/close its native pane",
  getArgumentCompletions: (prefix) => {
   if (/\s/.test(prefix)) return null;
   const matches = ["report", "settings", "attach", "pane"].filter((command) => command.startsWith(prefix));
   return matches.length ? matches.map((command) => ({value: command, label: command})) : null;
  },
  handler: async (args, ctx) => {
   scope.use(ctx);
   const paneFence = scope.fence();
   const input = args.trim(), command = input.split(/\s+/, 1)[0], instructions = input.slice(command.length).trim();
   if (!["", "report", "settings", "attach", "pane"].includes(command) || instructions && command !== "report" && !(command === "pane" && instructions === "close")) {
    warning("Usage: /streams or /streams report [instructions]; /streams settings; /streams attach; /streams pane [close]. Put report instructions after 'report'.");return;
   }
   try {
    if (command === "pane") {
     if (!native) throw Error("Native work cards require session startup in an interactive Herdr pane.");
     if (instructions === "close") await native.close();else await native.open();
    }
    else if (command === "settings") await configure(ctx);
    else if (command === "attach") await attach(ctx, {});
    else {settings();reporter.request(ctx, true, instructions);}
   } catch(error) {
    if (command === "pane" && !paneFence.current()) return;
    warning(command === "pane" ? `${String(error)}. Verify enrollment and a ready Stream workspace; use /streams attach for initial enrollment, or /reload after a native identity change.` : String(error));
   }
  },
 });
 pi.registerShortcut("alt+p", {description: "Inspect, edit or discuss the live work card", handler: show});
 pi.registerTool({name: "stream_read", label: "Read work card", description: "Read typed durable catalog state and recorded contributions from persisted local records. No fetch, scratch notes or pending drafts.", parameters: Type.Object({}), async execute(_id, _args, _signal, _update, ctx) {scope.use(ctx);const {authority, ...catalog} = await cli(ctx).context(ctx.sessionManager.getSessionId(), process.env.STREAMS_STREAM_ID);return result({...catalog, reporting_authority: !!authority});}});
 pi.registerTool({name: "checkpoint", label: "Report work card", description: "Request an immediate durable progress report. Local saves and publication are separate; this is not an approval draft.", parameters: Type.Object({instructions: Type.Optional(Type.String())}), async execute(_id, args, _signal, _update, ctx) {scope.use(ctx);settings();reporter.request(ctx, true, args.instructions);return result({status: "report-requested"});}});
 pi.registerTool({name: "select_stream", label: "Attach initial effort", description: "Ask the user to attach this unbound Pi conversation to an exact catalog stream ID. An enrolled conversation cannot switch; membership moves/forks are not implemented.", parameters: Type.Object({stream_id: Type.Optional(Type.String()), reason: Type.String()}), async execute(_id, args, _signal, _update, ctx) {return result(await attach(ctx, {streamId: args.stream_id, reason: args.reason}));}});
 pi.registerTool({name: "create_stream", label: "Create initial effort", description: "Ask the user to create an initial durable effort and enroll this conversation. Never switches existing membership or launches workspaces.", parameters: Type.Object({title: Type.String(), reason: Type.String()}), async execute(_id, args, _signal, _update, ctx) {return result(await attach(ctx, args));}});
 pi.on("session_start", async (_event, ctx) => {
  native?.stop();shutdown = undefined;scope.reset(ctx);
  const client = cli(ctx) as StreamCli & {captureArrangement?: (streamId: string) => Promise<unknown>};
  native = new NativeController({pi, ctx, current: () => scope.context, factory: options.native, warning,
   capture: client.captureArrangement ? (streamId) => client.captureArrangement!(streamId) : undefined});
  native.start();
  await sync(ctx, "start");
 });
 pi.on("agent_end", (event, ctx) => {
  if (scope.stopped) return;
  scope.use(ctx);
  if (!event.messages.some((message) => message.role === "assistant" && ["stop", "length"].includes(message.stopReason))) return;
  try {reporter.response(ctx);} catch(error) {warning(String(error));}
 });
 pi.on("session_tree", () => {scope.invalidate();native?.invalidate();});
 pi.on("session_shutdown", (_event, ctx) => shutdown ??= (async () => {
  native?.stop();
  scope.reset(ctx, true);
  const fence = scope.fence();
  // Flush already-saved catalog changes only, never infer a final summary.
  await sync(ctx, "shutdown");
  if (fence.live()) scope.context = undefined;
 })());
}
