import { isDeepStrictEqual } from "node:util";
import {
  isReferenceText,
  normalizeDocumentKey,
  REFERENCE_KINDS,
  REFERENCE_ROLES,
  REFERENCE_STATUSES,
  referenceUrl,
  sessionKeyOf,
  streamKeyOf,
  type EditBody,
  type EditResult,
  type Goal,
  type LocalContext,
  type NamingSettings,
  type RawReferenceProposal,
  type ReferenceEntry,
  type SessionModelChanges,
  type SessionWire,
  type StreamModelChanges,
  type StreamWire,
  type TextPair,
  type TextPairInput,
  type Title,
} from "./wire.ts";
import { referenceText, type StreamCli } from "./streams.ts";

export type { RawReferenceProposal, SessionModelChanges, StreamModelChanges, TextPairInput };

export function canAutoEditStreamTitle(stream: StreamWire, naming: NamingSettings): boolean {
  return !stream.title.manual && naming.stream === "automatic";
}

export function canAutoEditStreamGoal(stream: StreamWire): boolean {
  return !stream.goal?.manual;
}

export function canAutoEditSessionTitle(session: SessionWire, naming: NamingSettings): boolean {
  return !session.title.manual && naming.session === "automatic";
}

export function automaticStreamFieldNames(context: LocalContext): (keyof StreamModelChanges)[] {
  if (!context.stream) return [];
  const fields: (keyof StreamModelChanges)[] = [];
  if (canAutoEditStreamTitle(context.stream, context.naming)) fields.push("title");
  if (canAutoEditStreamGoal(context.stream)) fields.push("goal");
  return fields;
}

export function automaticSessionFieldNames(context: LocalContext): (keyof SessionModelChanges)[] {
  if (!context.session) return [];
  const fields: (keyof SessionModelChanges)[] = [];
  if (canAutoEditSessionTitle(context.session, context.naming)) fields.push("title");
  fields.push("status", "recap", "next");
  if (context.session.directory?.repository) fields.push("branch");
  return fields;
}

function cleanText(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  return trimmed.length > 0 && isReferenceText(trimmed) ? trimmed : null;
}

function textPairFromInput(
  current: TextPair | null,
  input: TextPairInput | null | undefined,
  clearable: boolean
): TextPair | null | undefined {
  if (input === undefined) return undefined;
  if (input === null) return clearable ? null : undefined;
  const nextText = cleanText(input.text);
  if (!nextText) return clearable ? null : undefined;
  const hasShort = Object.hasOwn(input, "short") && input.short !== undefined;
  const nextShort = hasShort
    ? cleanText(input.short)
    : nextText !== current?.text
      ? null
      : (current?.short ?? null);
  return { text: nextText, short: nextShort };
}

/**
 * Filter model Session proposals against format-6 rules (`docs/session-card-fields.md`),
 * producing a single `session edit --input -` body with exact `before` values.
 */
