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
import { registerArtifactTool } from "./artifacts.ts";
import { SessionScope, captureInputs, hasConversation } from "./session-scope.ts";
import { editRequest, ReferenceEdit, referenceRequest, referenceSelector } from "./core.ts";
import { namingCompletions, parseNamingEdit, saveNamingEdit, savePiNameEdit } from "./naming.ts";
import { defaultConfigSnapshot, reportingSettingsFromSnapshot, settingsDialog } from "./settings.ts";
import type { ReportingSettings } from "./settings.ts";
import { openCard, compose } from "./panel.ts";
import type { CardState } from "./panel.ts";
import { chooseInitial } from "./chooser.ts";

import { NativeController } from "./native.ts";
import { StreamsChild } from "./child.ts";
import type { NativeFactory } from "./native.ts";

type PiContextState = {
 conversation: {harness: "pi"; native_id: string}; generation: number; session_file: string | null;
 helper?: StreamsChild; ready: Promise<void>; hydrated: boolean; hydrate: Promise<void>; retired: boolean; retire(): Promise<void>;
};
export type PiContextHolder = {generation: number; current?: PiContextState; retirement?: Promise<void>};
const contextKey = Symbol.for("streams.pi.context");
const processContext = ((globalThis as typeof globalThis & {[contextKey]?: PiContextHolder})[contextKey] ??= {generation:0});
type Options = {agentDir?: string; cli?: StreamCli; infer?: typeof infer; settings?: ReportingSettings; native?: NativeFactory; child?: (ctx: ExtensionContext) => StreamsChild; contextState?: PiContextHolder};
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
const shouldStartStreams = (ctx: ExtensionContext): boolean =>
 ctx.mode !== "print" &&
 ctx.mode !== "json" &&
 (typeof ctx.sessionManager.getSessionFile !== "function" || ctx.sessionManager.getSessionFile() !== undefined);

const isInteractiveTui = (ctx: ExtensionContext): boolean =>
 ctx.hasUI === true &&
 ctx.mode === "tui" &&
 shouldStartStreams(ctx) &&
 (typeof ctx.sessionManager.buildSessionProjection !== "function" || hasConversation(ctx.sessionManager));

