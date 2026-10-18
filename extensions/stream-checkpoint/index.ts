import { homedir } from "node:os";
import { join } from "node:path";
import { randomUUID } from "node:crypto";
import { Type } from "typebox";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { StreamCli, receiptText } from "./streams.ts";
import type { Receipt } from "./streams.ts";
import { Reporter } from "./reporter.ts";
import { checkpointContext, infer } from "./agent.ts";
import { editRequest } from "./core.ts";
import { loadSettings, saveSettings, settingsDialog } from "./settings.ts";
import type { ReportingSettings } from "./settings.ts";
import { openCard, compose } from "./panel.ts";
import type { CardState } from "./panel.ts";
import { chooseInitial } from "./chooser.ts";

type Options = {agentDir?: string; cli?: StreamCli; infer?: typeof infer; settings?: ReportingSettings};
const result = (details: unknown) => ({content: [{type: "text" as const, text: JSON.stringify(details)}], details});
export default function registerWorkCard(pi: ExtensionAPI, options: Options = {}): void {
 const agentDir = options.agentDir ?? process.env.PI_CODING_AGENT_DIR ?? join(homedir(), ".pi", "agent");
 let config = options.settings;
 const settings = () => config ??= loadSettings(agentDir);
 let current: ExtensionContext | undefined;
 let panelOpen = false;
 let shutdown: Promise<void> | undefined;
 let generation = 0;
 const states = new Map<string, CardState>();
 const cli = (ctx: ExtensionContext) => options.cli ?? new StreamCli(ctx.cwd);
 const warning = (text: string) => {current?.ui.notify(`Work card: ${text}`, "warning");};
 const receipt = (value: Receipt) => {
  const text = receiptText(value);
  // Receipts do not enter model context or count as fresh user work.
  pi.appendEntry("stream-catalog-receipt", value);
  current?.ui.notify(text, value.completion_error || value.save && (!value.save.domain || ["conflict", "superseded"].includes(value.save.domain.outcome)) || value.saves?.some(({save}) => !save.domain || ["conflict", "superseded"].includes(save.domain.outcome)) ? "warning" : "info");
 };
 const reporter = new Reporter({cli, settings, infer: options.infer, notice: warning, receipt, activity: (text) => current?.ui.setStatus("stream-reporting", text ? `Work card · ${text} · Alt+P` : undefined)});
 async function sync(ctx: ExtensionContext, event: "start" | "shutdown"): Promise<void> {
  const epoch = generation;
  let failure: {event: string; result?: unknown; error?: string} | undefined;
  try {
   const value = await cli(ctx).sync(event);
   if (["failed", "blocked", "uncertain"].includes(value.sharing) || value.local === "repair-required") failure = {event, result: value};
  } catch (error) {failure = {event, error: String(error)};}
  if (failure && epoch === generation) {
   pi.appendEntry("stream-catalog-sync", failure);
   ctx.ui.notify(`Catalog sync (${event}): ${JSON.stringify(failure)}. Inspect with streams sync; local saves and publication are separate.`, "warning");
  }
 }
 const configure = async (ctx: ExtensionContext) => settingsDialog(ctx, settings(), (value) => {saveSettings(agentDir, value);config = value;});
 async function discussion(ctx: ExtensionContext, state: CardState): Promise<void> {
  for (;;) {
   const input = await compose(ctx, state.discussion);if (input === undefined) return;
   try {
    await reporter.exclusive(async (signal) => {
     const native = ctx.sessionManager.getSessionId();
     const evidence = checkpointContext(ctx.sessionManager, settings().context);
     const source = process.env.STREAMS_STREAM_ID;
     const catalog = structuredClone(await cli(ctx).context(native, source));
     const operation = randomUUID();
     const prospective = structuredClone([...state.discussion.messages, {role: "user" as const, text: input}]);
     const valid = () => !signal.aborted && ctx.sessionManager.getSessionId() === native && process.env.STREAMS_STREAM_ID === source && (evidence.leafId === null ? ctx.sessionManager.getLeafId() === null : ctx.sessionManager.getBranch().some((entry) => entry.id === evidence.leafId));
     const response = await (options.infer ?? infer)(ctx, {type: "discussion", catalog, messages: evidence.messages, discussion: structuredClone(prospective)}, signal, settings().model);
     if (!valid()) throw Error("Discussion result cancelled because the active conversation changed");
     if (typeof response.reply !== "string") throw Error("Discussion model did not return a typed reply");
     const edit = response.edit;
     const request = edit ? editRequest(catalog, edit.kind, edit.changes, edit.actor, operation, true) : undefined;
     const now = await cli(ctx).context(native, source);
     if (now.catalog_source !== catalog.catalog_source || now.stream?.id !== catalog.stream?.id || now.session?.id !== catalog.session?.id || !valid()) throw Error("Card attachment or active conversation changed; discussion input retained and no edits saved");
     state.discussion.messages = [...prospective, {role: "assistant", text: response.reply}];state.discussion.input = "";
     if (request) receipt(await cli(ctx).apply(request));
     state.discussion.error = undefined;
    });
   } catch(error) {state.discussion.error = String(error);warning(String(error));}
  }
 }
 async function show(ctx: ExtensionContext): Promise<void> {
  current = ctx;
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
  current = ctx;
  const before = await cli(ctx).context(ctx.sessionManager.getSessionId(), process.env.STREAMS_STREAM_ID);
  if (before.session) return {status: "already-bound; membership moves/forks are not implemented", stream_id: before.session.stream_id};
  return reporter.exclusive((signal) => chooseInitial(ctx, cli(ctx), args, receipt, signal));
 }
 pi.registerCommand("streams", {
  description: "Report progress now, configure reporting, or attach an initial work card",
  getArgumentCompletions: (prefix) => {
   if (/\s/.test(prefix)) return null;
   const matches = ["report", "settings", "attach"].filter((command) => command.startsWith(prefix));
   return matches.length ? matches.map((command) => ({value: command, label: command})) : null;
  },
  handler: async (args, ctx) => {
   current = ctx;
   const input = args.trim(), command = input.split(/\s+/, 1)[0], instructions = input.slice(command.length).trim();
   if (!["", "report", "settings", "attach"].includes(command) || instructions && command !== "report") {
    warning("Usage: /streams or /streams report [instructions]; /streams settings; /streams attach. Put report instructions after 'report'.");return;
   }
   try {
    if (command === "settings") await configure(ctx);
    else if (command === "attach") await attach(ctx, {});
    else {settings();reporter.request(ctx, true, instructions);}
   } catch(error) {warning(String(error));}
  },
 });
 pi.registerShortcut("alt+p", {description: "Inspect, edit or discuss the live work card", handler: show});
 pi.registerTool({name: "stream_read", label: "Read work card", description: "Read typed durable catalog state and recorded contributions from persisted local records. No fetch, scratch notes or pending drafts.", parameters: Type.Object({}), async execute(_id, _args, _signal, _update, ctx) {current = ctx;const {authority, ...catalog} = await cli(ctx).context(ctx.sessionManager.getSessionId(), process.env.STREAMS_STREAM_ID);return result({...catalog, reporting_authority: !!authority});}});
 pi.registerTool({name: "checkpoint", label: "Report work card", description: "Request an immediate durable progress report. Local saves and publication are separate; this is not an approval draft.", parameters: Type.Object({instructions: Type.Optional(Type.String())}), async execute(_id, args, _signal, _update, ctx) {current = ctx;settings();reporter.request(ctx, true, args.instructions);return result({status: "report-requested"});}});
 pi.registerTool({name: "select_stream", label: "Attach initial effort", description: "Ask the user to attach this unbound Pi conversation to an exact catalog stream ID. An enrolled conversation cannot switch; membership moves/forks are not implemented.", parameters: Type.Object({stream_id: Type.Optional(Type.String()), reason: Type.String()}), async execute(_id, args, _signal, _update, ctx) {return result(await attach(ctx, {streamId: args.stream_id, reason: args.reason}));}});
 pi.registerTool({name: "create_stream", label: "Create initial effort", description: "Ask the user to create an initial durable effort and enroll this conversation. Never switches existing membership or launches workspaces.", parameters: Type.Object({title: Type.String(), reason: Type.String()}), async execute(_id, args, _signal, _update, ctx) {return result(await attach(ctx, args));}});
 pi.on("session_start", async (_event, ctx) => {generation++;shutdown = undefined;current = ctx;reporter.invalidate();await sync(ctx, "start");});
 pi.on("agent_end", (event, ctx) => {
  if (shutdown) return;
  current = ctx;
  if (!event.messages.some((message) => message.role === "assistant" && ["stop", "length"].includes(message.stopReason))) return;
  try {reporter.response(ctx);} catch(error) {warning(String(error));}
 });
 pi.on("session_tree", () => {reporter.invalidate();});
 pi.on("session_shutdown", (_event, ctx) => shutdown ??= (async () => {
  const epoch = generation;
  reporter.invalidate();
  // Flush already-saved catalog changes only, never infer a final summary.
  await sync(ctx, "shutdown");
  if (epoch === generation) current = undefined;
 })());
}