export function buildSessionReportEdit(
  session: SessionWire,
  naming: NamingSettings,
  rawChanges: SessionModelChanges,
  rawProposals: unknown,
  notices: string[] = []
): EditBody | null {
  const changes: Record<string, unknown> = {};
  const before: Record<string, unknown> = {};

  // Title (respecting manual mark and naming.session setting)
  if (Object.hasOwn(rawChanges, "title") && canAutoEditSessionTitle(session, naming)) {
    const nextText = cleanText(rawChanges.title);
    if (nextText && nextText !== session.title.text) {
      const next: Title = { text: nextText, manual: false };
      changes.title = next;
      before.title = structuredClone(session.title);
    }
  }

  // Status
  if (
    Object.hasOwn(rawChanges, "status") &&
    typeof rawChanges.status === "string" &&
    rawChanges.status !== session.status
  ) {
    changes.status = rawChanges.status;
    before.status = session.status;
  }

  // TextPair fields: recap cannot be cleared by automatic reports; next can.
  for (const field of ["recap", "next"] as const) {
    if (!Object.hasOwn(rawChanges, field)) continue;
    const next = textPairFromInput(session[field], rawChanges[field], field === "next");
    if (next !== undefined && !isDeepStrictEqual(next, session[field])) {
      changes[field] = next;
      before[field] = structuredClone(session[field]);
    }
  }

  // Branch update inside directory.repository (non-clearable: omit if unchanged)
  if (Object.hasOwn(rawChanges, "branch") && session.directory?.repository) {
    const nextBranch = cleanText(rawChanges.branch);
    if (nextBranch && nextBranch !== session.directory.repository.branch) {
      changes.directory = {
        path: session.directory.path,
        repository: {
          ...session.directory.repository,
          branch: nextBranch,
        },
      };
      before.directory = structuredClone(session.directory);
    }
  }

  // Reference proposals
  if (Array.isArray(rawProposals) && rawProposals.length > 0) {
    const refChanges: Record<string, ReferenceEntry> = {};
    const refBefore: Record<string, ReferenceEntry | null> = {};
    const seen = new Set<string>();

    for (const raw of rawProposals) {
      if (!raw || typeof raw !== "object" || Array.isArray(raw)) continue;
      const proposal = raw as Partial<RawReferenceProposal>;
      const kind = proposal.kind;
      if (!kind || !(REFERENCE_KINDS as readonly string[]).includes(kind)) continue;
      if (typeof proposal.key !== "string" || !proposal.key.trim()) continue;
      const name = cleanText(proposal.name);
      const purpose = cleanText(proposal.purpose);
      const role = proposal.role;
      const status = proposal.status;
      if (
        !name ||
        !purpose ||
        !role ||
        !(REFERENCE_ROLES as readonly string[]).includes(role) ||
        !status ||
        !(REFERENCE_STATUSES as readonly string[]).includes(status)
      ) {
        continue;
      }

      let normalizedKey: string;
      if (kind === "document") {
        const key = normalizeDocumentKey(session.directory?.path, proposal.key.trim());
        if (!key) {
          notices.push(
            `Skipped document outside Session directory (${referenceText(proposal.key)}); save external outputs with the artifact tool instead.`
          );
          continue;
        }
        normalizedKey = key;
      } else {
        const trimmed = proposal.key.trim();
        if (!referenceUrl(trimmed)) continue;
        normalizedKey = trimmed;
      }

      // Keep the first proposal for each key.
      if (seen.has(normalizedKey)) continue;
      seen.add(normalizedKey);

      const existing = session.references[normalizedKey] ?? null;

      // Never touch a dropped entry or re-add a dropped key.
      if (existing?.status === "dropped") continue;

      let desired: ReferenceEntry;
      if (existing?.manual) {
        // Leave manual name, purpose, role alone; automatic report may only update status.
        if (status === existing.status) continue;
        desired = {
          kind: existing.kind,
          name: existing.name,
          purpose: existing.purpose,
          role: existing.role,
          status,
          manual: true,
        };
      } else {
        desired = {
          kind,
          name,
          purpose,
          role,
          status,
          manual: false,
        };
        if (existing && isDeepStrictEqual(existing, desired)) continue;
      }

      refChanges[normalizedKey] = desired;
      refBefore[normalizedKey] = existing ? structuredClone(existing) : null;
    }

    if (Object.keys(refChanges).length > 0) {
      changes.references = refChanges;
      before.references = refBefore;
    }
  }

  if (Object.keys(changes).length === 0) return null;
  return { changes, before };
}

/**
 * Filter model Stream proposals against format-6 rules, producing a single
 * `stream edit --input -` body with exact `before` values.
 */
