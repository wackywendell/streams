import { randomUUID } from "node:crypto";
import type { AssistantMessage } from "@earendil-works/pi-ai";
import { join } from "node:path";
import { helperSessionLogger } from "./agent.ts";
import type {
  EnrollmentInference,
  Inference,
  InferenceResult,
  InferenceRuntime,
  Inferred,
  ModelSelection,
  ReportInference,
} from "./agent.ts";
import { applyEditLocally } from "./core.ts";
import { prepareInput, run as runPrepared } from "./prompt-lab.ts";
import { savePrepared } from "./lab-artifacts.ts";
import { digest, privateDirectory, privateWrite, sourceFiles } from "./capture.ts";
import type { Capture } from "./capture.ts";
import { runReport, type ReportAction } from "./report-workflow.ts";
import {
  formatNotWritten,
  sessionKeyOf,
  streamKeyOf,
  type EditBody,
  type EditResult,
  type LocalContext,
  type SessionWire,
  type StreamWire,
} from "./streams.ts";

type ReplayTask = {
  task: Inference["type"];
  input_file: string;
  request_id?: string;
  status: "running" | "parsed" | "failed";
  model?: ModelSelection;
  prompt_hash?: string;
  helper_file?: string;
  raw_response?: AssistantMessage;
  result?: Inferred;
  error?: string;
};

type ReplaySample = {
  native_session_id: string;
  host_id: string;
  open: boolean | null;
  status: "not-run" | "completed" | "skipped" | "failed" | "unavailable" | "cancelled";
  before: LocalContext;
  after: LocalContext;
  tasks: ReplayTask[];
  edits: { kind: "session" | "stream"; body: EditBody; result: EditResult }[];
  notices: string[];
  error?: string;
};

export type ReplayRun = {
  version: 2;
  id: string;
  capture_id: string;
  started_at: string;
  completed_at?: string;
  model: ModelSelection;
  action: ReportAction;
  source_files: Record<string, string>;
  capture_source_files: Record<string, string>;
  samples: ReplaySample[];
};

