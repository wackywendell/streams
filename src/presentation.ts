import { resolve } from 'node:path';
import { goalText, referenceIdentity, validateAgent, validateMembership, validateWorkspace, type AgentRecord, type Snapshot, type WorkspaceRecord, type ReferenceSelection } from './records/schema.js';

/** Labels are presentation only; callers retain checkout keys and absolute targets. */
export function checkoutLabels(checkouts: WorkspaceRecord['checkouts']): Record<string, string> {
  const entries = Object.entries(checkouts).map(([key, checkout]) => ({ key, directory: checkout.directory, parts: checkout.directory.split('/').filter(Boolean), depth: 1 }));
  const label = (entry: typeof entries[number]): string => entry.depth > entry.parts.length ? entry.directory : entry.parts.slice(-entry.depth).join('/');
  for (;;) {
    const collisions = entries.filter(e => entries.some(other => other !== e && label(other) === label(e)));
    if (!collisions.length) return Object.fromEntries(entries.map(e => [e.key, label(e) || '/']));
    if (collisions.some(e => e.depth > e.parts.length)) throw new Error('Duplicate checkout directory');
    for (const entry of collisions) entry.depth++;
  }
}

export function resolvedReferences(workspace: WorkspaceRecord) {
  const labels = checkoutLabels(workspace.checkouts);
  const qualify = Object.keys(labels).length > 1;
  return {
    documents: workspace.documents.map(ref => ({
      ...ref, identity: referenceIdentity(ref), target: resolve(workspace.checkouts[ref.checkout].directory, ref.path),
      label: qualify ? `${labels[ref.checkout] === '/' ? '' : labels[ref.checkout]}/${ref.path}` : ref.path,
    })),
    issues: workspace.issues.map(ref => ({ ...ref, identity: referenceIdentity(ref) })),
    prs: workspace.prs.map(ref => ({ ...ref, identity: referenceIdentity(ref) })),
  };
}

export function composeView(workspace: Snapshot<WorkspaceRecord>, agent: Snapshot<AgentRecord>) {
  const w = validateWorkspace(workspace.record); const a = validateAgent(agent.record);
  validateMembership(w, a);
  return {
    workspaceId: w.id, sessionId: a.sessionId, workspaceTitle: w.title, title: a.title,
    goal: goalText(w.goal), status: a.status, now: a.now, remaining: a.remaining,
    ...resolvedReferences(w),
  };
}

export type ReferenceMatch = { identity: string; target: string; label: string };
export type ReferenceSelectionResult =
  | { kind: 'empty' | 'none' }
  | { kind: 'ambiguous'; candidates: ReferenceMatch[] }
  | { kind: 'unique'; selection: ReferenceSelection; candidate: ReferenceMatch };

/** Literal matching against stored targets, never descriptions or filesystem existence. */
export function selectReference(snapshot: Snapshot<WorkspaceRecord>, substring: string): ReferenceSelectionResult {
  if (typeof substring !== 'string') throw new Error('Substring must be a string');
  if (!substring.length) return { kind: 'empty' };
  const workspace = validateWorkspace(snapshot.record); const refs = resolvedReferences(workspace);
  const candidates: ReferenceMatch[] = [
    ...refs.documents.map(ref => ({ identity: ref.identity, target: ref.target, label: ref.label })),
    ...[...refs.issues, ...refs.prs].map(ref => ({ identity: ref.identity, target: ref.url, label: ref.url })),
  ].filter(ref => ref.target.includes(substring));
  if (!candidates.length) return { kind: 'none' };
  if (candidates.length !== 1) return { kind: 'ambiguous', candidates };
  return { kind: 'unique', candidate: candidates[0], selection: { workspaceId: workspace.id, revision: snapshot.revision, identity: candidates[0].identity } };
}
