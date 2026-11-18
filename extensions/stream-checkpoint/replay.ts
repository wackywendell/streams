import { randomUUID } from "node:crypto";
import type { AssistantMessage } from "@earendil-works/pi-ai";
import { join } from "node:path";
import { helperSessionLogger } from "./agent.ts";
import { prepareInput, run as runPrepared } from "./prompt-lab.ts";
import { savePrepared } from "./lab-artifacts.ts";
import type { Inference, Inferred, InferenceResult, EnrollmentInference, ReportInference, InferenceRuntime, ModelSelection } from "./agent.ts";
import { digest, privateDirectory, privateWrite, sourceFiles } from "./capture.ts";
import type { Capture } from "./capture.ts";
import { runReport } from "./report-workflow.ts";
import type { ReportAction } from "./report-workflow.ts";
import { receiptCanContinue, receiptHasWarning, receiptText, type LocalContext, type Receipt } from "./streams.ts";

export type Preview = (input: {catalog: LocalContext; action: "apply" | "enroll"; request: unknown}, signal: AbortSignal) => Promise<{catalog: LocalContext; receipt: Receipt}>;
type ReplayTask = {task: Inference["type"]; input_file: string; request_id?: string; status: "running" | "parsed" | "failed"; model?: ModelSelection; prompt_hash?: string; helper_file?: string; raw_response?: AssistantMessage; result?: Inferred; error?: string};
type ReplaySample = {native_session_id: string; host_id: string; open: boolean | null; status: "not-run" | "completed" | "skipped" | "failed" | "unavailable" | "cancelled"; before: LocalContext; after: LocalContext; tasks: ReplayTask[]; receipts: Receipt[]; notices: string[]; error?: string};
export type ReplayRun = {version: 1; id: string; capture_id: string; started_at: string; completed_at?: string; model: ModelSelection; action: ReportAction; source_files: Record<string, string>; capture_source_files: Record<string, string>; samples: ReplaySample[]};
/** Each conversation starts at its frozen base; later tasks see its own simulated saves. */
export async function replayCapture(capture: Capture, root: string, runtime: InferenceRuntime, model: ModelSelection, action: ReportAction, preview: Preview, signal: AbortSignal, options: {trim?: boolean; prompt?: string} = {}): Promise<{run: ReplayRun; directory: string}> {
 if (!runtime.modelRegistry.find(model.provider, model.id)) throw Error("Replay model is unavailable; no fallback was used");
 signal.throwIfAborted();
 if (options.trim === false && capture.context_mode !== "full") throw Error("This older capture contains trimmed evidence. Capture it again before replaying full context.");
 const id = randomUUID(), directory = join(root, "runs", id), helpers = join(directory, "helpers");
 privateDirectory(helpers, root);
 const run: ReplayRun = {version: 1, id, capture_id: capture.id, started_at: new Date().toISOString(), model, action, source_files: sourceFiles(), capture_source_files: structuredClone(capture.source_files), samples: capture.samples.map(sample => ({native_session_id: sample.native_session_id, host_id: sample.host_id, open: sample.open, status: "not-run", before: structuredClone(sample.catalog), after: structuredClone(sample.catalog), tasks: [], receipts: [], notices: []}))};
 try {
  for (const [index, sample] of capture.samples.entries()) {
   const result = run.samples[index];
   if (signal.aborted) break;
   if (sample.unavailable || !sample.evidence) {result.status = "unavailable";result.error = sample.unavailable ?? "No frozen evidence";continue;}
   let catalog = structuredClone(sample.catalog);
   const generate = async <T extends EnrollmentInference | ReportInference>(request: T): Promise<InferenceResult<T>> => {
    const task: ReplayTask = {task: request.type, input_file: `sample-${index}-task-${result.tasks.length}.json`, status: "running"};
    result.tasks.push(task);
    try {
     const prepared = prepareInput(request, {capture_id: capture.id, sample: index, host_id: sample.host_id, native_session_id: sample.native_session_id, cwd: sample.cwd!, leafId: sample.evidence!.leafId}, options.prompt, options.trim ?? true);
     task.request_id = savePrepared(root, prepared).id;
     // These inputs are reconstructed, never claimed to be historically sent requests.
     privateWrite(join(directory, task.input_file), JSON.stringify({origin: "reconstructed-from-frozen-sample", request: prepared.input, request_id: task.request_id}, null, 2) + "\n", root);
     const logger = helperSessionLogger({cwd: sample.cwd!, sessionId: sample.native_session_id, leafId: sample.evidence!.leafId, sessionFile: sample.snapshot_file ? join(root, "captures", capture.id, sample.snapshot_file) : undefined}, {directory: helpers, onCreate: value => {task.helper_file = `helpers/${value.session_file.split(/[\\/]/).at(-1)}`;}});
     const outcome = await runPrepared(prepared, runtime, model, signal, event => {
      if (event.type === "request") {task.model = event.model;task.prompt_hash = digest(JSON.stringify({systemPrompt: event.payload.systemPrompt, tools: event.payload.tools}));}
      if (event.type === "response") task.raw_response = structuredClone(event.response);
      if (event.type === "result") {task.status = "parsed";task.result = structuredClone(event.result);}
      logger(event);
     });
     if (outcome.status !== "completed") throw Error(outcome.error ?? "Replay inference cancelled");
     return outcome.value;
    } catch (error) {task.status = "failed";task.error = String(error);throw error;}
   };
   const project = async (kind: "apply" | "enroll", request: unknown) => {
    signal.throwIfAborted();
    const projected = await preview({catalog: structuredClone(catalog), action: kind, request}, signal);
    catalog = structuredClone(projected.catalog);result.after = structuredClone(catalog);
    return projected.receipt;
   };
   try {
    const saved = await runReport(action, {
     signal, automaticEnrollment: true,
     capture: async () => ({native: sample.native_session_id, cwd: sample.cwd!, evidence: structuredClone(sample.evidence!), catalog: structuredClone(catalog), valid: () => !signal.aborted, attached: async () => !signal.aborted}),
     generate,
     // A captured card was decoded when it was captured; there is nothing to discard.
     discardUnreadable: async () => ({session: null, stream: null}),
     apply: request => project("apply", request), enroll: request => project("enroll", request),
     receipt: receipt => {const saved = {...receipt, simulated:true} as Receipt;result.receipts.push(saved);if (receiptHasWarning(saved)) result.notices.push(receiptText(saved));}, notice: text => {result.notices.push(text);},
    });
    const refused = result.receipts.find(receipt => !receiptCanContinue(receipt));
    if (refused) throw Error(`Simulated save refused: ${receiptText(refused)}`);
    // A valid empty model delta saves nothing, but still completes inference.
    result.status = signal.aborted ? "cancelled" : !saved && !result.tasks.length ? "skipped" : "completed";
    if (result.status === "skipped" && !result.notices.length) result.notices.push("No helper ran: the selected fields are manual or no reporting authority was captured.");
   } catch (error) {result.status = signal.aborted ? "cancelled" : "failed";result.error = String(error);}
  }
 } finally {
  run.completed_at = new Date().toISOString();
  privateWrite(join(directory, "run.json"), JSON.stringify(run, null, 2) + "\n", root);
  privateWrite(join(directory, "comparison.md"), comparison(run), root);
 }
 return {run, directory};
}
export function comparison(run: ReplayRun): string {
 const lines = ["# Streams prompt replay", "", `Capture: ${run.capture_id}`, `Model: ${run.model.provider}/${run.model.id}`, `Mode: ${run.action.kind === "report" ? "incremental" : `reset ${run.action.target}${run.action.auto ? " --auto" : ""}`}`, "", "Each sample starts independently from its captured cards. All resulting cards and receipts are simulated. No live writes or publication are exercised.", ""];
 for (const sample of run.samples) {
  lines.push(`## ${sample.host_id} / ${sample.native_session_id}`, "", `Open at capture: ${sample.open === null ? "unknown" : sample.open ? "yes" : "no"}. Replay: ${sample.status}.`, "");
  for (const kind of ["session", "stream"] as const) lines.push(`### ${kind}`, "", "Before:", "```json", JSON.stringify(sample.before[kind], null, 2), "```", "After (simulated):", "```json", JSON.stringify(sample.after[kind], null, 2), "```", "");
  for (const task of sample.tasks) lines.push(`- ${task.task}: ${task.status}; prompt ${task.prompt_hash ?? "not built"}; input ${task.input_file}; request ${task.request_id ?? "not prepared"}; helper ${task.helper_file ?? "not recorded"}${task.error ? `; ${task.error}` : ""}`);
  if (sample.error) lines.push("", `Error: ${sample.error}`);
  lines.push(...sample.notices.map(notice => `Notice: ${notice}`), "");
 }
 return lines.join("\n") + "\n";
}