export function buildStreamReportEdit(
  stream: StreamWire,
  naming: NamingSettings,
  rawChanges: StreamModelChanges
): EditBody | null {
  const changes: Record<string, unknown> = {};
  const before: Record<string, unknown> = {};

  if (Object.hasOwn(rawChanges, "title") && canAutoEditStreamTitle(stream, naming)) {
    const nextText = cleanText(rawChanges.title);
    if (nextText && nextText !== stream.title.text) {
      changes.title = { text: nextText, manual: false } satisfies Title;
      before.title = structuredClone(stream.title);
    }
  }

  if (Object.hasOwn(rawChanges, "goal") && canAutoEditStreamGoal(stream)) {
    const pair = textPairFromInput(stream.goal, rawChanges.goal, false);
    if (pair) {
      const nextGoal: Goal = { ...pair, manual: false };
      if (!isDeepStrictEqual(nextGoal, stream.goal)) {
        changes.goal = nextGoal;
        before.goal = structuredClone(stream.goal);
      }
    }
  }

  if (Object.keys(changes).length === 0) return null;
  return { changes, before };
}

/** Summarize reference entries that landed (omitted from `not_written.references`). */
export function summarizeWrittenReferences(
  body: EditBody,
  result: EditResult
): string[] {
  const sent = body.changes.references;
  if (!sent || typeof sent !== "object" || Array.isArray(sent)) return [];
  const before = (body.before?.references ?? {}) as Record<string, ReferenceEntry | null>;
  const notWritten = (result.not_written.references ?? {}) as Record<string, unknown>;
  const messages: string[] = [];

  for (const [key, rawEntry] of Object.entries(sent)) {
    if (Object.hasOwn(notWritten, key)) continue;
    const entry = rawEntry as ReferenceEntry;
    const prev = before[key] ?? null;
    if (!prev) {
      messages.push(`Reference added: ${referenceText(entry.name)}`);
    } else if (prev.status !== entry.status) {
      messages.push(`Reference ${entry.status}: ${referenceText(entry.name)}`);
    }
  }
  return messages;
}

/** Build a `before` map for a record matching every key in `changes` (and inside `references`). */
export function buildBeforeForChanges(
  record: StreamWire | SessionWire,
  changes: Record<string, unknown>
): Record<string, unknown> {
  const before: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(changes)) {
    if (key === "references" && value && typeof value === "object" && !Array.isArray(value)) {
      const savedRefs = (record as SessionWire).references ?? {};
      const refBefore: Record<string, unknown> = {};
      for (const refKey of Object.keys(value)) {
        refBefore[refKey] = Object.hasOwn(savedRefs, refKey)
          ? structuredClone(savedRefs[refKey])
          : null;
      }
      before.references = refBefore;
    } else {
      before[key] = structuredClone((record as Record<string, unknown>)[key] ?? null);
    }
  }
  return before;
}

/** RFC 7386 merge matching Rust `operations::edits::merge_value` (null sets null; absent keeps base). */
function mergeValue(base: unknown, patch: unknown): unknown {
  if (
    patch !== null &&
    typeof patch === "object" &&
    !Array.isArray(patch) &&
    base !== null &&
    typeof base === "object" &&
    !Array.isArray(base)
  ) {
    const out: Record<string, unknown> = { ...(base as Record<string, unknown>) };
    for (const [k, v] of Object.entries(patch as Record<string, unknown>)) {
      out[k] = v === null ? null : mergeValue(out[k], v);
    }
    return out;
  }
  return structuredClone(patch);
}

/**
 * Pure in-memory format-6 edit simulation (used by prompt-lab replay).
 * Applies each unit whose saved value matches `before` and reports refused units in `not_written`.
 */
