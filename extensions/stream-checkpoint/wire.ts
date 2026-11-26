import { basename, dirname, isAbsolute, relative, resolve, sep } from "node:path";
import { realpathSync } from "node:fs";
import { Type, type Static, type TObject, type TSchema } from "typebox";

export const REPORT_STATUSES = ["active", "waiting", "blocked", "parked", "done"] as const;
export const STATUSES = [...REPORT_STATUSES, "closed"] as const;
export const MODES = ["automatic", "manual"] as const;
export const REFERENCE_KINDS = ["document", "pr", "issue", "link"] as const;
export const REFERENCE_ROLES = ["deliverable", "dependency", "brief", "background"] as const;
export const REFERENCE_STATUSES = ["active", "fulfilled", "dropped"] as const;

export function literalUnion<const T extends readonly string[]>(
  values: T,
  options?: Record<string, unknown>
) {
  return Type.Union(
    values.map((v) => Type.Literal(v)),
    options
  );
}

const nullable = <T extends TSchema>(schema: T) => Type.Union([schema, Type.Null()]);

export const ReportStatusSchema = literalUnion(REPORT_STATUSES);
export const StatusSchema = literalUnion(STATUSES);
export const ModeSchema = literalUnion(MODES);
export const NamingSettingsSchema = Type.Object(
  {
    stream: ModeSchema,
    session: ModeSchema,
  },
  { additionalProperties: false }
);

export const TitleSchema = Type.Object(
  {
    text: Type.String(),
    manual: Type.Boolean(),
  },
  { additionalProperties: false }
);

export const TextPairSchema = Type.Object(
  {
    text: Type.String(),
    short: nullable(Type.String()),
  },
  { additionalProperties: false }
);

export const GoalSchema = Type.Object(
  {
    text: Type.String(),
    short: nullable(Type.String()),
    manual: Type.Boolean(),
  },
  { additionalProperties: false }
);

export const RepositoryInfoSchema = Type.Object(
  {
    url: nullable(Type.String()),
    within: nullable(Type.String()),
    branch: nullable(Type.String()),
  },
  { additionalProperties: false }
);

export const SessionDirectorySchema = Type.Object(
  {
    path: Type.String(),
    repository: nullable(RepositoryInfoSchema),
  },
  { additionalProperties: false }
);

export const ReferenceKindSchema = literalUnion(REFERENCE_KINDS);
export const ReferenceRoleSchema = literalUnion(REFERENCE_ROLES);
export const ReferenceStatusSchema = literalUnion(REFERENCE_STATUSES);

export const ReferenceEntrySchema = Type.Object(
  {
    kind: ReferenceKindSchema,
    name: Type.String(),
    purpose: Type.String(),
    role: ReferenceRoleSchema,
    status: ReferenceStatusSchema,
    manual: Type.Boolean(),
  },
  { additionalProperties: false }
);

export const ArtifactEntrySchema = Type.Object(
  {
    description: Type.String(),
  },
  { additionalProperties: false }
);

export const DocumentFileStateSchema = Type.Union([
  Type.Object({ path: Type.String() }, { additionalProperties: false }),
  Type.Object({ reason: Type.String() }, { additionalProperties: false }),
]);

export const UnreadableRecordSchema = Type.Object({
  kind: literalUnion(["host", "stream", "session"] as const),
  host: Type.String(),
  path: Type.String(),
  cause: Type.String(),
  repair: Type.String(),
});

export const SessionWireSchema = Type.Object({
  key: Type.Optional(Type.String()),
  host_id: Type.String(),
  harness: Type.String(),
  id: Type.String(),
  stream_id: Type.String(),
  title: TitleSchema,
  status: StatusSchema,
  recap: nullable(TextPairSchema),
  next: nullable(TextPairSchema),
  directory: nullable(SessionDirectorySchema),
  references: Type.Record(Type.String(), ReferenceEntrySchema),
  artifacts: Type.Record(Type.String(), ArtifactEntrySchema),
  document_files: Type.Optional(Type.Record(Type.String(), DocumentFileStateSchema)),
  revision: Type.Integer({ minimum: 0 }),
  updated_at_ms: Type.Integer({ minimum: 0 }),
});

export const StreamSessionSummarySchema = Type.Object({
  key: Type.Optional(Type.String()),
  harness: SessionWireSchema.properties.harness,
  title: SessionWireSchema.properties.title,
  status: SessionWireSchema.properties.status,
  recap: Type.Optional(SessionWireSchema.properties.recap),
  next: Type.Optional(SessionWireSchema.properties.next),
});

