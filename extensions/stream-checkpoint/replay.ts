import { randomUUID } from "node:crypto";
import { join } from "node:path";
import { runInference } from "./agent.ts";
import type { Inference, Inferred, InferenceResult, InferenceRuntime, ModelSelection, InferenceEvent } from "./agent.ts";
import { digest, privateDirectory, privateWrite, sourceFiles } from "./capture.ts";
import type { Capture } from "./capture.ts";
import { runReport } from "./report-workflow.ts";
import type { ReportAction } from "./report-workflow.ts";
import type { LocalContext, Receipt } from "./streams.ts";

/** The only catalog capability available to replay. No apply/enroll client is accepted. */
export type Preview = (input: {catalog: LocalContext; action: "apply" | "enroll"; request: unknown}, signal: AbortSignal) => Promise<{catalog: LocalContext; receipt: Receipt}>;
type ReplayTask = {task: Inference["type"]; artifact: string; status: "running" | "parsed" | "failed"; model?: ModelSelection; prompt_hash?: string; result?: Inferred; error?: string};
type ReplaySample = {native_session_id: string; host_id: string; open: boolean | null; status: "not-run" | "completed" | "failed" | "unavailable" | "cancelled"; before: LocalContext; after: LocalContext; tasks: ReplayTask[]; receipts: Receipt[]; notices: string[]; error?: string};
export type ReplayRun = {version: 1; id: string; capture_id: string; started_at: string; completed_at?: string; model: ModelSelection; action: ReportAction; capture_source_files: Record<string, string>; source_files: Record<string, string>; samples: ReplaySample[]};
/** Each trial starts from its own frozen cards. Only helpers within that trial share projections. */
export async function replayCapture(capture: Capture, root: string, runtime: InferenceRuntime, model: ModelSelection, action: ReportAction, preview: Preview, signal: AbortSignal): Promise<{run: ReplayRun; directory: string}> {
 signal.throwIfAborted();
 if (!runtime.modelRegistry.find(model.provider, model.id)) throw Error("Replay model is unavailable; no fallback was used");
 const frozen = structuredClone(capture), id = randomUUID(), directory = join(root, "runs", id);
 privateDirectory(directory, root);
 const run: ReplayRun = {version: 1, id, capture_id: frozen.id, started_at: new Date().toISOString(), model: structuredClone(model), action: structuredClone(action), capture_source_files: structuredClone(frozen.source_files), source_files: sourceFiles(), samples: frozen.samples.map(sample => ({native_session_id: sample.native_session_id, host_id: sample.host_id, open: sample.open, status: "not-run", before: structuredClone(sample.catalog), after: structuredClone(sample.catalog), tasks: [], receipts: [], notices: []}))};
 try {
  for (const [index, sample] of frozen.samples.entries()) {
   const result = run.samples[index];
   if (signal.aborted) break;
   if (sample.unavailable || !sample.evidence) {result.status = "unavailable";result.error = sample.unavailable ?? "No frozen evidence";continue;}
   let catalog = structuredClone(sample.catalog), operation = 0;
   const generate = async <T extends Inference>(request: T): Promise<InferenceResult<T>> => {
    const task: ReplayTask = {task: request.type, artifact: `sample-${index}-task-${result.tasks.length}.json`, status: "running"};
    const events: InferenceEvent[] = [];
    result.tasks.push(task);
    try {
     return await runInference(runtime, request, signal, model, event => {
      events.push(structuredClone(event));
      if (event.type === "request") {task.model = event.model;task.prompt_hash = digest(JSON.stringify(event.payload));}
      if (event.type === "result") {task.status = "parsed";task.result = structuredClone(event.result);}
     });
    } catch (error) {task.status = "failed";task.error = String(error);throw error;}
    finally {
     // Includes the exact rendered prompt/schema and every raw attempt, even parse failures.
     privateWrite(join(directory, task.artifact), JSON.stringify({origin: "reconstructed-from-frozen-sample", request, events, result: task.result, error: task.error}, null, 2) + "\n", root);
    }
   };
   const project = async (kind: "apply" | "enroll", request: unknown) => {
    signal.throwIfAborted();
    const projected = await preview({catalog: structuredClone(catalog), action: kind, request}, signal);
    signal.throwIfAborted();
    catalog = structuredClone(projected.catalog);result.after = structuredClone(catalog);
    return projected.receipt;
   };
   try {
    const completed = await runReport(action, {
     signal, live: () => !signal.aborted, automaticEnrollment: true, operationId: () => `replay:${frozen.id}:${index}:${operation++}`,
     capture: async () => ({native: sample.native_session_id, cwd: sample.cwd!, evidence: structuredClone(sample.evidence!), catalog: structuredClone(catalog), valid: () => !signal.aborted, attached: async () => !signal.aborted}),
     generate, apply: request => project("apply", request), enroll: request => project("enroll", request),
     receipt: receipt => {result.receipts.push({...receipt, simulated: true} as Receipt);}, notice: text => {result.notices.push(text);},
    });
    result.status = signal.aborted ? "cancelled" : completed ? "completed" : result.tasks.length ? "failed" : "not-run";
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
 const lines = ["# Streams prompt replay", "", `Capture: ${run.capture_id}`, `Model: ${run.model.provider}/${run.model.id}`, `Mode: ${run.action.kind === "report" ? "incremental" : `reset ${run.action.target}${run.action.auto ? " --auto" : ""}`}`, "", "Each sample starts independently from its frozen cards. All cards and receipts below are simulated; no live writes or publication are exercised.", ""];
 for (const sample of run.samples) {
  lines.push(`## ${sample.host_id} / ${sample.native_session_id}`, "", `Open at capture: ${sample.open === null ? "unknown" : sample.open ? "yes" : "no"}. Replay: ${sample.status}.`, "");
  for (const kind of ["session", "stream"] as const) lines.push(`### ${kind}`, "", "Before:", "```json", JSON.stringify(sample.before[kind], null, 2), "```", "After (simulated):", "```json", JSON.stringify(sample.after[kind], null, 2), "```", "");
  for (const task of sample.tasks) lines.push(`- ${task.task}: ${task.status}; prompt ${task.prompt_hash ?? "not built"}; diagnostics ${task.artifact}${task.error ? `; ${task.error}` : ""}`);
  if (sample.error) lines.push("", `Error: ${sample.error}`);
  lines.push(...sample.notices.map(notice => `Notice: ${notice}`), "");
 }
 return lines.join("\n") + "\n";
}
