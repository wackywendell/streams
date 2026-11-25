import {
  automaticStreamFieldNames,
  buildSessionReportEdit,
  buildStreamReportEdit,
  summarizeWrittenReferences,
} from "./core.ts";
import {
  formatNotWritten,
  sessionKeyOf,
  streamKeyOf,
  type EditBody,
  type EditResult,
  type LocalContext,
  type StreamWire,
} from "./streams.ts";
import type {
  EnrollmentCandidateStream,
  EnrollmentInference,
  InferenceResult,
  ReportInference,
  SessionReportResult,
  StreamReportResult,
} from "./agent.ts";

export type ResetTarget = "all" | "session" | "stream";

export type ReportAction =
  | { kind: "report"; force: boolean; instructions?: string }
  | { kind: "reset"; target: ResetTarget; auto: boolean; instructions?: string };

export type ReportSnapshot = {
  native: string;
  cwd: string;
  evidence: { messages: any[]; leafId: string | null };
  catalog: LocalContext;
  valid(): boolean;
  attached(requireSameStream?: boolean): Promise<boolean>;
};

export type ReportGenerator = <T extends EnrollmentInference | ReportInference>(
  request: T
) => Promise<InferenceResult<T>>;

export type ReportEnvironment = {
  signal: AbortSignal;
  automaticEnrollment: boolean;
  capture(): Promise<ReportSnapshot>;
  enrollmentCandidates?(): Promise<{
    attachedStream?: StreamWire | null;
    candidates?: EnrollmentCandidateStream[];
  }>;
  generate: ReportGenerator;
  createStream(input: {
    title: string;
    titleMode: "automatic";
    goal?: string;
    goalShort?: string;
    goalMode?: "automatic";
  }): Promise<{ key: string }>;
  createSession(
    sessionId: string,
    input: { stream: string; title: string; titleMode: "automatic"; directory: string }
  ): Promise<{ key: string }>;
  editSession(target: string, body: EditBody): Promise<EditResult>;
  editStream(target: string, body: EditBody): Promise<EditResult>;
  onEnrolled?(info: { streamKey: string; sessionKey: string; sessionTitle: string }): Promise<void> | void;
  onEditResult?(kind: "session" | "stream", body: EditBody, result: EditResult): void;
  notice(text: string, level?: "info" | "warning"): void;
};

