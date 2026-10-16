import type { ExtensionAPI, ExtensionContext } from '@earendil-works/pi-coding-agent';
import { opaque, text, validateWorkspace, type Snapshot, type WorkspaceRecord } from '../records.js';

export const workspaceNamingEntry = 'work-cards.workspace-naming';
type Session = ExtensionContext['sessionManager'];
type Proof = { version: 1; sessionId: string; workspaceId: string; creationToken: string; title: string; pendingRevision: string };
export type AcceptedWorkspaceTitle = Pick<Proof, 'title' | 'pendingRevision'>;
function validateProof(value: unknown): Proof {
  try {
    if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error();
    const p = value as Record<string, unknown>;
    if (Object.keys(p).sort().join() !== 'creationToken,pendingRevision,sessionId,title,version,workspaceId' || p.version !== 1) throw new Error();
    opaque(p.sessionId); opaque(p.workspaceId); opaque(p.creationToken); text(p.title, true);
    if (typeof p.pendingRevision !== 'string' || !/^[a-f0-9]{64}$/.test(p.pendingRevision)) throw new Error();
    return p as Proof;
  } catch { throw new Error('Malformed workspace naming source; automatic initial naming suspended'); }
}
function latest(manager: Session): Proof | null {
  const entry = manager.getEntries().filter(e => e.type === 'custom' && e.customType === workspaceNamingEntry).at(-1);
  return entry?.type === 'custom' ? validateProof(entry.data) : null;
}
/** Only an actual helper publication after a native baseline can authorize initial takeover. */
export function acceptedWorkspaceTitle(manager: Session, sessionId: string, workspace: Snapshot<WorkspaceRecord>): AcceptedWorkspaceTitle | null {
  const proof = latest(manager), record = validateWorkspace(workspace.record);
  if (!proof || manager.getSessionId() !== sessionId || proof.sessionId !== sessionId || record.goal.kind !== 'known'
    || proof.workspaceId !== record.id || proof.creationToken !== record.creationToken || proof.title !== record.title) return null;
  return { title: proof.title, pendingRevision: proof.pendingRevision };
}
export function acceptWorkspaceTitle(pi: Pick<ExtensionAPI, 'appendEntry'>, manager: Session, sessionId: string, workspace: Snapshot<WorkspaceRecord>, pendingRevision: string, assertCurrent: () => void): void {
  const previous = latest(manager), record = validateWorkspace(workspace.record);
  const proof = validateProof({ version: 1, sessionId, workspaceId: record.id, creationToken: record.creationToken, title: record.title, pendingRevision });
  assertCurrent();
  if (manager.getSessionId() !== sessionId) throw new Error('Stale Pi workspace naming session');
  if (record.goal.kind !== 'known') throw new Error('Workspace naming requires a known Goal');
  if (previous && Object.keys(proof).every(key => previous[key as keyof Proof] === proof[key as keyof Proof])) return;
  pi.appendEntry(workspaceNamingEntry, proof);
}