export function applyEditLocally<T extends StreamWire | SessionWire>(
  current: T,
  body: EditBody
): { after: T; result: EditResult } {
  const after = structuredClone(current);
  const notWritten: Record<string, unknown> = {};
  const key =
    "harness" in current
      ? sessionKeyOf(current as SessionWire)
      : streamKeyOf(current as StreamWire);

  for (const [field, change] of Object.entries(body.changes)) {
    if (field === "references" && change && typeof change === "object" && !Array.isArray(change)) {
      const savedRefs = (current as SessionWire).references;
      const afterRefs = (after as SessionWire).references;
      const beforeRefs = body.before?.references as Record<string, unknown> | undefined;
      const refNotWritten: Record<string, unknown> = {};
      for (const [refKey, refChange] of Object.entries(change as Record<string, unknown>)) {
        const curVal = Object.hasOwn(savedRefs, refKey) ? savedRefs[refKey] : null;
        const hasBefore = beforeRefs !== undefined;
        const desired = hasBefore
          ? mergeValue(beforeRefs[refKey], refChange)
          : structuredClone(refChange);
        const admit = !hasBefore || isDeepStrictEqual(curVal, beforeRefs[refKey]);
        if (admit) {
          afterRefs[refKey] = desired as ReferenceEntry;
        }
        const finalVal = Object.hasOwn(afterRefs, refKey) ? afterRefs[refKey] : null;
        if (!isDeepStrictEqual(finalVal, desired)) {
          refNotWritten[refKey] = structuredClone(finalVal);
        }
      }
      if (Object.keys(refNotWritten).length > 0) {
        notWritten.references = refNotWritten;
      }
      continue;
    }

    const curVal = (current as Record<string, unknown>)[field];
    const hasBefore = body.before !== undefined;
    const desired = hasBefore ? mergeValue(body.before![field], change) : structuredClone(change);
    const admit = !hasBefore || isDeepStrictEqual(curVal, body.before![field]);
    if (admit) {
      (after as Record<string, unknown>)[field] = desired;
    }
    const finalVal = (after as Record<string, unknown>)[field];
    if (!isDeepStrictEqual(finalVal, desired)) {
      notWritten[field] = structuredClone(finalVal);
    }
  }

  if (!isDeepStrictEqual(after, current)) {
    after.revision += 1;
    after.updated_at_ms = Date.now();
  }
  return {
    after,
    result: {
      key,
      not_written: notWritten,
      partial: Object.keys(notWritten).length > 0,
    },
  };
}

/** Retained single-field manual edit for the Pi panel. */
export class FieldEdit {
  readonly base: LocalContext;
  value: unknown;
  error?: string;

  constructor(
    readonly kind: "stream" | "session",
    context: LocalContext,
    readonly field: string
  ) {
    this.base = structuredClone(context);
    this.value = structuredClone((context[kind] as Record<string, unknown> | null)?.[field] ?? null);
  }

  get dirty(): boolean {
    return !isDeepStrictEqual(
      this.value,
      (this.base[this.kind] as Record<string, unknown> | null)?.[this.field] ?? null
    );
  }

  refresh(context: LocalContext): void {
    if (!this.dirty && !this.error) {
      Object.assign(this.base, structuredClone(context));
      this.value = structuredClone((context[this.kind] as Record<string, unknown> | null)?.[this.field] ?? null);
    }
  }

  rebase(context: LocalContext): void {
    Object.assign(this.base, structuredClone(context));
    this.error = undefined;
  }

  async save(
    cli: Pick<StreamCli, "editStream" | "editSession">,
    onResult?: (result: EditResult) => void
  ): Promise<EditResult | undefined> {
    try {
      if (!this.dirty) return;
      const record = this.base[this.kind];
      if (!record) throw new Error(`No attached ${this.kind}`);
      const body: EditBody = {
        changes: { [this.field]: this.value },
        before: { [this.field]: structuredClone((record as Record<string, unknown>)[this.field] ?? null) },
      };
      const result =
        this.kind === "stream"
          ? await cli.editStream(streamKeyOf(record as StreamWire), body)
          : await cli.editSession(sessionKeyOf(record as SessionWire), body);
      onResult?.(result);
      if (Object.hasOwn(result.not_written, this.field)) {
        throw new Error(
          `Field ${this.field} changed concurrently. Your input is retained; rebase on the live field or discard.`
        );
      }
      this.error = undefined;
      return result;
    } catch (error) {
      this.error = error instanceof Error ? error.message : String(error);
      throw error;
    }
  }
}