/** Shared report ordering and format-5 admission rules; the environment owns I/O and liveness. */
export async function runReport(
  action: ReportAction,
  environment: ReportEnvironment
): Promise<boolean> {
  const reset = action.kind === "reset";
  const auto = action.kind === "reset" && action.auto;
  const target = action.kind === "reset" ? action.target : "all";
  const force = action.kind === "reset" || action.force;
  const instructions = action.instructions;
  const { signal, generate } = environment;

  let savedAny = false;
  let capture = await environment.capture();
  if (!capture.valid() || signal.aborted) return savedAny;

  // 1. Enrollment when no Session exists yet.
  if (!capture.catalog.session) {
    if (reset) {
      environment.notice("Reset requires an enrolled Session. Use /streams attach first.");
      return false;
    }
    if (!force && !environment.automaticEnrollment) return false;

    let preboundStream: StreamWire | null = null;
    let candidates: EnrollmentCandidateStream[] = [];
    if (!capture.catalog.stream && environment.enrollmentCandidates) {
      const hint = await environment.enrollmentCandidates();
      if (signal.aborted || !(await capture.attached())) return false;
      preboundStream = hint?.attachedStream ?? null;
      candidates = preboundStream ? [] : (hint?.candidates ?? []);
    }

    const effectiveStream = capture.catalog.stream ?? preboundStream;
    const enrollCatalog: LocalContext = effectiveStream
      ? { ...capture.catalog, stream: effectiveStream }
      : capture.catalog;
    const candidateKeys = new Set(candidates.map((c) => c.key));

    const enrolled = await generate({
      type: "enroll",
      catalog: enrollCatalog,
      messages: capture.evidence.messages,
      instructions,
      ...(candidates.length > 0 ? { candidate_streams: candidates } : {}),
    });
    if (signal.aborted || !(await capture.attached()) || (!enrolled.meaningful && !force)) {
      return false;
    }
    const sessionTitle = enrolled.session?.title?.trim();
    const streamTitle = enrolled.stream?.title?.trim();
    const chosenStreamKey = enrolled.stream_key?.trim() || "";
    const matchedCandidateKey =
      chosenStreamKey && candidateKeys.has(chosenStreamKey) ? chosenStreamKey : null;
    if (!sessionTitle) {
      throw new Error("Enrollment requires meaningful Stream and Session titles");
    }
    if (!effectiveStream && !matchedCandidateKey && !streamTitle) {
      if (chosenStreamKey) {
        throw new Error(`Enrollment returned unknown Stream key: ${chosenStreamKey}`);
      }
      throw new Error("Enrollment requires meaningful Stream and Session titles");
    }

    let streamKey: string;
    if (effectiveStream) {
      streamKey = streamKeyOf(effectiveStream);
    } else if (matchedCandidateKey) {
      streamKey = matchedCandidateKey;
    } else {
      const created = await environment.createStream({
        title: streamTitle!,
        titleMode: "automatic",
        ...(enrolled.stream?.goal?.text?.trim()
          ? {
              goal: enrolled.stream.goal.text.trim(),
              goalShort: enrolled.stream.goal.short?.trim() || undefined,
              goalMode: "automatic" as const,
            }
          : {}),
      });
      streamKey = created.key;
    }

    const createdSession = await environment.createSession(capture.native, {
      stream: streamKey,
      title: sessionTitle,
      titleMode: "automatic",
      directory: capture.cwd,
    });
    savedAny = true;
    await environment.onEnrolled?.({
      streamKey,
      sessionKey: createdSession.key,
      sessionTitle,
    });

    if (!capture.valid() || signal.aborted) return savedAny;
    capture = await environment.capture();
    if (!capture.catalog.session || !capture.valid() || signal.aborted) return savedAny;
  }

  const targets: readonly ("session" | "stream")[] =
    !reset || target === "all" ? ["session", "stream"] : [target];

  // 2. Optional `--auto` reset: clear manual marks before regenerating.
  if (reset && auto) {
    const sessKey = sessionKeyOf(capture.catalog.session);
    const stKey = capture.catalog.stream ? streamKeyOf(capture.catalog.stream) : null;
    if (targets.includes("session") && capture.catalog.session.title.manual) {
      if (signal.aborted || !(await capture.attached())) return savedAny;
      const body: EditBody = {
        changes: { title: { text: capture.catalog.session.title.text, manual: false } },
        before: { title: structuredClone(capture.catalog.session.title) },
      };
      const res = await environment.editSession(sessKey, body);
      environment.onEditResult?.("session", body, res);
      savedAny = true;
    }
    if (targets.includes("stream") && capture.catalog.stream && stKey) {
      const st = capture.catalog.stream;
      const changes: Record<string, unknown> = {};
      const before: Record<string, unknown> = {};
      if (st.title.manual) {
        changes.title = { text: st.title.text, manual: false };
        before.title = structuredClone(st.title);
      }
      if (st.goal?.manual) {
        changes.goal = { text: st.goal.text, short: st.goal.short, manual: false };
        before.goal = structuredClone(st.goal);
      }
      if (Object.keys(changes).length > 0) {
        if (signal.aborted || !(await capture.attached(true))) return savedAny;
        const body: EditBody = { changes, before };
        const res = await environment.editStream(stKey, body);
        environment.onEditResult?.("stream", body, res);
        savedAny = true;
      }
    }
    capture = await environment.capture();
    if (!capture.valid() || !capture.catalog.session || signal.aborted) return savedAny;
  }

  // 3. Session report (progress + references + branch in ONE `session edit`).
  if (targets.includes("session")) {
    const inferred = (await generate({
      type: "session",
      catalog: capture.catalog,
      messages: capture.evidence.messages,
      instructions,
      reset,
    })) as SessionReportResult;
    if (signal.aborted || !(await capture.attached())) return savedAny;

    const filterNotices: string[] = [];
    const body = buildSessionReportEdit(
      capture.catalog.session,
      capture.catalog.naming,
      inferred.changes ?? {},
      inferred.reference_proposals,
      filterNotices
    );
    for (const msg of filterNotices) environment.notice(msg, "info");

    if (body) {
      const key = sessionKeyOf(capture.catalog.session);
      const res = await environment.editSession(key, body);
      savedAny = true;
      environment.onEditResult?.("session", body, res);
      for (const msg of summarizeWrittenReferences(body, res)) {
        environment.notice(msg, "info");
      }
      for (const skipped of formatNotWritten(res.not_written)) {
        environment.notice(`Session edit skipped: ${skipped}`, "warning");
      }
    }

    if (!targets.includes("stream")) return savedAny;
    if (!capture.valid() || signal.aborted) return savedAny;

    const prevSessionKey = sessionKeyOf(capture.catalog.session);
    const prevStreamKey = capture.catalog.stream ? streamKeyOf(capture.catalog.stream) : null;
    capture = await environment.capture();
    if (
      !capture.valid() ||
      !capture.catalog.session ||
      !capture.catalog.stream ||
      sessionKeyOf(capture.catalog.session) !== prevSessionKey ||
      streamKeyOf(capture.catalog.stream) !== prevStreamKey
    ) {
      return savedAny;
    }
  }

  // 4. Stream report (title + goal in ONE `stream edit`, when automatic fields are allowed).
  if (
    targets.includes("stream") &&
    capture.catalog.stream &&
    automaticStreamFieldNames(capture.catalog).length > 0
  ) {
    const inferred = (await generate({
      type: "stream",
      catalog: capture.catalog,
      messages: capture.evidence.messages,
      instructions,
      reset,
    })) as StreamReportResult;
    if (signal.aborted || !(await capture.attached(true))) return savedAny;

    const body = buildStreamReportEdit(
      capture.catalog.stream,
      capture.catalog.naming,
      inferred.changes ?? {}
    );
    if (body) {
      const key = streamKeyOf(capture.catalog.stream);
      const res = await environment.editStream(key, body);
      savedAny = true;
      environment.onEditResult?.("stream", body, res);
      for (const skipped of formatNotWritten(res.not_written)) {
        environment.notice(`Stream edit skipped: ${skipped}`, "warning");
      }
    }
  }

  return savedAny;
}
