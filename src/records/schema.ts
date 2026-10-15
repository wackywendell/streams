import { isAbsolute, normalize, relative, sep } from 'node:path';
export type CardStatus = 'active' | 'waiting' | 'blocked' | 'parked' | 'done';

export type TitleMode = 'auto' | 'manual';
export interface DocumentIdentity { checkout: string; path: string }
export interface WorkspaceDocument extends DocumentIdentity { description: string }
export interface WorkspaceLink { url: string; description: string }
export type ReferenceIdentity = DocumentIdentity | { url: string };
export interface ReferenceAdditions { documents?: WorkspaceDocument[]; issues?: WorkspaceLink[]; prs?: WorkspaceLink[] }
export interface WorkspaceRecord {
  version: 1; id: string; creationToken: string; mutationToken: string; title: string; titleMode: TitleMode;
  goal: string; planSources: DocumentIdentity[]; sourceFingerprint: string | null;
  checkouts: Record<string, { directory: string }>;
  documents: WorkspaceDocument[]; issues: WorkspaceLink[]; prs: WorkspaceLink[];
  exclusions: ReferenceIdentity[];
}
export interface ReferenceBoundary { compactionDigest: string; transcriptLength: number; prefixDigest: string }
export interface AgentRecord {
  version: 1; workspaceId: string; sessionId: string; sessionFile?: string;
  membershipToken: string; boundary: ReferenceBoundary; evidenceGeneration: string;
  displayAttempt?: { attempt: string; nonce: string };
  checkout: string; cwd: string; title: string; titleMode: TitleMode; status: CardStatus; now: string; remaining: string;
}
export interface Snapshot<T> { path: string; record: T; revision: string }
export interface ReferenceSelection { workspaceId: string; revision: string; identity: string }
export interface Authority { sessionId: string; workspaceId: string; membershipToken: string; evidenceGeneration: string }
export type OwnUpdate = Partial<Pick<AgentRecord, 'title' | 'status' | 'now' | 'remaining'>>;