export const StreamWireSchema = Type.Object({
  key: Type.Optional(Type.String()),
  host_id: Type.String(),
  id: Type.String(),
  title: TitleSchema,
  goal: nullable(GoalSchema),
  status: Type.Optional(StatusSchema),
  sessions: Type.Optional(Type.Array(StreamSessionSummarySchema)),
  unreadable: Type.Optional(Type.Array(UnreadableRecordSchema)),
  last_activity_ms: Type.Optional(Type.Integer({ minimum: 0 })),
  revision: Type.Integer({ minimum: 0 }),
  updated_at_ms: Type.Integer({ minimum: 0 }),
});

export const LocalContextSchema = Type.Object({
  session: nullable(SessionWireSchema),
  stream: nullable(StreamWireSchema),
  naming: NamingSettingsSchema,
});

/** Model-facing `{ text, short? }` input derived directly from `TextPairSchema`. */
export function textPairInputSchema(descriptions?: { text?: string; short?: string }) {
  return Type.Object(
    {
      text: descriptions?.text
        ? Type.String({ description: descriptions.text })
        : TextPairSchema.properties.text,
      short: Type.Optional(
        descriptions?.short
          ? nullable(Type.String({ description: descriptions.short }))
          : TextPairSchema.properties.short
      ),
    },
    { additionalProperties: false }
  );
}

export const TextPairInputSchema = textPairInputSchema();

/** Model-facing reference proposal derived directly from `ReferenceEntrySchema` (omits `manual`, adds `key`). */
export function referenceProposalSchema(descriptions?: {
  key?: string;
  name?: string;
  purpose?: string;
}) {
  const { manual: _manual, ...entryProps } = ReferenceEntrySchema.properties;
  return Type.Object(
    {
      key: Type.String(descriptions?.key ? { description: descriptions.key } : undefined),
      ...entryProps,
      ...(descriptions?.name ? { name: Type.String({ description: descriptions.name }) } : {}),
      ...(descriptions?.purpose
        ? { purpose: Type.String({ description: descriptions.purpose }) }
        : {}),
    },
    { additionalProperties: false }
  );
}

export const ReferenceProposalSchema = referenceProposalSchema();

/**
 * Automatic Session report delta schema, derived from `SessionWireSchema` / `TitleSchema` /
 * `TextPairSchema` / `RepositoryInfoSchema`. Only `next` is nullable (clearable).
 */
export const SessionModelChangesSchema = Type.Object(
  {
    title: Type.Optional(TitleSchema.properties.text),
    status: Type.Optional(ReportStatusSchema),
    recap: Type.Optional(TextPairInputSchema),
    next: Type.Optional(nullable(TextPairInputSchema)),
    branch: Type.Optional(Type.String()),
  },
  { additionalProperties: false }
);

/**
 * Automatic Stream report delta schema, derived from `StreamWireSchema` / `TitleSchema` /
 * `GoalSchema` (without `manual`). Non-nullable: omit unchanged fields.
 */
export const StreamModelChangesSchema = Type.Object(
  {
    title: Type.Optional(TitleSchema.properties.text),
    goal: Type.Optional(TextPairInputSchema),
  },
  { additionalProperties: false }
);

export type ReportStatus = Static<typeof ReportStatusSchema>;
export type Status = Static<typeof StatusSchema>;
export type Mode = Static<typeof ModeSchema>;
export type NamingSettings = Static<typeof NamingSettingsSchema>;

export type Title = Static<typeof TitleSchema>;
export type Goal = Static<typeof GoalSchema>;
export type TextPair = Static<typeof TextPairSchema>;
export type TextPairInput = Static<typeof TextPairInputSchema>;

export type RepositoryInfo = Static<typeof RepositoryInfoSchema>;
export type SessionDirectory = Static<typeof SessionDirectorySchema>;

export type ReferenceKind = Static<typeof ReferenceKindSchema>;
export type ReferenceRole = Static<typeof ReferenceRoleSchema>;
export type ReferenceStatus = Static<typeof ReferenceStatusSchema>;
export type ReferenceEntry = Static<typeof ReferenceEntrySchema>;
export type RawReferenceProposal = Static<typeof ReferenceProposalSchema>;