/** Each conversation starts at its frozen base; later tasks see its own simulated in-memory saves. */
export async function replayCapture(
  capture: Capture,
  root: string,
  runtime: InferenceRuntime,
  model: ModelSelection,
  action: ReportAction,
  signal: AbortSignal,
  options: { trim?: boolean; prompt?: string } = {}
): Promise<{ run: ReplayRun; directory: string }> {
  if (!runtime.modelRegistry.find(model.provider, model.id)) {
    throw new Error("Replay model is unavailable; no fallback was used");
  }
  signal.throwIfAborted();
  if (options.trim === false && capture.context_mode !== "full") {
    throw new Error(
      "This older capture contains trimmed evidence. Capture it again before replaying full context."
    );
  }
  const id = randomUUID();
  const directory = join(root, "runs", id);
  const helpers = join(directory, "helpers");
  privateDirectory(helpers, root);
  const run: ReplayRun = {
    version: 2,
    id,
    capture_id: capture.id,
    started_at: new Date().toISOString(),
    model,
    action,
    source_files: sourceFiles(),
    capture_source_files: structuredClone(capture.source_files),
    samples: capture.samples.map((sample) => ({
      native_session_id: sample.native_session_id,
      host_id: sample.host_id,
      open: sample.open,
      status: "not-run",
      before: structuredClone(sample.catalog),
      after: structuredClone(sample.catalog),
      tasks: [],
      edits: [],
      notices: [],
    })),
  };

  try {
    for (const [index, sample] of capture.samples.entries()) {
      const result = run.samples[index];
      if (signal.aborted) break;
      if (sample.unavailable || !sample.evidence) {
        result.status = "unavailable";
        result.error = sample.unavailable ?? "No frozen evidence";
        continue;
      }
      let catalog = structuredClone(sample.catalog);
      const generate = async <T extends EnrollmentInference | ReportInference>(
        request: T
      ): Promise<InferenceResult<T>> => {
        const task: ReplayTask = {
          task: request.type,
          input_file: `sample-${index}-task-${result.tasks.length}.json`,
          status: "running",
        };
        result.tasks.push(task);
        try {
          const prepared = prepareInput(
            request,
            {
              capture_id: capture.id,
              sample: index,
              host_id: sample.host_id,
              native_session_id: sample.native_session_id,
              cwd: sample.cwd!,
              leafId: sample.evidence!.leafId,
            },
            options.prompt,
            options.trim ?? true
          );
          task.request_id = savePrepared(root, prepared).id;
          privateWrite(
            join(directory, task.input_file),
            JSON.stringify(
              {
                origin: "reconstructed-from-frozen-sample",
                request: prepared.input,
                request_id: task.request_id,
              },
              null,
              2
            ) + "\n",
            root
          );
          const logger = helperSessionLogger(
            {
              cwd: sample.cwd!,
              sessionId: sample.native_session_id,
              leafId: sample.evidence!.leafId,
              sessionFile: sample.snapshot_file
                ? join(root, "captures", capture.id, sample.snapshot_file)
                : undefined,
            },
            {
              directory: helpers,
              onCreate: (value) => {
                task.helper_file = `helpers/${value.session_file.split(/[\\/]/).at(-1)}`;
              },
            }
          );
          const outcome = await runPrepared(prepared, runtime, model, signal, (event) => {
            if (event.type === "request") {
              task.model = event.model;
              task.prompt_hash = digest(
                JSON.stringify({
                  systemPrompt: event.payload.systemPrompt,
                  tools: event.payload.tools,
                })
              );
            }
            if (event.type === "response") task.raw_response = structuredClone(event.response);
            if (event.type === "result") {
              task.status = "parsed";
              task.result = structuredClone(event.result);
            }
            logger(event);
          });
          if (outcome.status !== "completed") {
            throw new Error(outcome.error ?? "Replay inference cancelled");
          }
          return outcome.value;
        } catch (error) {
          task.status = "failed";
          task.error = String(error);
          throw error;
        }
      };

      try {
        const saved = await runReport(action, {
          signal,
          automaticEnrollment: true,
          capture: async () => ({
            native: sample.native_session_id,
            cwd: sample.cwd!,
            evidence: structuredClone(sample.evidence!),
            catalog: structuredClone(catalog),
            valid: () => !signal.aborted,
            attached: async () => !signal.aborted,
          }),
          generate,
          createStream: async (input) => {
            const st: StreamWire = {
              host_id: sample.host_id,
              id: randomUUID(),
              title: { text: input.title, manual: false },
              goal: input.goal
                ? { text: input.goal, short: input.goalShort ?? null, manual: false }
                : null,
              revision: 1,
              updated_at_ms: Date.now(),
            };
            catalog.stream = st;
            result.after = structuredClone(catalog);
            return { key: streamKeyOf(st) };
          },
          createSession: async (sessionId, input) => {
            const streamId = input.stream.includes("/")
              ? input.stream.split("/").slice(1).join("/")
              : input.stream;
            const sess: SessionWire = {
              host_id: sample.host_id,
              harness: "pi",
              id: sessionId,
              stream_id: streamId,
              title: { text: input.title, manual: false },
              status: "active",
              recap: null,
              state: null,
              remaining: null,
              directory: { path: input.directory, repository: null },
              references: {},
              artifacts: {},
              revision: 1,
              updated_at_ms: Date.now(),
            };
            catalog.session = sess;
            result.after = structuredClone(catalog);
            return { key: sessionKeyOf(sess) };
          },
          editSession: async (_target, body) => {
            if (!catalog.session) throw new Error("No simulated Session");
            const applied = applyEditLocally(catalog.session, body);
            catalog.session = applied.after;
            result.after = structuredClone(catalog);
            return applied.result;
          },
          editStream: async (_target, body) => {
            if (!catalog.stream) throw new Error("No simulated Stream");
            const applied = applyEditLocally(catalog.stream, body);
            catalog.stream = applied.after;
            result.after = structuredClone(catalog);
            return applied.result;
          },
          onEditResult: (kind, body, editRes) => {
            result.edits.push({ kind, body, result: editRes });
            for (const line of formatNotWritten(editRes.not_written)) {
              result.notices.push(line);
            }
          },
          notice: (text) => {
            result.notices.push(text);
          },
        });
        result.status = signal.aborted
          ? "cancelled"
          : !saved && !result.tasks.length
            ? "skipped"
            : "completed";
        if (result.status === "skipped" && !result.notices.length) {
          result.notices.push("No helper ran: the selected fields are manual.");
        }
      } catch (error) {
        result.status = signal.aborted ? "cancelled" : "failed";
        result.error = String(error);
      }
    }
  } finally {
    run.completed_at = new Date().toISOString();
    privateWrite(join(directory, "run.json"), JSON.stringify(run, null, 2) + "\n", root);
    privateWrite(join(directory, "comparison.md"), comparison(run), root);
  }
  return { run, directory };
}

export function comparison(run: ReplayRun): string {
  const lines = [
    "# Streams prompt replay",
    "",
    `Capture: ${run.capture_id}`,
    `Model: ${run.model.provider}/${run.model.id}`,
    `Mode: ${run.action.kind === "report" ? "incremental" : `reset ${run.action.target}${run.action.auto ? " --auto" : ""}`}`,
    "",
    "Each sample starts independently from its captured cards. All resulting cards and edits are simulated. No live writes are exercised.",
    "",
  ];
  for (const sample of run.samples) {
    lines.push(
      `## ${sample.host_id} / ${sample.native_session_id}`,
      "",
      `Replay: ${sample.status}.`,
      ""
    );
    for (const kind of ["session", "stream"] as const) {
      lines.push(
        `### ${kind}`,
        "",
        "Before:",
        "```json",
        JSON.stringify(sample.before[kind], null, 2),
        "```",
        "After (simulated):",
        "```json",
        JSON.stringify(sample.after[kind], null, 2),
        "```",
        ""
      );
    }
    for (const task of sample.tasks) {
      lines.push(
        `- ${task.task}: ${task.status}; prompt ${task.prompt_hash ?? "not built"}; input ${task.input_file}; request ${task.request_id ?? "not prepared"}; helper ${task.helper_file ?? "not recorded"}${task.error ? `; ${task.error}` : ""}`
      );
    }
    if (sample.error) lines.push("", `Error: ${sample.error}`);
    lines.push(...sample.notices.map((notice) => `Notice: ${notice}`), "");
  }
  return lines.join("\n") + "\n";
}
