import { basename, dirname, isAbsolute, relative, resolve, sep } from "node:path";
import { realpathSync } from "node:fs";

export type Status = "active" | "waiting" | "blocked" | "parked" | "done" | "closed";
export type Mode = "automatic" | "manual";
export type NamingSettings = { stream: Mode; session: Mode };

export type Title = { text: string; manual: boolean };
export type Goal = { text: string; short: string | null; manual: boolean };
export type TextPair = { text: string; short: string | null };

export type RepositoryInfo = {
  url: string | null;
  within: string | null;
  branch: string | null;
};

export type SessionDirectory = {
  path: string;
  repository: RepositoryInfo | null;
};

export type ReferenceKind = "document" | "pr" | "issue" | "link";
export type ReferenceRole = "deliverable" | "dependency" | "brief" | "background";
export type ReferenceStatus = "active" | "fulfilled" | "dropped";

export type ReferenceEntry = {
  kind: ReferenceKind;
  name: string;
  purpose: string;
  role: ReferenceRole;
  status: ReferenceStatus;
  manual: boolean;
};

export type ArtifactEntry = {
  description: string;
};

export type DocumentFileState =
  | { status: "available"; path: string }
  | { status: "unavailable"; reason: string };

export type UnreadableRecord = {
  kind: "host" | "stream" | "session";
  host: string;
  path: string;
  cause: string;
  repair: string;
};

export type StreamWire = {
  key?: string;
  host_id: string;
  id: string;
  title: Title;
  goal: Goal | null;
  status?: Status;
  sessions?: SessionWire[];
  unreadable?: UnreadableRecord[];
  last_activity_ms?: number;
  revision: number;
  updated_at_ms: number;
};

export type SessionWire = {
  key?: string;
  host_id: string;
  harness: string;
  id: string;
  stream_id: string;
  title: Title;
  status: Status;
  recap: TextPair | null;
  state: TextPair | null;
  remaining: TextPair | null;
  directory: SessionDirectory | null;
  references: Record<string, ReferenceEntry>;
  artifacts: Record<string, ArtifactEntry>;
  document_files?: Record<string, DocumentFileState>;
  revision: number;
  updated_at_ms: number;
};

export type LocalContext = {
  session: SessionWire | null;
  stream: StreamWire | null;
  naming: NamingSettings;
};

export type EditBody = {
  changes: Record<string, unknown>;
  before?: Record<string, unknown>;
};

export type EditResult = {
  key: string;
  not_written: Record<string, unknown>;
  partial?: boolean;
};

const object = (value: unknown): value is Record<string, unknown> =>
  !!value && typeof value === "object" && !Array.isArray(value);
const text = (value: unknown): value is string =>
  typeof value === "string" && !value.includes("\0");
const identity = (value: unknown): value is string =>
  text(value) && value.trim().length > 0;
const nullableText = (value: unknown): value is string | null =>
  value === null || text(value);
const mode = (value: unknown): value is Mode =>
  value === "automatic" || value === "manual";
const count = (value: unknown): value is number =>
  Number.isSafeInteger(value) && (value as number) >= 0;
const stamped = (value: Record<string, unknown>) =>
  count(value.revision) && count(value.updated_at_ms);
const status = (value: unknown): value is Status =>
  typeof value === "string" &&
  ["active", "waiting", "blocked", "parked", "done", "closed"].includes(value);

export function isReferenceText(value: unknown): value is string {
  return (
    typeof value === "string" &&
    !value.includes("\0") &&
    !/[\ud800-\udfff]/u.test(value) &&
    /[^\u0009-\u000d\u0020\u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]/u.test(
      value
    )
  );
}

const onlyFields = (value: Record<string, unknown>, fields: readonly string[]) =>
  Object.keys(value).every((key) => fields.includes(key));

export function isPathKey(value: unknown): value is string {
  if (!isReferenceText(value)) return false;
  if (/[\x00-\x1f\x7f]/.test(value)) return false;
  if (value.startsWith("/") || value.includes("\\")) return false;
  const segments = value.split("/");
  if (segments[0]?.includes(":")) return false;
  return segments.every((seg) => seg.length > 0 && seg !== "." && seg !== "..");
}

