import { createHash, randomUUID } from "node:crypto";
import { lstatSync, readdirSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { CURRENT_SESSION_VERSION, SessionManager } from "@earendil-works/pi-coding-agent";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { parseLocalContext, streamKeyOf, type LocalContext } from "./wire.ts";
import type { StreamCli } from "./streams.ts";
import { privateDirectory, privateRead, privateWrite } from "./private-storage.ts";
export { privateDirectory, privateRead, privateWrite } from "./private-storage.ts";

export type InventorySample = {
  host_id: string;
  harness: "pi";
  native_session_id: string;
  open: boolean | null;
  transcript_file: string | null;
  unavailable: string | null;
  catalog: LocalContext;
};

export type CaptureInventory = {
  version: 2;
  host_id: string;
  streams: { id: string; key: string; title: string }[];
  target?: { kind: "stream"; id: string; title: string };
  samples?: InventorySample[];
};

export type FrozenSample = InventorySample & {
  captured_at: string;
  cwd?: string;
  projection_source?: "current-session" | "persisted-leaf";
  evidence?: { messages: any[]; leafId: string | null };
  evidence_hash?: string;
  snapshot_file?: string;
  snapshot_hash?: string;
  warnings: string[];
  helper_logs: { session_id: string; file?: string; error?: string }[];
};

export type Capture = {
  version: 2;
  id: string;
  captured_at: string;
  context_mode: "trimmed" | "full";
  target: { kind: "stream"; id: string; title: string };
  host_id: string;
  source_files: Record<string, string>;
  samples: FrozenSample[];
};

const object = (value: unknown): value is Record<string, any> =>
  !!value && typeof value === "object" && !Array.isArray(value);
const identity = (value: unknown): value is string =>
  typeof value === "string" && value.trim().length > 0 && !/[\x00-\x1f]/.test(value);

/** Find Pi JSONL transcripts under `agentDir/sessions` keyed by header `id`. */
export function findPiTranscripts(agentDir: string): Map<string, string> {
  const result = new Map<string, string>();
  const sessionsDir = join(agentDir, "sessions");
  const walk = (dir: string, depth: number) => {
    if (depth > 4) return;
    let entries;
    try {
      entries = readdirSync(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const entry of entries) {
      const full = join(dir, entry.name);
      if (entry.isDirectory()) {
        walk(full, depth + 1);
      } else if (entry.isFile() && entry.name.endsWith(".jsonl")) {
        try {
          const text = readFileSync(full, "utf8");
          const firstLine = text.split("\n", 1)[0];
          const header = JSON.parse(firstLine);
          if (header?.type === "session" && typeof header.id === "string" && !result.has(header.id)) {
            result.set(header.id, full);
          }
        } catch {
          // Ignore malformed transcripts
        }
      }
    }
  };
  walk(sessionsDir, 0);
  return result;
}

/** Build a CaptureInventory using only public `streams` CLI read commands. */
export async function buildCaptureInventory(
  cli: Pick<StreamCli, "showStream" | "listStreamSessions" | "listStreams" | "getConfig">,
  streamSelector?: string,
  agentDir = process.env.PI_CODING_AGENT_DIR ?? join(homedir(), ".pi", "agent"),
  signal?: AbortSignal
): Promise<CaptureInventory> {
  const cfg = await cli.getConfig(signal);
  const listing = await cli.listStreams({ all: true, signal });
  const streams = listing.items.map((s) => ({
    id: s.id,
    key: streamKeyOf(s),
    title: s.title.text,
  }));
  if (!streamSelector) {
    return {
      version: 2,
      host_id: listing.items[0]?.host_id ?? "local",
      streams,
    };
  }
  const stream = await cli.showStream(streamSelector, signal);
  const streamKey = streamKeyOf(stream);
  const sessionListing = await cli.listStreamSessions(streamKey, signal);
  const transcripts = findPiTranscripts(agentDir);
  const samples: InventorySample[] = [];
  for (const session of sessionListing.items) {
    if (session.harness !== "pi") continue;
    const transcriptFile = transcripts.get(session.id) ?? null;
    samples.push({
      host_id: session.host_id,
      harness: "pi",
      native_session_id: session.id,
      open: null,
      transcript_file: transcriptFile,
      unavailable: transcriptFile ? null : "Pi transcript not found locally",
      catalog: {
        session,
        stream,
        naming: { ...cfg.naming },
      },
    });
  }
  return {
    version: 2,
    host_id: stream.host_id,
    streams,
    target: { kind: "stream", id: stream.id, title: stream.title.text },
    samples,
  };
}

export function parseCaptureInventory(value: unknown): CaptureInventory {
  if (
    !object(value) ||
    value.version !== 2 ||
    !identity(value.host_id) ||
    !Array.isArray(value.streams)
  ) {
    throw new Error("Invalid capture inventory");
  }
  for (const stream of value.streams) {
    if (!object(stream) || !identity(stream.id) || typeof stream.title !== "string") {
      throw new Error("Invalid capture stream");
    }
  }
  if (
    value.target !== undefined &&
    (!object(value.target) ||
      value.target.kind !== "stream" ||
      !identity(value.target.id) ||
      typeof value.target.title !== "string")
  ) {
    throw new Error("Invalid capture target");
  }
  if (value.samples !== undefined) {
    if (!value.target || !Array.isArray(value.samples)) throw new Error("Invalid capture samples");
    const seen = new Set<string>();
    for (const sample of value.samples) {
      if (
        !object(sample) ||
        !identity(sample.host_id) ||
        sample.harness !== "pi" ||
        !identity(sample.native_session_id) ||
        ![true, false, null].includes(sample.open) ||
        !(sample.transcript_file === null || identity(sample.transcript_file)) ||
        !(sample.unavailable === null || typeof sample.unavailable === "string")
      ) {
        throw new Error("Invalid capture sample");
      }
      sample.catalog = parseLocalContext(sample.catalog);
      const session = sample.catalog.session;
      if (session && (session.host_id !== sample.host_id || session.id !== sample.native_session_id)) {
        throw new Error("Captured card/conversation identity mismatch");
      }
      const key = JSON.stringify([sample.host_id, sample.native_session_id]);
      if (seen.has(key)) throw new Error("Duplicate capture sample");
      seen.add(key);
    }
  }
  return value as CaptureInventory;
}

export const digest = (value: string): string =>
  createHash("sha256").update(value).digest("hex");

export function sourceFiles(): Record<string, string> {
  return Object.fromEntries(
    [
      "agent.ts",
      "core.ts",
      "report-workflow.ts",
      "capture.ts",
      "prompt-lab.ts",
      "replay.ts",
      "private-storage.ts",
      "helper-log.ts",
      "provider-log.ts",
    ].map((name) => [name, digest(readFileSync(new URL(name, import.meta.url), "utf8"))])
  );
}

function transcript(content: string, expected: string): { entries: any[]; partial: boolean } {
  const lines = content.split("\n");
  if (lines.at(-1) === "") lines.pop();
  let partial = false;
  const entries = lines.flatMap((line, index) => {
    try {
      return [JSON.parse(line)];
    } catch (error) {
      if (index !== lines.length - 1 || content.endsWith("\n")) throw error;
      partial = true;
      return [];
    }
  });
  const header = entries[0];
  if (!object(header) || header.type !== "session" || header.id !== expected || typeof header.cwd !== "string") {
    throw new Error("Captured transcript identity/header mismatch");
  }
  if (
    header.version !== undefined &&
    (!Number.isInteger(header.version) || header.version < 1 || header.version > CURRENT_SESSION_VERSION)
  ) {
    throw new Error("Unsupported captured transcript version");
  }
  return { entries, partial };
}

/** Snapshot data only. Never open a source file with a mutating SessionManager. */
export function captureSessions(
  inventory: CaptureInventory,
  root: string,
  current?: ExtensionContext["sessionManager"],
  signal?: AbortSignal,
  helperRoot?: string
): { capture: Capture; directory: string } {
  inventory = parseCaptureInventory(structuredClone(inventory));
  if (!inventory.target || !inventory.samples) throw new Error("Select a capture target first");
  signal?.throwIfAborted();
  const id = randomUUID();
  const directory = join(root, "captures", id);
  privateDirectory(directory, root);
  const capture: Capture = {
    version: 2,
    id,
    captured_at: new Date().toISOString(),
    context_mode: "full",
    target: inventory.target,
    host_id: inventory.host_id,
    source_files: sourceFiles(),
    samples: [],
  };
  for (const [index, source] of inventory.samples.entries()) {
    signal?.throwIfAborted();
    const sample: FrozenSample = {
      ...structuredClone(source),
      captured_at: new Date().toISOString(),
      warnings: [],
      helper_logs: [],
    };
    capture.samples.push(sample);
    const live =
      source.host_id === inventory.host_id &&
      current?.getSessionId() === source.native_session_id
        ? current
        : undefined;
    if (source.unavailable && !live) continue;
    try {
      if (live) {
        sample.unavailable = null;
        sample.open = true;
      } else if (source.open) {
        sample.warnings.push(
          "Evidence uses the persisted leaf, not another running process's unsaved branch selection."
        );
      }
      const content = live
        ? [live.getHeader(), ...live.getEntries()]
            .map((entry) => JSON.stringify(entry))
            .join("\n") + "\n"
        : readFileSync(source.transcript_file!, "utf8");
      const parsed = transcript(content, source.native_session_id);
      const manager = live ?? SessionManager.inMemory(parsed.entries[0].cwd, undefined, parsed.entries);
      if (parsed.partial) {
        sample.warnings.push(
          "Incomplete final JSONL record excluded from projected evidence; raw snapshot retained."
        );
      }
      const evidence = structuredClone({
        messages: manager.buildSessionProjection().messages,
        leafId: manager.getLeafId(),
      });
      const snapshot_file = `session-${index}.jsonl`;
      const metadata = {
        cwd: manager.getCwd(),
        projection_source: live ? ("current-session" as const) : ("persisted-leaf" as const),
        evidence,
        evidence_hash: digest(JSON.stringify(evidence)),
        snapshot_file,
        snapshot_hash: digest(content),
      };
      privateWrite(join(directory, snapshot_file), content, root);
      Object.assign(sample, metadata);
      for (const entry of parsed.entries) {
        if (entry.type !== "custom" || entry.customType !== "stream-helper-session") continue;
        const link = entry.data;
        if (!object(link) || !identity(link.session_id) || !identity(link.session_file)) {
          sample.warnings.push("Malformed helper log link omitted.");
          continue;
        }
        const log: FrozenSample["helper_logs"][number] = { session_id: link.session_id };
        sample.helper_logs.push(log);
        try {
          if (!helperRoot || resolve(dirname(link.session_file)) !== resolve(helperRoot)) {
            throw new Error("Helper log must be within the reporting sessions root");
          }
          const raw = privateRead(helperRoot, basename(link.session_file), helperRoot);
          const helper = transcript(raw, link.session_id);
          if (
            !helper.entries.some(
              (e) =>
                e.type === "custom" &&
                e.customType === "stream-helper-request" &&
                e.data?.parent_session_id === source.native_session_id
            )
          ) {
            throw new Error("Helper log parent identity mismatch");
          }
          const file = `session-${index}-helper-${sample.helper_logs.length}.jsonl`;
          privateWrite(join(directory, file), raw, root);
          log.file = file;
        } catch (error) {
          log.error = String(error);
        }
      }
    } catch (error) {
      sample.unavailable = String(error);
      delete sample.evidence;
    }
  }
  signal?.throwIfAborted();
  privateWrite(join(directory, "capture.json"), JSON.stringify(capture, null, 2) + "\n", root);
  return { capture, directory };
}

const captureId = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/;

export function captureIds(root: string): string[] {
  let entries: string[];
  try {
    entries = readdirSync(privateDirectory(join(root, "captures"), root, false));
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return [];
    throw error;
  }
  return entries
    .filter((id) => {
      if (!captureId.test(id)) return false;
      try {
        const directory = join(root, "captures", id);
        return (
          !lstatSync(directory).isSymbolicLink() &&
          !lstatSync(join(directory, "capture.json")).isSymbolicLink() &&
          lstatSync(join(directory, "capture.json")).isFile()
        );
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code === "ENOENT") return false;
        throw error;
      }
    })
    .sort();
}

export function loadCapture(root: string, id: string): Capture {
  if (!captureId.test(id)) throw new Error("Choose an exact capture ID");
  const directory = join(root, "captures", id);
  const value = JSON.parse(privateRead(directory, "capture.json", root));
  if (
    !object(value) ||
    value.version !== 2 ||
    value.id !== id ||
    !["trimmed", "full"].includes(value.context_mode) ||
    !identity(value.captured_at) ||
    !object(value.source_files) ||
    !Array.isArray(value.samples)
  ) {
    throw new Error("Invalid frozen capture");
  }
  parseCaptureInventory({ ...value, streams: [] });
  for (const sample of value.samples) {
    if (
      !Array.isArray(sample.warnings) ||
      !Array.isArray(sample.helper_logs) ||
      !(
        sample.evidence === undefined ||
        (object(sample.evidence) &&
          Array.isArray(sample.evidence.messages) &&
          (sample.evidence.leafId === null || identity(sample.evidence.leafId)))
      )
    ) {
      throw new Error("Invalid frozen evidence");
    }
    if (sample.evidence && digest(JSON.stringify(sample.evidence)) !== sample.evidence_hash) {
      throw new Error("Frozen evidence hash mismatch");
    }
    if (
      sample.evidence &&
      (sample.unavailable !== null ||
        !identity(sample.snapshot_file) ||
        !/^session-[0-9]+[.]jsonl$/.test(sample.snapshot_file) ||
        !identity(sample.cwd) ||
        !["current-session", "persisted-leaf"].includes(sample.projection_source))
    ) {
      throw new Error("Frozen evidence missing source identity");
    }
    if (sample.snapshot_file !== undefined) {
      const raw = privateRead(directory, sample.snapshot_file, root);
      if (digest(raw) !== sample.snapshot_hash) throw new Error("Frozen transcript hash mismatch");
      const parsed = transcript(raw, sample.native_session_id);
      if (parsed.entries[0].cwd !== sample.cwd) throw new Error("Frozen transcript cwd mismatch");
      if (
        sample.evidence?.leafId &&
        !parsed.entries.some((entry) => entry.id === sample.evidence.leafId)
      ) {
        throw new Error("Frozen transcript leaf identity mismatch");
      }
    }
    for (const log of sample.helper_logs) {
      if (
        !object(log) ||
        !identity(log.session_id) ||
        !(log.error === undefined || typeof log.error === "string")
      ) {
        throw new Error("Invalid frozen helper log");
      }
      if (log.file !== undefined) {
        if (!identity(log.file)) throw new Error("Invalid frozen helper log file");
        transcript(privateRead(directory, log.file, root), log.session_id);
      }
    }
  }
  return value as Capture;
}
