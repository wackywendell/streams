import { join } from "node:path";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { captureIds, captureSessions, loadCapture, parseCaptureInventory } from "./capture.ts";
import { replayCapture } from "./replay.ts";
import type { ModelSelection } from "./agent.ts";
import type { ReportAction, ResetTarget } from "./report-workflow.ts";
import type { StreamCli } from "./streams.ts";
import type { ReportingSettings } from "./settings.ts";

type DebugCommand = {kind: "capture"; target?: {kind: "stream" | "workspace"; id: string}} | {kind: "replay"; id?: string; model?: ModelSelection; action: ReportAction};
const usage = "Usage: /streams debug capture [stream ID|workspace ID]; /streams debug replay [CAPTURE_ID] [--model PROVIDER/ID] [--reset [session|stream|all]] [--auto]";
export function parseDebugCommand(input: string): DebugCommand {
 const words = input.trim().split(/\s+/), kind = words.shift();
 if (kind === "capture" && (words.length === 0 || words.length === 2 && ["stream", "workspace"].includes(words[0]))) return {kind, target: words.length ? {kind: words[0] as "stream" | "workspace", id: words[1]} : undefined};
 if (kind !== "replay") throw Error(usage);
 const id = words[0] && !words[0].startsWith("--") ? words.shift() : undefined;
 let model: ModelSelection | undefined, reset: ResetTarget | undefined, auto = false;
 while (words.length) {
  const flag = words.shift();
  if (flag === "--model" && !model) {
   const value = words.shift(), slash = value?.indexOf("/") ?? -1;
   if (!value || slash < 1 || slash === value.length - 1 || value.startsWith("--")) throw Error(usage);
   model = {provider: value.slice(0, slash), id: value.slice(slash + 1)};
  } else if (flag === "--reset" && !reset) reset = ["session", "stream", "all"].includes(words[0]) ? words.shift() as ResetTarget : "all";
  else if (flag === "--auto" && !auto) auto = true;
  else throw Error(usage);
 }
 if (auto && !reset) throw Error("--auto requires --reset. " + usage);
 return {kind, id, model, action: reset ? {kind: "reset", target: reset, auto} : {kind: "report", force: true}};
}
/** Pi UI is an adapter; neither capture nor replay receives real catalog write methods. */
export async function debugCommand(input: string, ctx: ExtensionContext, client: Pick<StreamCli, "captureInputs" | "preview">, agentDir: string, settings: ReportingSettings, signal: AbortSignal): Promise<void> {
 if (!settings.debug_commands) throw Error("Debug commands are disabled. Enable them in /streams settings → Debug → Debug commands.");
 const command = parseDebugCommand(input), root = join(agentDir, "stream-debug");
 if (!ctx.hasUI) throw Error("Streams debug commands require Pi's interactive dialogs");
 const current = () => {signal.throwIfAborted();};
 if (command.kind === "capture") {
  let target = command.target;
  if (!target) {
   const inventory = parseCaptureInventory(await client.captureInputs(undefined, signal));current();
   const targets = [...inventory.streams.map(stream => ({kind: "stream" as const, id: stream.id, title: stream.title})), ...inventory.workspaces.map(workspace => ({kind: "workspace" as const, id: workspace.id, title: workspace.label}))];
   if (!targets.length) throw Error(`No capture targets found${inventory.topology_error ? `; ${inventory.topology_error}` : ""}`);
   const labels = targets.map(target => `${target.kind} · ${target.title} · ${target.id}`);
   const chosen = await ctx.ui.select("Capture a stream or workspace", labels, {signal});current();
   if (chosen === undefined) return;
   target = targets[labels.indexOf(chosen)];if (!target) return;
  }
  const inventory = parseCaptureInventory(await client.captureInputs(target, signal));current();
  const samples = inventory.samples ?? [], available = samples.filter(sample => !sample.unavailable || sample.host_id === inventory.host_id && sample.native_session_id === ctx.sessionManager.getSessionId()).length;
  const selection = samples.map(sample => `${sample.catalog.session?.title ?? "no card"} · ${sample.native_session_id} · ${sample.open === null ? "open unknown" : sample.open ? "open" : "closed"}${sample.unavailable ? ` · ${sample.unavailable}` : ""}`).join("\n");
  const proceed = await ctx.ui.confirm("Capture private session samples?", `${samples.length} Pi conversations; ${available} locally available; ${samples.filter(sample => sample.open === true).length} observed open; ${inventory.unsupported?.length ?? 0} unsupported non-Pi entries. Copies transcripts, cards and available helper logs into ${root}. Other-host/missing transcripts remain listed.${inventory.topology_error ? `\nOpen-state warning: ${inventory.topology_error}` : ""}\n\n${selection}`, {signal});current();
  if (!proceed) return;
  const {capture, directory} = captureSessions(inventory, root, settings.context, ctx.sessionManager, signal);
  const captured = capture.samples.filter(sample => sample.evidence).length;
  ctx.ui.notify(`Captured ${captured}/${capture.samples.length} conversations: ${directory}/capture.json`, captured === capture.samples.length ? "info" : "warning");
  return;
 }
 let id = command.id;
 if (!id) {
  const ids = captureIds(root);
  if (!ids.length) throw Error("No frozen captures. Run /streams debug capture first.");
  const captures = ids.map(id => {const capture = loadCapture(root, id);return {id, label: `${capture.captured_at} · ${capture.target.title} · ${capture.samples.length} sessions · ${id}`};});
  const chosen = await ctx.ui.select("Replay a frozen capture", captures.map(item => item.label), {signal});current();
  if (!chosen) return;
  id = captures.find(item => item.label === chosen)?.id;if (!id) return;
 }
 const capture = loadCapture(root, id);current();
 let model = command.model;
 if (!model) {
  const available = ctx.modelRegistry.getAvailable(), defaultModel = settings.model ?? ctx.model;
  const models = [...available].sort((a, b) => Number(b.provider === defaultModel?.provider && b.id === defaultModel?.id) - Number(a.provider === defaultModel?.provider && a.id === defaultModel?.id));
  const labels = models.map(model => `${model.provider}/${model.id}`);
  const chosen = await ctx.ui.select("Replay model", labels, {signal});current();
  if (!chosen) return;
  const selected = models[labels.indexOf(chosen)];if (!selected) return;
  model = {provider: selected.provider, id: selected.id};
 }
 if (!ctx.modelRegistry.find(model.provider, model.id)) throw Error("Replay model is unavailable; no fallback was used");
 const count = capture.samples.filter(sample => sample.evidence && !sample.unavailable).length;
 if (!await ctx.ui.confirm("Run model inference on frozen samples?", `${count} available conversations using ${model.provider}/${model.id}. Each may run enrollment, session and effort helpers. Normal provider requests/costs apply. Results stay local; real cards, sessions, names and layout are not changed.`, {signal})) return;
 current();
 ctx.ui.setStatus("stream-debug", "Streams debug · Replaying...");
 try {
  const {run, directory} = await replayCapture(capture, root, ctx, model, command.action, (input, signal) => client.preview(input, signal), signal);
  if (!signal.aborted) ctx.ui.notify(`Replay ${run.id}: ${run.samples.filter(sample => sample.status === "completed").length} completed, ${run.samples.filter(sample => sample.status === "failed").length} failed, ${run.samples.filter(sample => sample.status === "skipped").length} skipped, ${run.samples.filter(sample => sample.status === "unavailable").length} unavailable, ${run.samples.filter(sample => sample.status === "not-run" || sample.status === "cancelled").length} unrun/cancelled. Comparison: ${directory}/comparison.md`, run.samples.some(sample => sample.status !== "completed") ? "warning" : "info");
 } finally {if (!signal.aborted) ctx.ui.setStatus("stream-debug", undefined);}
}