export function referenceUrl(value: string): URL | undefined {
  if (!isReferenceText(value) || /[\\\p{White_Space}\p{Cc}\p{Cf}]/u.test(value)) {
    return undefined;
  }
  try {
    if (/[\p{Cc}\p{Cf}]/u.test(decodeURIComponent(value))) return undefined;
    const url = new URL(value);
    return ["http:", "https:"].includes(url.protocol) &&
      url.host &&
      !url.username &&
      !url.password
      ? url
      : undefined;
  } catch {
    return undefined;
  }
}

export function isTitle(value: unknown): value is Title {
  return (
    object(value) &&
    onlyFields(value, ["text", "manual"]) &&
    isReferenceText(value.text) &&
    typeof value.manual === "boolean"
  );
}

export function isGoal(value: unknown): value is Goal {
  return (
    object(value) &&
    onlyFields(value, ["text", "short", "manual"]) &&
    isReferenceText(value.text) &&
    (value.short === null || isReferenceText(value.short)) &&
    typeof value.manual === "boolean"
  );
}

export function isTextPair(value: unknown): value is TextPair {
  return (
    object(value) &&
    onlyFields(value, ["text", "short"]) &&
    isReferenceText(value.text) &&
    (value.short === null || isReferenceText(value.short))
  );
}

export function isRepositoryInfo(value: unknown): value is RepositoryInfo {
  return (
    object(value) &&
    onlyFields(value, ["url", "within", "branch"]) &&
    (value.url === null || isReferenceText(value.url)) &&
    (value.within === null || isReferenceText(value.within)) &&
    (value.branch === null || isReferenceText(value.branch))
  );
}

export function isSessionDirectory(value: unknown): value is SessionDirectory {
  return (
    object(value) &&
    onlyFields(value, ["path", "repository"]) &&
    isReferenceText(value.path) &&
    (value.repository === null || isRepositoryInfo(value.repository))
  );
}

export function isReferenceEntry(value: unknown): value is ReferenceEntry {
  return (
    object(value) &&
    onlyFields(value, ["kind", "name", "purpose", "role", "status", "manual"]) &&
    ["document", "pr", "issue", "link"].includes(value.kind as string) &&
    isReferenceText(value.name) &&
    isReferenceText(value.purpose) &&
    ["deliverable", "dependency", "brief", "background"].includes(value.role as string) &&
    ["active", "fulfilled", "dropped"].includes(value.status as string) &&
    typeof value.manual === "boolean"
  );
}

export function isValidReferencePair(key: string, entry: ReferenceEntry): boolean {
  if (entry.kind === "document") return isPathKey(key);
  return referenceUrl(key) !== undefined;
}

export function isArtifactEntry(value: unknown): value is ArtifactEntry {
  return (
    object(value) &&
    onlyFields(value, ["description"]) &&
    isReferenceText(value.description)
  );
}

export function isUnreadableRecord(value: unknown): value is UnreadableRecord {
  return (
    object(value) &&
    ["host", "stream", "session"].includes(value.kind as string) &&
    text(value.host) &&
    text(value.path) &&
    text(value.cause) &&
    text(value.repair)
  );
}

export function isStream(value: unknown): value is StreamWire {
  if (
    !object(value) ||
    !identity(value.host_id) ||
    !identity(value.id) ||
    !isTitle(value.title) ||
    !(value.goal === null || isGoal(value.goal)) ||
    !stamped(value)
  ) {
    return false;
  }
  if (value.status !== undefined && !status(value.status)) return false;
  if (value.key !== undefined && !identity(value.key)) return false;
  if (
    value.sessions !== undefined &&
    (!Array.isArray(value.sessions) ||
      !value.sessions.every(
        (item) =>
          object(item) &&
          identity(item.harness) &&
          isTitle(item.title) &&
          status(item.status)
      ))
  ) {
    return false;
  }
  if (
    value.unreadable !== undefined &&
    (!Array.isArray(value.unreadable) || !value.unreadable.every(isUnreadableRecord))
  ) {
    return false;
  }
  if (value.last_activity_ms !== undefined && !count(value.last_activity_ms)) {
    return false;
  }
  return true;
}