export default function registerWorkCard(pi: ExtensionAPI, options: Options = {}): void {
 const agentDir = options.agentDir ?? process.env.PI_CODING_AGENT_DIR ?? join(homedir(), ".pi", "agent");
 let snapshot = defaultConfigSnapshot();
 if (options.settings) {
  snapshot.pi.reporting.mode = options.settings.reporting;
  snapshot.pi.reporting.enrollment = options.settings.enrollment;
  snapshot.pi.reporting.frequency = options.settings.frequency;
  snapshot.pi.reporting.context = options.settings.context;
  snapshot.pi.reporting.model = options.settings.model ?? null;
  snapshot.pi.reporting.session_logging = options.settings.session_logging;
  snapshot.pi.reporting.debug_commands = options.settings.debug_commands;
  snapshot.pi.naming.import_edits = options.settings.pi_name_edits;
 }
 const settings = (): ReportingSettings => reportingSettingsFromSnapshot(snapshot);
 const logging = () => settings().session_logging ? {directory: join(agentDir, "stream-reporting-sessions"), onCreate: (value: {session_id: string; session_file: string}) => pi.appendEntry("stream-helper-session", value)} : undefined;
 const scope = new SessionScope();
 let panelOpen = false;
 let shutdown: Promise<void> | undefined;
 let native: NativeController | undefined;
 const holder: PiContextHolder = options.contextState ?? (options.cli || options.child || options.native ? {generation:0} : processContext);
 let owned: PiContextState | undefined;
 let statusTimer: ReturnType<typeof setTimeout> | undefined;
 const clearStatusTimer = () => {if (statusTimer) {clearTimeout(statusTimer);statusTimer = undefined;}};
 const states = new Map<string, CardState>();
 const warning = (error: unknown) => {scope.context?.ui.notify(`Streams — ${error instanceof Error ? error.message : String(error)}`, "warning");};
 const current = (state: PiContextState, ctx: ExtensionContext) => holder.current === state && !state.retired && ctx.sessionManager.getSessionId() === state.conversation.native_id;
 const sessionChild = (ctx: ExtensionContext, state: PiContextState) => state.helper ??= options.child?.(ctx) ?? new StreamsChild({sessionId:state.conversation.native_id, cwd:ctx.cwd, env:{...process.env},
  piContext: ctx.hasUI && ctx.mode === "tui" && process.env.HC_TERMINAL_ID ? () => ({conversation:state.conversation, session_file:state.session_file}) : undefined,
  warning:message => {if (current(state, ctx) || state.retired && holder.current === state && ctx.sessionManager.getSessionId() === state.conversation.native_id) warning(message);}});
 function useContext(ctx: ExtensionContext, restart = false): PiContextState {
  const id = ctx.sessionManager.getSessionId(), file = ctx.sessionManager.getSessionFile?.() ?? null;
  if (owned && (holder.current && holder.current !== owned || !restart && owned.retired && id === owned.conversation.native_id)) throw Error("Streams extension retired; Session changed");
  if (!restart && owned && current(owned, ctx)) {
   owned.session_file = file;scope.use(ctx);return owned;
  }
  const previous = holder.current;
  if (owned && owned.conversation.native_id !== id) states.clear();
  let retirement: Promise<void> | undefined;
  const state: PiContextState = {conversation:{harness:"pi",native_id:id}, generation:++holder.generation, session_file:file, ready:Promise.resolve(), hydrated:Boolean(options.settings || options.cli), hydrate:Promise.resolve(), retired:false,
   retire: () => {
    if (retirement) return retirement;
    state.retired = true;
    const controller = native;native = undefined;
    controller?.stop();clearStatusTimer();panelOpen = false;scope.reset(scope.context ?? ctx, true);
    return retirement = (async () => {
     try {await controller?.idle();} finally {await state.helper?.stop();state.helper = undefined;}
    })();
   },
  };
  holder.current = state;owned = state;
  holder.retirement = Promise.all([holder.retirement, previous?.retire()]).then(() => {});
  scope.reset(ctx);shutdown = undefined;
  state.ready = holder.retirement.then(async () => {
   if (!current(state, ctx)) return;
   native = new NativeController({pi, ctx, current: () => current(state, ctx) ? scope.context : undefined, factory:options.native, warning,
    child:options.cli ? undefined : sessionChild(ctx, state)});
   try {await native.start();} catch(error) {if (current(state, ctx)) warning(error);}
  });
  void state.ready.catch(() => {});
  state.hydrate = state.hydrated
   ? Promise.resolve()
   : state.ready.then(async () => {
      if (!current(state, ctx)) return;
      try {
       const fresh = await clientFor(ctx, state).getConfig();
       if (current(state, ctx)) snapshot = fresh;
      } catch {}
      state.hydrated = true;
     });
  void state.hydrate.catch(() => {});
  return state;
 }
 const clientFor = (ctx: ExtensionContext, state: PiContextState) => {
  if (options.cli) return options.cli;
  const fence = scope.fence();
  return new StreamCli(ctx.cwd, async (request, options) => {
   if (!current(state, ctx)) throw Error("Session changed before Streams request");
   await state.ready;
   if (!current(state, ctx)) throw Error("Session changed before Streams request");
   if (request.kind === "context" || request.kind === "enroll") {
    if (!fence.current()) throw Error("Session changed before catalog context");
    await native?.prepareCatalog();
    if (!fence.current()) throw Error("Session changed before catalog context");
   }
   const outcome = await sessionChild(ctx, state).run(request, options);
   if (!current(state, ctx)) throw Error("Session changed before Streams result");
   return outcome;
  });
 };
 const cli = (ctx: ExtensionContext) => clientFor(ctx, useContext(ctx));
 const receipt = (value: Receipt, quiet = false) => {
  const text = receiptText(value), hasWarning = receiptHasWarning(value);
  // Receipts do not enter model context or count as fresh user work.
  pi.appendEntry("stream-catalog-receipt", value);
  if (!quiet || hasWarning) scope.context?.ui.notify(`Streams — ${text}`, hasWarning ? "warning" : "info");
 };
 const showDoneStatus = (label = "Updated") => {
  clearStatusTimer();
  const check = (scope.context?.ui as any)?.theme?.fg?.("success", "✓") ?? "\x1b[32m✓\x1b[0m";
  scope.context?.ui.setStatus("stream-reporting", `Streams · ${check} ${label}`);
  statusTimer = setTimeout(() => {statusTimer = undefined;scope.context?.ui.setStatus("stream-reporting", undefined);}, 4000);
  statusTimer.unref?.();
 };
 const reporter = new Reporter({
  scope, cli, settings, logging, prepare: () => native?.prepareCatalog(), infer: options.infer,
  notice: (text, level) => scope.context?.ui.notify(`Streams — ${text}`, level ?? "warning"), receipt,
  activity: (text, completed) => {
   clearStatusTimer();
   if (text) scope.context?.ui.setStatus("stream-reporting", `Streams · ${text}`);
   else if (completed) showDoneStatus("Updated");
   else scope.context?.ui.setStatus("stream-reporting", undefined);
  },
 });
 async function sync(ctx: ExtensionContext, event: "start" | "shutdown", state: PiContextState): Promise<void> {
  const fence = scope.fence();
  let failure: {event: string; result?: unknown; error?: string} | undefined;
  let message: string | undefined;
  try {
   const value = await clientFor(ctx, state).sync();
   if (resultNeedsAttention(value)) failure = {event, result: value};
  } catch (error) {failure = {event, error: String(error)};message = error instanceof Error ? error.message : String(error);}
  if (failure && fence.live()) {
   pi.appendEntry("stream-catalog-sync", failure);
   ctx.ui.notify(`Streams — Catalog sync (${event}): ${message ?? JSON.stringify(failure)}. Inspect with streams sync; local saves and publication are separate.`, "warning");
  }
 }
 const configure = async (ctx: ExtensionContext) => {
  const client = cli(ctx);
  snapshot = await client.getConfig();
  await settingsDialog(ctx, snapshot, {
   setConfig: async (key, value) => snapshot = await client.setConfig(key, value),
   unsetConfig: async (key) => snapshot = await client.unsetConfig(key),
  });
 };
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
     ctx.ui.setStatus?.("stream-reporting", "Streams · Thinking…");
     let response;
     try {
      response = await (options.infer ?? infer)(ctx, {type: "discussion", catalog, messages: evidence.messages, discussion: structuredClone(prospective)}, signal, settings().model, logging());
     } finally {
      if (fence.live()) ctx.ui.setStatus?.("stream-reporting", undefined);
     }
     if (!capture.valid()) return;
     if (typeof response.reply !== "string") throw Error("Discussion model did not return a typed reply");
     const edit = response.edit;
     if (edit?.kind === "references" && state.referenceEdits?.size) throw Error("Finish or discard retained reference edits before applying discussion reference changes.");
     const request = !edit ? undefined : edit.kind === "references" ? referenceRequest(catalog, edit.actor, edit.changes, operation) : editRequest(catalog, edit.kind, edit.changes, edit.actor, operation);
     if (!await capture.attached()) return;
     if (request) {
      const value = await client.apply(request);
      if (fence.live()) {
       receipt(value);
       if (edit?.kind === "references") {
        const saves = value.saves?.map(item => item.save) ?? (value.save ? [value.save] : []);
        for (const save of saves) for (const row of save.domain?.reference_results ?? []) {
         if (row.outcome !== "conflict" && row.outcome !== "superseded") continue;
         const draft = edit.changes.find(item => referenceSelector(item.key) === referenceSelector(row.key));
         if (draft) (state.referenceEdits ??= new Map()).set(referenceSelector(draft.key), new ReferenceEdit(catalog, draft.key, edit.actor, draft.desired));
        }
       }
      }
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
  useContext(ctx);
  const fence = scope.fence();
  if (!ctx.hasUI || ctx.mode !== "tui") {warning("The work card panel requires the interactive Pi terminal");return;}
  if (panelOpen) return;
  panelOpen = true;
  try {
   const catalog = await cli(ctx).context(ctx.sessionManager.getSessionId());
   if (!fence.current()) return;
   if (!catalog.stream) {ctx.ui.notify("Streams — No work card attached. Use /streams attach to choose or create a Stream, or start meaningful work for automatic enrollment.", "info");return;}
   const key = JSON.stringify([ctx.sessionManager.getSessionId(), catalog.catalog_source, catalog.stream.id]);
   let state = states.get(key);if (!state) {state = {discussion: {input: "", messages: []}, edits: new Map()};states.set(key, state);}
   await openCard(ctx, cli(ctx), state, {receipt, discuss: () => discussion(ctx, state!), settings: () => configure(ctx), report: () => reporter.request(ctx), reset: (auto, target) => reporter.reset(ctx, auto, target)});
  } catch(error) {if (fence.live()) warning(error);} finally {if (fence.live()) panelOpen = false;}
 }
 async function attach(ctx: ExtensionContext, args: {streamId?: string; title?: string; reason?: string}) {
  useContext(ctx);
  if (!hasConversation(ctx.sessionManager)) throw Error("Start a conversation before attaching a work card.");
  const before = await cli(ctx).context(ctx.sessionManager.getSessionId());
  if (before.session) {return {status: "already-bound; membership moves/forks are not implemented", stream_id: before.session.stream_id};}
  return reporter.exclusive((signal) => chooseInitial(ctx, cli(ctx), args, receipt, signal));
 }
 pi.registerCommand("streams", {
  description: "Open the Stream and Session panel",
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
     if (flag === "--reset" && i === words.length - 1 && !partial.startsWith("-")) return choices([{label: "session", description: "Reset Session fields"}, {label: "stream", description: "Reset Stream fields"}, {label: "all", description: "Reset Stream and Session fields"}]);
     if (flag === "--reset" && ["session", "stream", "all"].includes(words[i + 1])) i++;
    }
    if (partial && !partial.startsWith("-")) return null;
    return choices([
     {label: "--model", description: "Choose an available replay model"},
     {label: "--reset", description: "Reset session, stream or all fields (default all)"},
     ...(used.has("--reset") ? [{label: "--auto", description: "Re-enable automatic naming for reset"}] : []),
    ].filter(item => !used.has(item.label)));
   }
   if (prefix.startsWith("name ") || prefix.startsWith("title-mode ")) return namingCompletions(prefix);
   if (prefix.startsWith("reset ")) {
    const rest = prefix.slice(6);
    const resetItems = [
     {value: "reset --auto", label: "--auto", description: "Re-enable automatic naming and reset"},
     {value: "reset session", label: "session", description: "Reset only this session's fields"},
     {value: "reset session --auto", label: "session --auto", description: "Re-enable session auto-naming & reset"},
     {value: "reset stream", label: "stream", description: "Reset only the Stream's fields"},
     {value: "reset stream --auto", label: "stream --auto", description: "Re-enable Stream auto-naming & reset"},
     {value: "reset all", label: "all", description: "Reset Stream and Session fields"},
     {value: "reset all --auto", label: "all --auto", description: "Re-enable all auto-naming and reset"},
    ].filter((item) => item.label.startsWith(rest));
    return resetItems.length ? resetItems : null;
   }
   if (prefix.startsWith("pane ")) {
    return "close".startsWith(prefix.slice(5)) ? [{value: "pane close", label: "close", description: "Close the native pane"}] : null;
   }
   if (/\s/.test(prefix)) return null;
   const matches = [
    {value: "card", label: "card", description: "Open the Stream and Session panel"},
    {value: "report", label: "report", description: "Save a progress update"},
    {value: "name", label: "name", description: "Set and protect a saved title"},
    {value: "title-mode", label: "title-mode", description: "Change title generation policy"},
    {value: "reset", label: "reset", description: "Regenerate automatic fields"},
    {value: "settings", label: "settings", description: "Configure reporting"},
    {value: "attach", label: "attach", description: "Attach an initial work card"},
    {value: "pane", label: "pane", description: "Open native pane; 'pane close' hides it"},
    ...(settings().debug_commands ? [{value: "debug", label: "debug", description: "Capture and replay private prompt samples"}] : []),
   ].filter((item) => item.value.startsWith(prefix));
   return matches.length ? matches : null;
  },
  handler: async (args, ctx) => {
   useContext(ctx);
   const paneFence = scope.fence();
   const input = args.trim(), command = input.split(/\s+/, 1)[0], instructions = input.slice(command.length).trim();
   const resetParsed = command === "reset" ? parseResetArgs(instructions) : null;
   if (!["", "card", "report", "name", "title-mode", "reset", "settings", "attach", "pane", "debug"].includes(command) || instructions && !["report", "debug", "name", "title-mode"].includes(command) && !(command === "pane" && instructions === "close") && !(command === "reset" && resetParsed)) {
    warning("Usage: /streams [card]; /streams report [instructions]; /streams name [session|stream] <name>; /streams title-mode [session|stream] automatic|manual|inherit; /streams reset [session|stream|all] [--auto]; /streams settings; /streams attach; /streams pane [close]. Put report instructions after 'report'.");return;
   }
   try {
    if (command === "" || command === "card") await show(ctx);
    else if (command === "debug") {
     if (owned && !owned.hydrated) await owned.hydrate;
     if (!settings().debug_commands) throw Error("Debug commands are disabled. Enable them in /streams settings → Debug → pi.reporting.debug_commands.");
     if (!instructions) {ctx.ui.notify("Streams — Debug commands\n/streams debug capture — freeze a stream or workspace\n/streams debug replay — choose a capture and model\nReplay options: --model PROVIDER/ID; --reset [session|stream|all]; --auto (with --reset).", "info");return;}
     await reporter.exclusive(signal => debugCommand(instructions, ctx, cli(ctx), agentDir, settings(), signal));
    }
    else if (command === "pane") {
     if (!native) throw Error("Native work cards require session startup in an interactive Herdr pane.");
     clearStatusTimer();
     ctx.ui.setStatus?.("stream-reporting", instructions === "close" ? "Streams · Closing…" : "Streams · Opening…");
     let ok = false;
     try {
      if (instructions === "close") await native.close();
      else {
       await native.ready();
       const client = cli(ctx), id = ctx.sessionManager.getSessionId();
       const catalog = await client.context(id);
       if (!paneFence.current()) return;
       if (!catalog.session) {
        if (settings().enrollment !== "automatic") throw Error("Enrollment is manual. Use /streams attach before opening a pane.");
        await reporter.reportNow(ctx);
        if (!paneFence.current()) return;
        const reported = await client.context(id);
        if (!paneFence.current()) return;
        if (!reported.session) throw Error("Reporting did not enroll this Session. Use /streams attach to create or select a work card.");
       }
       await native.open();
      }
      ok = true;
     } finally {
      if (paneFence.current()) {
       if (ok) showDoneStatus(instructions === "close" ? "Pane closed" : "Pane open");
       else ctx.ui.setStatus?.("stream-reporting", undefined);
      }
     }
    }
    else if (command === "name" || command === "title-mode") {
     const edit = parseNamingEdit(command, instructions);
     const value = await saveNamingEdit(cli(ctx), ctx.sessionManager.getSessionId(), edit, () => {
      if (!paneFence.current()) throw Error("Session changed before naming edit");
     });
     if (paneFence.live()) receipt(value);
    }
    else if (command === "reset") reporter.reset(ctx, resetParsed!.auto, resetParsed!.target);
    else if (command === "settings") await configure(ctx);
    else if (command === "attach") await attach(ctx, {});
    else if (command === "report") {settings();reporter.request(ctx, true, instructions);}
   } catch(error) {
    if (["pane", "debug", "name", "title-mode"].includes(command) && !paneFence.current()) return;
    warning(error);
   }
  },
 });
 registerLabTools(pi, {enabled: () => settings().debug_commands, execute: (operation, signal, ctx) => {
  useContext(ctx);
  return reporter.exclusive(lifecycle => executeLab(operation, {root: join(agentDir, "stream-debug"), client: cli(ctx), runtime: ctx, current: ctx.sessionManager, helperRoot: join(agentDir, "stream-reporting-sessions")}, AbortSignal.any([signal, lifecycle])));
 }});
 registerArtifactTool(pi, ctx => {useContext(ctx);return cli(ctx);});
 pi.registerTool({name: "stream_read", label: "Read work card", description: "Read typed durable catalog state and recorded contributions from persisted local records. No fetch, scratch notes or pending drafts.", parameters: Type.Object({}), async execute(_id, _args, _signal, _update, ctx) {useContext(ctx);const catalog = await cli(ctx).context(ctx.sessionManager.getSessionId());return result(catalog);}});
 pi.registerTool({name: "checkpoint", label: "Report work card", description: "Request an immediate durable progress report. Local saves and publication are separate; this is not an approval draft.", parameters: Type.Object({instructions: Type.Optional(Type.String())}), async execute(_id, args, _signal, _update, ctx) {useContext(ctx);settings();reporter.request(ctx, true, args.instructions);return result({status: "report-requested"});}});
 pi.registerTool({name: "select_stream", label: "Attach initial Stream", description: "Ask the user to attach this unbound Pi conversation to an exact catalog stream ID. An enrolled conversation cannot switch; membership moves/forks are not implemented.", parameters: Type.Object({stream_id: Type.Optional(Type.String()), reason: Type.String()}), async execute(_id, args, _signal, _update, ctx) {return result(await attach(ctx, {streamId: args.stream_id, reason: args.reason}));}});
 pi.registerTool({name: "create_stream", label: "Create initial Stream", description: "Ask the user to create an initial durable Stream and enroll this conversation. Never switches existing membership or launches workspaces.", parameters: Type.Object({title: Type.String(), reason: Type.String()}), async execute(_id, args, _signal, _update, ctx) {return result(await attach(ctx, args));}});
 pi.on("session_start", async (_event, ctx) => {
  clearStatusTimer();ctx.ui.setStatus?.("stream-debug", undefined);
  if (!shouldStartStreams(ctx) && !options.cli) return;
  const state = useContext(ctx, true);
  if (!state.hydrated) await state.hydrate;
  else await state.ready;
  if (isInteractiveTui(ctx)) {
   try { await clientFor(ctx, state).lifecycle?.("resume"); } catch(error) { if (current(state, ctx)) warning(error); }
  }
  if (current(state, ctx)) await sync(ctx, "start", state);
 });
 pi.on("session_info_changed", async (event, ctx) => {
  if (!event.name?.trim() || (!shouldStartStreams(ctx) && !options.cli)) return;
  const state = useContext(ctx);
  const fence = scope.fence();
  try {
   if (!state.hydrated) await state.hydrate;
   if (!fence.current() || !settings().pi_name_edits) return;
   const value = await savePiNameEdit(cli(ctx), ctx.sessionManager.getSessionId(), event.name, () => {
    if (!fence.current()) throw Error("Session changed before Pi name edit");
   });
   if (value && fence.live()) receipt(value, true);
  } catch(error) {if (fence.current()) warning(`Pi name changed, but its Streams edit failed: ${String(error)}`);}
 });
 pi.on("agent_end", (event, ctx) => {
  if (owned && holder.current !== owned || !shouldStartStreams(ctx)) return;
  useContext(ctx);
  if (!event.messages.some((message) => message.role === "assistant" && ["stop", "length"].includes(message.stopReason))) return;
  try {reporter.response(ctx);} catch(error) {warning(error);}
 });
 pi.on("session_tree", (_event, ctx) => {if (owned && holder.current !== owned || (!shouldStartStreams(ctx) && !options.cli)) return;useContext(ctx);clearStatusTimer();ctx.ui.setStatus?.("stream-debug", undefined);scope.invalidate();});
 pi.on("session_shutdown", (_event, ctx) => shutdown ??= (async () => {
  const state = owned;
  if (!state || !current(state, ctx)) return;
  clearStatusTimer();ctx.ui.setStatus?.("stream-debug", undefined);
  native?.stop();scope.reset(ctx, true);
  if (isInteractiveTui(ctx)) {
   try { await clientFor(ctx, state).lifecycle?.("park"); } catch(error) { warning(error); }
  }
  // Flush already-saved catalog changes only, never infer a final summary.
  try {await sync(ctx, "shutdown", state);} finally {
   holder.retirement = state.retire();
   await holder.retirement;
   if (holder.current === state) {holder.current = undefined;scope.context = undefined;}
  }
 })());
}
