import { isAbsolute, normalize, relative, sep } from 'node:path';
import stringWidth from 'string-width';
export type CardStatus = 'active' | 'waiting' | 'blocked' | 'parked' | 'done';

export type TitleMode = 'auto' | 'manual';
export interface DocumentIdentity { checkout: string; path: string }
export interface WorkspaceDocument extends DocumentIdentity { description: string }
export interface WorkspaceLink { url: string; description: string; label?: string }
export interface LinkLabelUpdate { url: string; label: string }
export interface SharedReferenceUpdate { additions?: ReferenceAdditions; labels?: LinkLabelUpdate[] }
export type ReferenceIdentity = DocumentIdentity | { url: string };
export interface ReferenceAdditions { documents?: WorkspaceDocument[]; issues?: WorkspaceLink[]; prs?: WorkspaceLink[] }
export interface ConversationProvenance { readonly sessionId: string; readonly digest: string }
export type GoalProvenance =
  | { readonly kind: 'documents'; readonly sources: readonly [Readonly<DocumentIdentity>, ...Readonly<DocumentIdentity>[]]; readonly fingerprint: string; readonly sessionId?: never; readonly digest?: never }
  | ({ readonly kind: 'conversation'; readonly sources?: never; readonly fingerprint?: never } & ConversationProvenance);
export type Goal =
  | { readonly kind: 'unevaluated'; readonly text?: never; readonly short?: never; readonly provenance?: never }
  | { readonly kind: 'unknown'; readonly text?: never; readonly short?: never; readonly provenance: GoalProvenance }
  | { readonly kind: 'known'; readonly text: string; readonly short?: string; readonly provenance: GoalProvenance };

export interface WorkspaceRecord {
  version: 1; id: string; creationToken: string; mutationToken: string; title: string; titleMode: TitleMode;
  goal: Goal;
  checkouts: Record<string, { directory: string }>;
  documents: WorkspaceDocument[]; issues: WorkspaceLink[]; prs: WorkspaceLink[];
  exclusions: ReferenceIdentity[];
}
export interface PersistedWorkspaceV1 extends Omit<WorkspaceRecord, 'goal'> {
  goal: string; goalShort?: string; planSources: DocumentIdentity[]; sourceFingerprint: string | null;
  conversationProvenance?: ConversationProvenance;
}
export interface ReferenceBoundary { compactionDigest: string; transcriptLength: number; prefixDigest: string }
export interface AgentRecord {
  version: 1; workspaceId: string; sessionId: string; sessionFile?: string;
  membershipToken: string; boundary: ReferenceBoundary; evidenceGeneration: string;
  displayAttempt?: { attempt: string; nonce: string };
  checkout: string; cwd: string; title: string; titleMode: TitleMode; status: CardStatus; statusShort?: string; now: string; remaining: string;
}
export interface Snapshot<T> { path: string; record: T; revision: string }
export interface ReferenceSelection { workspaceId: string; revision: string; identity: string }
export interface Authority { sessionId: string; workspaceId: string; membershipToken: string; evidenceGeneration: string }
export type OwnUpdate = Partial<Pick<AgentRecord, 'title' | 'status' | 'statusShort' | 'now' | 'remaining'>>;

export function agentAuthority(agent: AgentRecord): Authority {
  return { sessionId: agent.sessionId, workspaceId: agent.workspaceId, membershipToken: agent.membershipToken, evidenceGeneration: agent.evidenceGeneration };
}
export function sameAgentMembership(a: AgentRecord, b: AgentRecord): boolean {
  return a.sessionId === b.sessionId && a.workspaceId === b.workspaceId && a.membershipToken === b.membershipToken
    && a.checkout === b.checkout && a.cwd === b.cwd;
}
export function assertAgentAuthority(agent: AgentRecord, authority: Authority): void {
  const v = object(authority, ['sessionId', 'workspaceId', 'membershipToken', 'evidenceGeneration']);
  for (const value of Object.values(v)) text(value, true);
  if (agent.sessionId !== authority.sessionId || agent.workspaceId !== authority.workspaceId || agent.membershipToken !== authority.membershipToken) throw new Error('Stale workspace membership');
  if (agent.evidenceGeneration !== authority.evidenceGeneration) throw new Error('Stale evidence generation');
}

