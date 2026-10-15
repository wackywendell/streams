import { createHash } from 'node:crypto';
import { constants } from 'node:fs';
import { open, stat } from 'node:fs/promises';
import { documentIdentity, referenceIdentity, resolveWorkspaceDocument, validateWorkspace, type DocumentIdentity, type WorkspaceRecord } from '../records.js';

export const MAX_PLAN_SOURCE_BYTES = 256 * 1024;
export interface PlanSource {
  readonly identity: Readonly<DocumentIdentity>;
  readonly directory: string;
  readonly target: string;
  readonly content: string;
  readonly bytesFingerprint: string;
}
export interface PlanSourceCapture {
  readonly sources: readonly PlanSource[];
  readonly fingerprint: string;
}

export function isMarkdownDocument(identity: DocumentIdentity): boolean {
  return /\.md$/i.test(identity.path);
}

/** Selection never discovers files: every identity must still be a visible Markdown reference. */
export function validatePlanSelection(workspace: WorkspaceRecord, selected: readonly DocumentIdentity[]): DocumentIdentity[] {
  if (!Array.isArray(selected)) throw new Error('Expected selected plan sources array');
  const current = validateWorkspace(workspace);
  const seen = new Set<string>();
  return Array.from(selected, value => {
    const identity = documentIdentity(value);
    const key = referenceIdentity(identity);
    if (!isMarkdownDocument(identity)) throw new Error('Plan source must be Markdown (.md)');
    if (!current.documents.some(d => referenceIdentity(d) === key)) throw new Error('Plan source must be a currently visible document');
    if (seen.has(key)) throw new Error('Duplicate plan source');
    seen.add(key);
    return identity;
  });
}

function digest(bytes: Buffer | string): string { return createHash('sha256').update(bytes).digest('hex'); }

async function readSource(workspace: WorkspaceRecord, identity: DocumentIdentity): Promise<PlanSource> {
  try {
    const directory = workspace.checkouts[identity.checkout].directory;
    const target = await resolveWorkspaceDocument(workspace, identity);
    const file = await open(target, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
    try {
      const before = await file.stat({ bigint: true });
      if (!before.isFile()) throw new Error('Plan source must be a regular file');
      if (before.size > BigInt(MAX_PLAN_SOURCE_BYTES)) throw new Error('Plan source exceeds 256 KiB limit');
      // Read at most limit+1 even if the source grows after fstat. Never expose a truncated plan.
      const buffer = Buffer.alloc(MAX_PLAN_SOURCE_BYTES + 1);
      let length = 0;
      while (length < buffer.length) {
        const { bytesRead } = await file.read(buffer, length, buffer.length - length, length);
        if (!bytesRead) break;
        length += bytesRead;
      }
      if (length > MAX_PLAN_SOURCE_BYTES) throw new Error('Plan source exceeds 256 KiB limit');
      const after = await file.stat({ bigint: true });
      const currentTarget = await resolveWorkspaceDocument(workspace, identity);
      const current = await stat(currentTarget, { bigint: true });
      const sameFile = (a: typeof before, b: typeof before) => a.dev === b.dev && a.ino === b.ino
        && a.size === b.size && a.mtimeNs === b.mtimeNs && a.ctimeNs === b.ctimeNs;
      if (currentTarget !== target || !sameFile(before, after) || !sameFile(after, current)) throw new Error('Plan source changed during read');
      const bytes = buffer.subarray(0, length);
      const content = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
      return Object.freeze({ identity: Object.freeze({ ...identity }), directory, target, content, bytesFingerprint: digest(bytes) });
    } finally { await file.close(); }
  } catch (error) {
    throw new Error(`Cannot read plan source ${identity.checkout}/${identity.path}: ${(error as Error).message}`, { cause: error });
  }
}

/** Immutable provenance includes exact selected identities, registered roots, canonical targets and bytes. */
export async function capturePlanSources(workspace: WorkspaceRecord, selected: readonly DocumentIdentity[]): Promise<PlanSourceCapture> {
  const current = validateWorkspace(workspace);
  const identities = validatePlanSelection(current, selected);
  const sources: PlanSource[] = [];
  for (const identity of identities) sources.push(await readSource(current, identity));
  const fingerprint = digest(JSON.stringify(sources.map(s => [s.identity.checkout, s.identity.path, s.directory, s.target, s.bytesFingerprint])));
  return Object.freeze({ sources: Object.freeze(sources), fingerprint });
}

/** Runtime must supply CURRENT selection/workspace, then apply with fresh revision and authority. No cached proof. */
export async function recheckPlanSources(
  workspace: WorkspaceRecord, selected: readonly DocumentIdentity[], captured: PlanSourceCapture,
): Promise<PlanSourceCapture> {
  const fresh = await capturePlanSources(workspace, selected);
  if (fresh.fingerprint !== captured.fingerprint) throw new Error('Selected plan sources changed during inference');
  return fresh;
}