export type ArtifactEntry = Static<typeof ArtifactEntrySchema>;
export type DocumentFileState = Static<typeof DocumentFileStateSchema>;
export type UnreadableRecord = Static<typeof UnreadableRecordSchema>;
export type StreamWire = Static<typeof StreamWireSchema>;
export type SessionWire = Static<typeof SessionWireSchema>;
export type LocalContext = Static<typeof LocalContextSchema>;

export type SessionModelChanges = Static<typeof SessionModelChangesSchema>;
export type StreamModelChanges = Static<typeof StreamModelChangesSchema>;

export type EditBody = {
  changes: Record<string, unknown>;
  before?: Record<string, unknown>;
};

export type EditResult = {
  key: string;
  not_written: Record<string, unknown>;
  partial?: boolean;
};

const schemaFields = (schema: TObject): readonly string[] => Object.keys(schema.properties);
const TITLE_FIELDS = schemaFields(TitleSchema);
const GOAL_FIELDS = schemaFields(GoalSchema);
const TEXT_PAIR_FIELDS = schemaFields(TextPairSchema);
const REPOSITORY_INFO_FIELDS = schemaFields(RepositoryInfoSchema);
const SESSION_DIRECTORY_FIELDS = schemaFields(SessionDirectorySchema);
const REFERENCE_ENTRY_FIELDS = schemaFields(ReferenceEntrySchema);
const ARTIFACT_ENTRY_FIELDS = schemaFields(ArtifactEntrySchema);

const object = (value: unknown): value is Record<string, unknown> =>
  !!value && typeof value === "object" && !Array.isArray(value);
const text = (value: unknown): value is string =>
  typeof value === "string" && !value.includes("\0");
const identity = (value: unknown): value is string =>
  text(value) && value.trim().length > 0;
const nullableText = (value: unknown): value is string | null =>
  value === null || text(value);
const mode = (value: unknown): value is Mode =>
  typeof value === "string" && (MODES as readonly string[]).includes(value);
const count = (value: unknown): value is number =>
  Number.isSafeInteger(value) && (value as number) >= 0;
const stamped = (value: Record<string, unknown>) =>
  count(value.revision) && count(value.updated_at_ms);
const status = (value: unknown): value is Status =>
  typeof value === "string" && (STATUSES as readonly string[]).includes(value);

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
    onlyFields(value, TITLE_FIELDS) &&
    isReferenceText(value.text) &&
    typeof value.manual === "boolean"
  );
}

export function isGoal(value: unknown): value is Goal {
  return (
    object(value) &&
    onlyFields(value, GOAL_FIELDS) &&
    isReferenceText(value.text) &&
    (value.short === null || isReferenceText(value.short)) &&
    typeof value.manual === "boolean"
  );
}

export function isTextPair(value: unknown): value is TextPair {
  return (
    object(value) &&
    onlyFields(value, TEXT_PAIR_FIELDS) &&
    isReferenceText(value.text) &&
    (value.short === null || isReferenceText(value.short))
  );
}

export function isRepositoryInfo(value: unknown): value is RepositoryInfo {
  return (
    object(value) &&
    onlyFields(value, REPOSITORY_INFO_FIELDS) &&
    (value.url === null || isReferenceText(value.url)) &&
    (value.within === null || isReferenceText(value.within)) &&
    (value.branch === null || isReferenceText(value.branch))
  );
}

export function isSessionDirectory(value: unknown): value is SessionDirectory {
  return (
    object(value) &&
    onlyFields(value, SESSION_DIRECTORY_FIELDS) &&
    isReferenceText(value.path) &&
    (value.repository === null || isRepositoryInfo(value.repository))
  );
}

export function isReferenceEntry(value: unknown): value is ReferenceEntry {
  return (
    object(value) &&
    onlyFields(value, REFERENCE_ENTRY_FIELDS) &&
    (REFERENCE_KINDS as readonly string[]).includes(value.kind as string) &&
    isReferenceText(value.name) &&
    isReferenceText(value.purpose) &&
    (REFERENCE_ROLES as readonly string[]).includes(value.role as string) &&
    (REFERENCE_STATUSES as readonly string[]).includes(value.status as string) &&
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
    onlyFields(value, ARTIFACT_ENTRY_FIELDS) &&
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
          (item.key === undefined || identity(item.key)) &&
          identity(item.harness) &&
          isTitle(item.title) &&
          status(item.status) &&
          (item.recap === undefined || item.recap === null || isTextPair(item.recap)) &&
          (item.next === undefined || item.next === null || isTextPair(item.next))
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
    !(value.next === null || isTextPair(value.next)) ||
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