export function object(value: unknown, keys: string[], required = keys): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value) || ![Object.prototype, null].includes(Object.getPrototypeOf(value))) throw new Error('Expected plain object');
  const input = value as Record<string, unknown>;
  for (const key of Object.keys(input)) if (!keys.includes(key)) throw new Error(`Unknown field ${key}`);
  for (const key of required) if (!Object.hasOwn(input, key)) throw new Error(`Missing field ${key}`);
  return input;
}
export function text(value: unknown, nonempty = false): string {
  if (typeof value !== 'string' || !value.isWellFormed() || /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u.test(value) || (nonempty && !value.trim())) throw new Error('Expected safe single-line text');
  return value;
}
export function displayText(value: unknown, cells: number, nonempty = false): string {
  const s = text(value, nonempty);
  if (stringWidth(s) > cells) throw new Error(`Display text exceeds ${cells} cells`);
  return s;
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
  const v = object(value, ['url', 'description', 'label'], ['url', 'description']);
  return { url: url(v.url), description: text(v.description), ...(Object.hasOwn(v, 'label') ? { label: displayText(v.label, 60, true) } : {}) };
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
export function unevaluatedGoal(): Goal { return Object.freeze({ kind: 'unevaluated' }); }

/** Evaluated results retain their origin even when the evidence establishes no purpose. */
function purposeGoal(fullText: string, provenance: GoalProvenance, short?: string): Goal {
  const purpose = text(fullText);
  const derivative = short === undefined ? undefined : displayText(short, 25);
  if (!purpose.trim()) {
    if (derivative?.trim()) throw new Error('Short goal requires nonempty goal');
    return Object.freeze({ kind: 'unknown', provenance });
  }
  return Object.freeze({ kind: 'known', text: purpose, ...(derivative?.trim() ? { short: derivative } : {}), provenance });
}

/** Document-backed purpose requires a nonempty immutable source selection. */
export function evaluatedGoal(fullText: string, sources: readonly DocumentIdentity[], fingerprint: string, short?: string): Goal {
  const identities = array(sources, value => Object.freeze(documentIdentity(value)));
  if (!identities.length) throw new Error('Goal requires nonempty plan provenance');
  if (new Set(identities.map(referenceIdentity)).size !== identities.length) throw new Error('Duplicate reference identity');
  return purposeGoal(fullText, Object.freeze({ kind: 'documents',
    sources: Object.freeze(identities) as readonly [Readonly<DocumentIdentity>, ...Readonly<DocumentIdentity>[]],
    fingerprint: text(fingerprint, true) }), short);
}

function conversationProvenance(value: unknown): ConversationProvenance {
  const v = object(value, ['sessionId', 'digest']);
  const digest = text(v.digest, true);
  if (!/^[a-f0-9]{64}$/.test(digest)) throw new Error('Invalid conversation evidence digest');
  return Object.freeze({ sessionId: text(v.sessionId, true), digest });
}

/** Persist only attribution and the digest, never the captured transcript. */
export function conversationGoal(fullText: string, origin: ConversationProvenance, short?: string): Goal {
  return purposeGoal(fullText, Object.freeze({ kind: 'conversation', ...conversationProvenance(origin) }), short);
}

export function parseGoal(value: unknown): Goal {
  const v = object(value, ['kind', 'text', 'short', 'provenance'], ['kind']);
  if (v.kind === 'unevaluated') { object(value, ['kind']); return unevaluatedGoal(); }
  if (v.kind === 'unknown') object(value, ['kind', 'provenance']);
  else if (v.kind === 'known') {
    object(value, ['kind', 'text', 'short', 'provenance'], ['kind', 'text', 'provenance']);
    text(v.text, true);
    if (Object.hasOwn(v, 'short')) displayText(v.short, 25);
  } else throw new Error('Invalid Goal kind');
  const provenance = object(v.provenance, ['kind', 'sources', 'fingerprint', 'sessionId', 'digest'], ['kind']);
  const purpose = v.kind === 'known' ? v.text as string : '';
  if (provenance.kind === 'documents') {
    object(provenance, ['kind', 'sources', 'fingerprint']);
    return evaluatedGoal(purpose, provenance.sources as DocumentIdentity[], provenance.fingerprint as string, v.short as string | undefined);
  }
  if (provenance.kind !== 'conversation') throw new Error('Invalid Goal provenance');
  object(provenance, ['kind', 'sessionId', 'digest']);
  return conversationGoal(purpose, { sessionId: provenance.sessionId as string, digest: provenance.digest as string }, v.short as string | undefined);
}

export type GoalTransition =
  | { readonly kind: 'evaluate'; readonly goal: Goal }
  | { readonly kind: 'source_removed'; readonly identity: ReferenceIdentity };

/** All purpose/provenance replacement and derivative invalidation has one owner. */
export function transitionGoal(previous: Goal, event: GoalTransition): Goal {
  if (event.kind === 'source_removed') {
    return previous.kind !== 'unevaluated' && previous.provenance.kind === 'documents' && previous.provenance.sources.some(source => referenceIdentity(source) === referenceIdentity(event.identity))
      ? unevaluatedGoal() : previous;
  }
  const next = parseGoal(event.goal);
  if (previous.kind === 'known' && next.kind === 'known' && next.text === previous.text && next.short === undefined && previous.short !== undefined) {
    return purposeGoal(next.text, next.provenance, previous.short);
  }
  return next;
}

export function goalText(goal: Goal): string { return goal.kind === 'known' ? goal.text : ''; }
export function goalShort(goal: Goal): string { return goal.kind === 'known' ? goal.short ?? '' : ''; }

export function validateWorkspace(value: unknown): WorkspaceRecord {
  const v = object(value, ['version', 'id', 'creationToken', 'mutationToken', 'title', 'titleMode', 'goal', 'checkouts', 'documents', 'issues', 'prs', 'exclusions']);
  if (v.version !== 1) throw new Error('Unsupported workspace version');
  if (!v.checkouts || typeof v.checkouts !== 'object' || Array.isArray(v.checkouts) || ![Object.prototype, null].includes(Object.getPrototypeOf(v.checkouts))) throw new Error('Invalid checkouts');
  const checkouts = Object.fromEntries(Object.entries(v.checkouts).map(([key, value]) => [opaque(key), { directory: absolute(object(value, ['directory']).directory) }]));
  if (new Set(Object.values(checkouts).map(c => c.directory)).size !== Object.keys(checkouts).length) throw new Error('Duplicate checkout directory');
  const documents = array(v.documents, document); const issues = array(v.issues, link); const prs = array(v.prs, link);
  const exclusions = array(v.exclusions, validateReferenceIdentity); const goal = parseGoal(v.goal);
  const planSources = goal.kind !== 'unevaluated' && goal.provenance.kind === 'documents' ? goal.provenance.sources : [];
  const visible = [...documents, ...issues, ...prs];
  for (const list of [visible, exclusions, planSources]) if (new Set(list.map(referenceIdentity)).size !== list.length) throw new Error('Duplicate reference identity');
  for (const ref of [...documents, ...exclusions, ...planSources]) if ('checkout' in ref && !Object.hasOwn(checkouts, ref.checkout)) throw new Error('Unknown checkout');
  if (visible.some(ref => exclusions.some(e => referenceIdentity(e) === referenceIdentity(ref)))) throw new Error('Excluded visible reference');
  for (const ref of planSources) if (!documents.some(d => referenceIdentity(d) === referenceIdentity(ref))) throw new Error('Plan source must be a visible document');
  return { version: 1, id: opaque(v.id), creationToken: text(v.creationToken, true), mutationToken: text(v.mutationToken, true), title: text(v.title, true), titleMode: mode(v.titleMode), goal, checkouts, documents, issues, prs, exclusions };
}

/** Only flat version-1 storage is accepted here; validate before semantic normalization. */
export function parseWorkspaceV1(value: unknown): WorkspaceRecord {
  const required = ['version', 'id', 'creationToken', 'mutationToken', 'title', 'titleMode', 'goal', 'planSources', 'sourceFingerprint', 'checkouts', 'documents', 'issues', 'prs', 'exclusions'];
  const v = object(value, [...required, 'goalShort', 'conversationProvenance'], required);
  const purpose = text(v.goal);
  const sources = array(v.planSources, documentIdentity);
  const fingerprint = v.sourceFingerprint === null ? null : text(v.sourceFingerprint, true);
  const conversation = Object.hasOwn(v, 'conversationProvenance') ? conversationProvenance(v.conversationProvenance) : undefined;
  if (conversation ? sources.length || fingerprint !== null : (purpose || sources.length || fingerprint) && (!sources.length || !fingerprint)) throw new Error('Goal requires exclusive provenance');
  const short = Object.hasOwn(v, 'goalShort') ? displayText(v.goalShort, 25) : undefined;
  if (short?.trim() && !purpose.trim()) throw new Error('Short goal requires nonempty goal');
  const { goal: _, goalShort: __, planSources: ___, sourceFingerprint: ____, conversationProvenance: _____, ...shared } = v;
  return validateWorkspace({ ...shared, goal: conversation ? conversationGoal(purpose, conversation, short) : sources.length ? evaluatedGoal(purpose, sources, fingerprint!, short) : unevaluatedGoal() });
}

/** Storage and public record projection stay flat; domain snapshots never carry these fields. */
export function serializeWorkspaceV1(record: WorkspaceRecord): PersistedWorkspaceV1 {
  const { goal, ...shared } = validateWorkspace(record);
  return { ...shared, goal: goalText(goal), ...(goal.kind === 'known' && goal.short !== undefined ? { goalShort: goal.short } : {}),
    planSources: goal.kind !== 'unevaluated' && goal.provenance.kind === 'documents' ? goal.provenance.sources.map(source => ({ ...source })) : [],
    sourceFingerprint: goal.kind !== 'unevaluated' && goal.provenance.kind === 'documents' ? goal.provenance.fingerprint : null,
    ...(goal.kind !== 'unevaluated' && goal.provenance.kind === 'conversation'
      ? { conversationProvenance: { sessionId: goal.provenance.sessionId, digest: goal.provenance.digest } } : {}) };
}
export function validateAgent(value: unknown): AgentRecord {
  const required = ['version', 'workspaceId', 'sessionId', 'membershipToken', 'boundary', 'evidenceGeneration', 'checkout', 'cwd', 'title', 'titleMode', 'status', 'now', 'remaining'];
  const v = object(value, [...required, 'sessionFile', 'displayAttempt', 'statusShort'], required);
  if (v.version !== 1) throw new Error('Unsupported agent version');
  const optional: Pick<AgentRecord, 'sessionFile' | 'displayAttempt' | 'statusShort'> = {};
  if (Object.hasOwn(v, 'statusShort')) optional.statusShort = displayText(v.statusShort, 23);
  if (Object.hasOwn(v, 'sessionFile')) optional.sessionFile = absolute(v.sessionFile);
  if (Object.hasOwn(v, 'displayAttempt')) { const d = object(v.displayAttempt, ['attempt', 'nonce']); optional.displayAttempt = { attempt: text(d.attempt, true), nonce: text(d.nonce, true) }; }
  return { version: 1, workspaceId: opaque(v.workspaceId), sessionId: text(v.sessionId, true), membershipToken: text(v.membershipToken, true), boundary: validateBoundary(v.boundary), evidenceGeneration: text(v.evidenceGeneration, true), checkout: opaque(v.checkout), cwd: absolute(v.cwd), title: text(v.title, true), titleMode: mode(v.titleMode), status: status(v.status), now: text(v.now), remaining: text(v.remaining), ...optional };
}
export function applyOwnUpdate(agent: AgentRecord, value: unknown): AgentRecord {
  const v = object(value, ['title', 'status', 'statusShort', 'now', 'remaining'], []);
  // Validate even an ignored automatic title.
  if (Object.hasOwn(v, 'title')) text(v.title, true);
  const next = { ...agent, ...v, title: agent.titleMode === 'manual' ? agent.title : (v.title ?? agent.title) };
  if (!Object.hasOwn(v, 'statusShort') && (['status', 'now', 'remaining'] as const).some(key => Object.hasOwn(v, key) && v[key] !== agent[key])) delete next.statusShort;
  return validateAgent(next);
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

export function validateSharedReferenceUpdate(value: unknown, baseline: WorkspaceRecord): SharedReferenceUpdate {
  const v = object(value, ['additions', 'labels'], []);
  const additions = Object.hasOwn(v, 'additions') ? validateAdditions(v.additions) : undefined;
  if (additions) {
    for (const ref of additions.documents ?? []) if (!Object.hasOwn(baseline.checkouts, ref.checkout)) throw new Error('Unknown checkout');
    applySharedAdditions(baseline, additions);
  }
  const labels = Object.hasOwn(v, 'labels') ? array(v.labels, item => {
    const l = object(item, ['url', 'label']);
    const target = url(l.url);
    if (![...baseline.issues, ...baseline.prs].some(ref => ref.url === target)) throw new Error('Label target must be an existing request link');
    return { url: target, label: displayText(l.label, 60, true) };
  }) : undefined;
  if (labels && new Set(labels.map(l => l.url)).size !== labels.length) throw new Error('Duplicate label target');
  return { ...(additions ? { additions } : {}), ...(labels ? { labels } : {}) };
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
  next.goal = transitionGoal(next.goal, { kind: 'source_removed', identity: ref });
  return validateWorkspace(next);
}

export function validateMembership(workspace: WorkspaceRecord, agent: AgentRecord): void {
  if (workspace.id !== agent.workspaceId || !Object.hasOwn(workspace.checkouts, agent.checkout)) throw new Error('Workspace-agent membership mismatch');
  const local = relative(workspace.checkouts[agent.checkout].directory, agent.cwd);
  if (local === '..' || local.startsWith(`..${sep}`) || isAbsolute(local)) throw new Error('Agent cwd outside registered checkout');
}
