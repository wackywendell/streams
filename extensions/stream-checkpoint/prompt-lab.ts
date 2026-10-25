import type { AssistantMessage } from "@earendil-works/pi-ai";
import { buildInferencePayload, projectMessages, runPreparedInference } from "./agent.ts";
import type { EnrollmentInference, ReportInference, InferencePayload, InferenceRuntime, InferenceObserver, InferenceResult, ModelSelection } from "./agent.ts";
import { digest, sourceFiles } from "./capture.ts";
import type { Capture } from "./capture.ts";

export type Task = {sample: number; type: "enroll" | "session" | "stream"; reset?: boolean};
type Input = EnrollmentInference | ReportInference;
export type PreparedRequest<T extends Input = Input> = {
 version: 1;
 source: {capture_id: string; sample: number; host_id: string; native_session_id: string; cwd: string; leafId: string | null};
 trim: boolean;
 input: T;
 model_request: InferencePayload;
 input_hash: string;
 source_files: Record<string, string>;
};
export type Result<T extends Input = Input> =
 | {status: "completed"; raw_response?: AssistantMessage; value: InferenceResult<T>}
 | {status: "failed" | "cancelled"; raw_response?: AssistantMessage; error: string};

export function prepare(recording: Capture, task: Task, prompt?: string, trim = true): PreparedRequest {
 if (!Number.isInteger(task.sample) || task.sample < 0 || !recording.samples[task.sample]) throw Error("Choose a captured sample index");
 const sample = recording.samples[task.sample];
 if (sample.unavailable || !sample.evidence || !sample.cwd) throw Error(sample.unavailable ?? "No frozen evidence for this sample");
 if (!trim && recording.context_mode !== "full") throw Error("This older capture contains trimmed evidence. Capture it again before preparing full context.");
 if (task.type === "enroll" && task.reset) throw Error("Enrollment cannot be reset");
 if (task.type === "enroll" && sample.catalog.session) throw Error("Enrollment requires an unbound captured conversation");
 if (task.type !== "enroll" && !sample.catalog[task.type]) throw Error(`No captured ${task.type} card`);
 const input: Input = {type: task.type, catalog: structuredClone(sample.catalog), messages: structuredClone(sample.evidence.messages), ...(task.reset ? {reset: true} : {})};
 return prepareInput(input, {capture_id: recording.id, sample: task.sample, host_id: sample.host_id, native_session_id: sample.native_session_id, cwd: sample.cwd, leafId: sample.evidence.leafId}, prompt, trim);
}
/** Replay supplies its current sandbox cards here rather than rereading captured cards. */
export function prepareInput<T extends Input>(input: T, source: PreparedRequest["source"], prompt?: string, trim = true): PreparedRequest<T> {
 if (prompt !== undefined && (typeof prompt !== "string" || !prompt.trim())) throw Error("A prompt override must contain text");
 const frozen = structuredClone(input);
 frozen.messages = projectMessages(frozen.messages, trim ? "trimmed" : "full");
 const model_request = buildInferencePayload(frozen);
 if (prompt !== undefined) model_request.systemPrompt = prompt;
 return {version: 1, source: structuredClone(source), trim, input: frozen, model_request, input_hash: digest(JSON.stringify({input: frozen, model_request})), source_files: sourceFiles()};
}
/** Executes the saved model request; it neither rerenders prompts nor applies card changes. */
export async function run<T extends Input>(request: PreparedRequest<T>, runtime: InferenceRuntime, model: ModelSelection, signal: AbortSignal, observe?: InferenceObserver): Promise<Result<T>> {
 let raw_response: AssistantMessage | undefined;
 try {
  const value = await runPreparedInference(runtime, request, signal, model, event => {
   if (event.type === "response") raw_response = structuredClone(event.response);
   observe?.(event);
  });
  return {status: "completed", raw_response, value};
 } catch (error) {return {status: signal.aborted || raw_response?.stopReason === "aborted" ? "cancelled" : "failed", raw_response, error: error instanceof Error ? error.message : String(error)};}
}