export function object(value: unknown, keys: string[], required = keys): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value) || ![Object.prototype, null].includes(Object.getPrototypeOf(value))) throw new Error('Expected plain object');
  const input = value as Record<string, unknown>;
  for (const key of Object.keys(input)) if (!keys.includes(key)) throw new Error(`Unknown field ${key}`);
  for (const key of required) if (!Object.hasOwn(input, key)) throw new Error(`Missing field ${key}`);
  return input;
}
export function text(value: unknown, nonempty = false): string {
  if (typeof value !== 'string' || /[\p{Cc}\p{Cf}]/u.test(value) || (nonempty && !value.trim())) throw new Error('Expected safe single-line text');
  return value;
}
export function opaque(value: unknown): string {
  const s = text(value, true);
  if (!/^[a-zA-Z0-9_-]{1,128}$/.test(s)) throw new Error('Invalid opaque identity');
  return s;
}
export function absolute(value: unknown): string {
  const path = text(value, true);
  if (!isAbsolute(path) || normalize(path) !== path) throw new Error('Expected normalized absolute path');
  return path;
}
function relativePath(value: unknown): string {
  const p = text(value, true);
  if (isAbsolute(p) || p.includes('\\') || p.split('/').some(p => !p || p === '.' || p === '..')) throw new Error('Invalid relative document path');
  return p;
}
function url(value: unknown): string {
  const s = text(value, true);
  try {
    const u = new URL(s);
    if (!/^https?:\/\//i.test(s) || !u.hostname || /\s|\\/u.test(s) || /[\p{Cc}\p{Cf}]/u.test(decodeURIComponent(s))) throw new Error();
  } catch { throw new Error('Invalid HTTP(S) URL'); }
  return s;
}
function array<T>(value: unknown, parse: (value: unknown) => T): T[] {
  if (!Array.isArray(value)) throw new Error('Expected array');
  return Array.from(value, parse);
}
function mode(value: unknown): TitleMode {
  if (value !== 'auto' && value !== 'manual') throw new Error('Invalid title mode');
  return value;
}
function status(value: unknown): CardStatus {
  if (!['active', 'waiting', 'blocked', 'parked', 'done'].includes(value as string)) throw new Error('Invalid status');
  return value as CardStatus;
}
export function documentIdentity(value: unknown): DocumentIdentity {
  const v = object(value, ['checkout', 'path']); return { checkout: opaque(v.checkout), path: relativePath(v.path) };
}
function document(value: unknown): WorkspaceDocument {
  const v = object(value, ['checkout', 'path', 'description']);
  return { ...documentIdentity({ checkout: v.checkout, path: v.path }), description: text(v.description) };
}
function link(value: unknown): WorkspaceLink {
  const v = object(value, ['url', 'description']); return { url: url(v.url), description: text(v.description) };
}
export function referenceIdentity(value: ReferenceIdentity): string {
  return 'url' in value ? JSON.stringify(['url', value.url]) : JSON.stringify(['document', value.checkout, value.path]);
}
export function validateReferenceIdentity(value: unknown): ReferenceIdentity {
  if (value && typeof value === 'object' && Object.hasOwn(value, 'url')) return { url: url(object(value, ['url']).url) };
  return documentIdentity(value);
}
export function validateBoundary(value: unknown): ReferenceBoundary {
  const v = object(value, ['compactionDigest', 'transcriptLength', 'prefixDigest']);
  if (!Number.isSafeInteger(v.transcriptLength) || (v.transcriptLength as number) < 0) throw new Error('Invalid transcript length');
  return { compactionDigest: text(v.compactionDigest, true), transcriptLength: v.transcriptLength as number, prefixDigest: text(v.prefixDigest, true) };
}
export function validateWorkspace(value: unknown): WorkspaceRecord {
  const v = object(value, ['version', 'id', 'creationToken', 'mutationToken', 'title', 'titleMode', 'goal', 'planSources', 'sourceFingerprint', 'checkouts', 'documents', 'issues', 'prs', 'exclusions']);
  if (v.version !== 1) throw new Error('Unsupported workspace version');
  if (!v.checkouts || typeof v.checkouts !== 'object' || Array.isArray(v.checkouts) || ![Object.prototype, null].includes(Object.getPrototypeOf(v.checkouts))) throw new Error('Invalid checkouts');
  const checkouts = Object.fromEntries(Object.entries(v.checkouts).map(([key, value]) => [opaque(key), { directory: absolute(object(value, ['directory']).directory) }]));
  if (new Set(Object.values(checkouts).map(c => c.directory)).size !== Object.keys(checkouts).length) throw new Error('Duplicate checkout directory');
  const documents = array(v.documents, document); const issues = array(v.issues, link); const prs = array(v.prs, link);
  const exclusions = array(v.exclusions, validateReferenceIdentity); const planSources = array(v.planSources, documentIdentity);
  const visible = [...documents, ...issues, ...prs];
  for (const list of [visible, exclusions, planSources]) if (new Set(list.map(referenceIdentity)).size !== list.length) throw new Error('Duplicate reference identity');
  for (const ref of [...documents, ...exclusions, ...planSources]) if ('checkout' in ref && !Object.hasOwn(checkouts, ref.checkout)) throw new Error('Unknown checkout');
  if (visible.some(ref => exclusions.some(e => referenceIdentity(e) === referenceIdentity(ref)))) throw new Error('Excluded visible reference');
  for (const ref of planSources) if (!documents.some(d => referenceIdentity(d) === referenceIdentity(ref))) throw new Error('Plan source must be a visible document');
  const goal = text(v.goal); const sourceFingerprint = v.sourceFingerprint === null ? null : text(v.sourceFingerprint, true);
  if ((goal || planSources.length || sourceFingerprint) && (!planSources.length || !sourceFingerprint)) throw new Error('Goal requires plan provenance');
  return { version: 1, id: opaque(v.id), creationToken: text(v.creationToken, true), mutationToken: text(v.mutationToken, true), title: text(v.title, true), titleMode: mode(v.titleMode), goal, planSources, sourceFingerprint, checkouts, documents, issues, prs, exclusions };
}
export function validateAgent(value: unknown): AgentRecord {
  const required = ['version', 'workspaceId', 'sessionId', 'membershipToken', 'boundary', 'evidenceGeneration', 'checkout', 'cwd', 'title', 'titleMode', 'status', 'now', 'remaining'];
  const v = object(value, [...required, 'sessionFile', 'displayAttempt'], required);
  if (v.version !== 1) throw new Error('Unsupported agent version');
  const optional: Pick<AgentRecord, 'sessionFile' | 'displayAttempt'> = {};
  if (Object.hasOwn(v, 'sessionFile')) optional.sessionFile = absolute(v.sessionFile);
  if (Object.hasOwn(v, 'displayAttempt')) { const d = object(v.displayAttempt, ['attempt', 'nonce']); optional.displayAttempt = { attempt: text(d.attempt, true), nonce: text(d.nonce, true) }; }
  return { version: 1, workspaceId: opaque(v.workspaceId), sessionId: text(v.sessionId, true), membershipToken: text(v.membershipToken, true), boundary: validateBoundary(v.boundary), evidenceGeneration: text(v.evidenceGeneration, true), checkout: opaque(v.checkout), cwd: absolute(v.cwd), title: text(v.title, true), titleMode: mode(v.titleMode), status: status(v.status), now: text(v.now), remaining: text(v.remaining), ...optional };
}
export function applyOwnUpdate(agent: AgentRecord, value: unknown): AgentRecord {
  const v = object(value, ['title', 'status', 'now', 'remaining'], []);
  // Validate even an ignored automatic title.
  if (Object.hasOwn(v, 'title')) text(v.title, true);
  return validateAgent({ ...agent, ...v, title: agent.titleMode === 'manual' ? agent.title : (v.title ?? agent.title) });
}
export function validateAdditions(value: unknown): ReferenceAdditions {
  const v = object(value, ['documents', 'issues', 'prs'], []);
  return { ...(Object.hasOwn(v, 'documents') ? { documents: array(v.documents, document) } : {}), ...(Object.hasOwn(v, 'issues') ? { issues: array(v.issues, link) } : {}), ...(Object.hasOwn(v, 'prs') ? { prs: array(v.prs, link) } : {}) };
}
export function applySharedAdditions(workspace: WorkspaceRecord, value: unknown, explicit = false): WorkspaceRecord {
  const additions = validateAdditions(value); const next = structuredClone(workspace);
  const seen = new Set([...next.documents, ...next.issues, ...next.prs].map(referenceIdentity));
  for (const key of ['documents', 'issues', 'prs'] as const) for (const ref of additions[key] ?? []) {
    const id = referenceIdentity(ref);
    if (seen.has(id)) continue;
    if (next.exclusions.some(e => referenceIdentity(e) === id)) {
      if (!explicit) continue;
      next.exclusions = next.exclusions.filter(e => referenceIdentity(e) !== id);
    }
    seen.add(id);
    if (key === 'documents') next.documents.push(ref as WorkspaceDocument);
    else next[key].push(ref as WorkspaceLink);
  }
  return validateWorkspace(next);
}

export function applyReferenceRemoval(snapshot: Snapshot<WorkspaceRecord>, selection: ReferenceSelection): WorkspaceRecord {
  object(selection, ['workspaceId', 'revision', 'identity']);
  if (selection.workspaceId !== snapshot.record.id) throw new Error('Workspace selection mismatch');
  if (selection.revision !== snapshot.revision) throw new Error('Stale revision conflict');
  const next = validateWorkspace(snapshot.record);
  const ref = [...next.documents, ...next.issues, ...next.prs].find(r => referenceIdentity(r) === selection.identity);
  if (!ref) throw new Error('Stale reference identity');
  next.documents = next.documents.filter(r => referenceIdentity(r) !== selection.identity);
  next.issues = next.issues.filter(r => referenceIdentity(r) !== selection.identity);
  next.prs = next.prs.filter(r => referenceIdentity(r) !== selection.identity);
  next.exclusions.push('url' in ref ? { url: ref.url } : { checkout: ref.checkout, path: ref.path });
  if (next.planSources.some(r => referenceIdentity(r) === selection.identity)) { next.goal = ''; next.planSources = []; next.sourceFingerprint = null; }
  return validateWorkspace(next);
}

export function validateMembership(workspace: WorkspaceRecord, agent: AgentRecord): void {
  if (workspace.id !== agent.workspaceId || !Object.hasOwn(workspace.checkouts, agent.checkout)) throw new Error('Workspace-agent membership mismatch');
  const local = relative(workspace.checkouts[agent.checkout].directory, agent.cwd);
  if (local === '..' || local.startsWith(`..${sep}`) || isAbsolute(local)) throw new Error('Agent cwd outside registered checkout');
}