export function isSession(value: unknown): value is SessionWire {
  if (
    !object(value) ||
    !identity(value.host_id) ||
    !identity(value.harness) ||
    !identity(value.id) ||
    !identity(value.stream_id) ||
    !isTitle(value.title) ||
    !status(value.status) ||
    !(value.recap === null || isTextPair(value.recap)) ||
    !(value.state === null || isTextPair(value.state)) ||
    !(value.remaining === null || isTextPair(value.remaining)) ||
    !(value.directory === null || isSessionDirectory(value.directory)) ||
    !object(value.references) ||
    !object(value.artifacts) ||
    !stamped(value)
  ) {
    return false;
  }
  if (value.key !== undefined && !identity(value.key)) return false;
  for (const [refKey, entry] of Object.entries(value.references)) {
    if (!isReferenceEntry(entry) || !isValidReferencePair(refKey, entry)) return false;
  }
  for (const [name, entry] of Object.entries(value.artifacts)) {
    if (!identity(name) || !isArtifactEntry(entry)) return false;
  }
  return true;
}

export function streamKeyOf(stream: Pick<StreamWire, "host_id" | "id" | "key">): string {
  return stream.key ?? `${stream.host_id}/${stream.id}`;
}

export function sessionKeyOf(
  session: Pick<SessionWire, "host_id" | "harness" | "id" | "key">
): string {
  return session.key ?? `${session.host_id}/${session.harness}/${session.id}`;
}

export function parseLocalContext(value: unknown): LocalContext {
  if (
    !object(value) ||
    !(value.session === null || isSession(value.session)) ||
    !(value.stream === null || isStream(value.stream)) ||
    !object(value.naming) ||
    !mode(value.naming.stream) ||
    !mode(value.naming.session)
  ) {
    throw new Error("Invalid local catalog context");
  }
  const session = value.session as SessionWire | null;
  const stream = value.stream as StreamWire | null;
  if (session && (!stream || session.host_id !== stream.host_id || session.stream_id !== stream.id)) {
    throw new Error("Inconsistent addressed catalog context");
  }
  return value as LocalContext;
}

/**
 * Turn a raw document path (relative or absolute) into a relative path key under `directoryPath`.
 * Refuses `..`, paths outside `directoryPath`, and invalid path keys.
 * Resolves the longest existing ancestor of the parent directory via `realpathSync` when direct
 * prefix stripping fails (so macOS `/var` vs `/private/var` works), keeping the final name unchanged.
 */
export function normalizeDocumentKey(
  directoryPath: string | null | undefined,
  rawPath: string
): string | null {
  if (!isReferenceText(rawPath)) return null;
  if (!directoryPath || !isAbsolute(directoryPath)) return null;
  if (rawPath.includes("\\")) return null;
  if (!isAbsolute(rawPath)) {
    const trimmed = rawPath.replace(/^(?:\.\/)+/, "");
    return isPathKey(trimmed) ? trimmed : null;
  }
  const rawSegments = rawPath.split("/");
  if (rawSegments.includes("..")) return null;

  const tryStrip = (base: string, candidate: string): string | null => {
    const rel = relative(base, candidate);
    if (!rel || rel === "." || rel.startsWith("..") || isAbsolute(rel)) return null;
    const normalized = sep === "/" ? rel : rel.split(sep).join("/");
    return isPathKey(normalized) ? normalized : null;
  };

  const direct = tryStrip(directoryPath, rawPath);
  if (direct) return direct;

  // Canonicalize base and the longest existing ancestor of parent, keeping final filename intact.
  let canonicalBase = directoryPath;
  try {
    canonicalBase = realpathSync(directoryPath);
  } catch {
    // Keep original directoryPath if it does not exist on disk.
  }

  const fileName = basename(rawPath);
  let cursor = dirname(rawPath);
  const tail: string[] = [fileName];
  while (cursor && cursor !== dirname(cursor)) {
    try {
      const resolvedAncestor = realpathSync(cursor);
      const rebuilt = resolve(resolvedAncestor, ...tail.reverse());
      return tryStrip(canonicalBase, rebuilt);
    } catch {
      tail.push(basename(cursor));
      cursor = dirname(cursor);
    }
  }
  return null;
}

export { nullableText };
