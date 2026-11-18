export type Status = 'active' | 'waiting' | 'blocked' | 'parked' | 'done' | 'closed';
export type Mode = 'automatic' | 'manual';
export type TitleMode = Mode | 'inherit';
export type TitleDefaults = { stream: Mode; session: Mode };
export type ReferenceRole = 'deliverable' | 'dependency' | 'brief' | 'background';
export type ReferenceState = 'active' | 'fulfilled' | 'dropped';
export type RepositoryDocument = {
  repository_url: string; path: string; ref?: string | null;
  commit?: string | null; source_session_id?: string | null;
};
export type ReferenceTarget =
  | ({kind: 'document'} & RepositoryDocument)
  | {kind: 'pr'; url: string}
  | {kind: 'issue'; url: string}
  | {kind: 'link'; url: string};
export type ResourceKey =
  | {kind: 'document'; repository_url: string; path: string}
  | {kind: 'url'; url: string};
export type ReferenceValue = {
  name: string; purpose: string; role: ReferenceRole; state: ReferenceState; target: ReferenceTarget;
};
/** A saved reference row; the Stream revision covers its concurrency. */
export type Reference = ReferenceValue;

export type StreamWire = {
  id: string; title: string; title_mode: TitleMode; goal: string | null; goal_short: string | null;
  goal_mode: Mode; status: Status; owner: { host_id: string };
  repositories: Record<string, { url: string }>; references: Reference[];
  /** The store's compare-and-swap token and last change time; user edits send them back unchanged. */
  revision: number; updated_at_ms: number;
};
export type SessionWire = {
  id: string; host_id: string; stream_id: string; harness: string; harness_session_id: string;
  membership_epoch?: number;
  artifacts?: Record<string, { name: string; description: string }[]>;
  title: string; title_mode: TitleMode; status: Status;
  recap?: string; recap_short?: string | null;
  state: string; state_short: string | null; remaining: string; remaining_short?: string | null;
  checkout: { repository: string; repository_url?: string | null; ref: string; directory_within_checkout: string } | null;
  revision: number; updated_at_ms: number;
};
export type LocalContext = {
  catalog_source: string; host_id: string; stream: StreamWire | null; session: SessionWire | null;
  sessions: SessionWire[]; title_defaults: TitleDefaults;
  checkouts?: unknown[]; document_locations?: unknown[];
};

const object = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);
const text = (value: unknown): value is string => typeof value === 'string' && !value.includes('\0');
const identity = (value: unknown): value is string => text(value) && value.trim().length > 0;
const nullableText = (value: unknown) => value === null || text(value);
const mode = (value: unknown): value is Mode => value === 'automatic' || value === 'manual';
const titleMode = (value: unknown): value is TitleMode => mode(value) || value === 'inherit';
const count = (value: unknown): value is number => Number.isSafeInteger(value) && (value as number) >= 0;
const stamped = (value: Record<string, unknown>) => count(value.revision) && count(value.updated_at_ms);
const status = (value: unknown): value is Status => typeof value === 'string' && ['active', 'waiting', 'blocked', 'parked', 'done', 'closed'].includes(value);

export function isReferenceText(value: unknown): value is string {
  return typeof value === 'string'
    && !value.includes('\0')
    && !/[\ud800-\udfff]/u.test(value)
    && /[^\u0009-\u000d\u0020\u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]/u.test(value);
}

const onlyFields = (value: Record<string, unknown>, fields: readonly string[]) =>
  Object.keys(value).every(key => fields.includes(key));
const referencePath = (value: unknown): value is string => isReferenceText(value)
  && !value.startsWith('/') && !/^[a-z]:/i.test(value) && !value.includes('\\') && !value.split('/').includes('..');
const recoveryText = (value: unknown) => value === undefined || value === null || isReferenceText(value);

export function referenceUrl(value: string): URL | undefined {
  if (!isReferenceText(value) || /[\\\p{White_Space}\p{Cc}\p{Cf}]/u.test(value)) return undefined;
  try {
    if (/[\p{Cc}\p{Cf}]/u.test(decodeURIComponent(value))) return undefined;
    const url = new URL(value);
    return ['http:', 'https:'].includes(url.protocol) && url.host && !url.username && !url.password ? url : undefined;
  } catch {
    return undefined;
  }
}

export function isRepositoryDocument(value: unknown): value is RepositoryDocument {
  return object(value) && onlyFields(value, ['repository_url', 'path', 'ref', 'commit', 'source_session_id'])
    && isReferenceText(value.repository_url) && referencePath(value.path)
    && recoveryText(value.ref) && recoveryText(value.commit) && recoveryText(value.source_session_id);
}

export function isReferenceTarget(value: unknown): value is ReferenceTarget {
  if (!object(value)) return false;
  if (value.kind === 'document') {
    const {kind, ...document} = value;
    return isRepositoryDocument(document);
  }
  return ['pr', 'issue', 'link'].includes(value.kind as string) && onlyFields(value, ['kind', 'url'])
    && isReferenceText(value.url) && !!referenceUrl(value.url);
}

export function resourceKey(target: ReferenceTarget): ResourceKey {
  return target.kind === 'document'
    ? {kind: 'document', repository_url: target.repository_url, path: target.path}
    : {kind: 'url', url: target.url};
}

export function isResourceKey(value: unknown): value is ResourceKey {
  if (!object(value)) return false;
  return value.kind === 'document'
    ? onlyFields(value, ['kind', 'repository_url', 'path']) && isReferenceText(value.repository_url) && referencePath(value.path)
    : value.kind === 'url' && onlyFields(value, ['kind', 'url']) && isReferenceText(value.url) && !!referenceUrl(value.url);
}

const referenceFields = ['name', 'purpose', 'role', 'state', 'target'];
const referenceValueFields = (value: Record<string, unknown>) => isReferenceText(value.name) && isReferenceText(value.purpose)
  && ['deliverable', 'dependency', 'brief', 'background'].includes(value.role as string)
  && ['active', 'fulfilled', 'dropped'].includes(value.state as string) && isReferenceTarget(value.target);

export function isReferenceValue(value: unknown): value is ReferenceValue {
  return object(value) && onlyFields(value, referenceFields) && referenceValueFields(value);
}

export function isStream(value: unknown): value is StreamWire {
  if (!object(value) || !identity(value.id) || !text(value.title) || !titleMode(value.title_mode)
    || !nullableText(value.goal) || !nullableText(value.goal_short) || !mode(value.goal_mode)
    || !status(value.status) || !stamped(value) || !object(value.owner) || !identity(value.owner.host_id)
    || !object(value.repositories) || !Object.entries(value.repositories).every(([alias, repo]) => identity(alias) && object(repo) && identity(repo.url))
    || ['documents', 'issues', 'prs'].some(key => Object.hasOwn(value, key))
    || !Array.isArray(value.references) || !value.references.every(isReferenceValue)) return false;
  return new Set(value.references.map(ref => {
    const key = resourceKey(ref.target);
    return JSON.stringify(key.kind === 'url' ? ['url', key.url] : ['document', key.repository_url, key.path]);
  })).size === value.references.length;
}

export function isSession(value: unknown): value is SessionWire {
  return object(value) && [value.id, value.host_id, value.stream_id, value.harness, value.harness_session_id].every(identity)
    && [value.title, value.state, value.remaining].every(text) && titleMode(value.title_mode) && status(value.status)
    && stamped(value)
    && nullableText(value.state_short)
    && (value.recap === undefined || text(value.recap))
    && (value.recap_short === undefined || nullableText(value.recap_short))
    && (value.remaining_short === undefined || nullableText(value.remaining_short))
    && (value.membership_epoch === undefined || count(value.membership_epoch))
    && (value.artifacts === undefined || object(value.artifacts) && Object.entries(value.artifacts).every(([stream, entries]) =>
      identity(stream) && Array.isArray(entries) && entries.every(entry => object(entry) && identity(entry.name) && identity(entry.description))
      && new Set(entries.map(entry => entry.name)).size === entries.length))
    && (value.checkout === null || object(value.checkout)
      && identity(value.checkout.repository) && identity(value.checkout.ref) && text(value.checkout.directory_within_checkout)
      && (value.checkout.repository_url === undefined || value.checkout.repository_url === null || identity(value.checkout.repository_url)));
}

export function parseLocalContext(value: unknown): LocalContext & Record<string, unknown> {
  if (!object(value) || Object.hasOwn(value, 'error') || Object.hasOwn(value, 'authority')
    || !identity(value.catalog_source) || !identity(value.host_id)
    || !(value.stream === null || isStream(value.stream)) || !(value.session === null || isSession(value.session))
    || !Array.isArray(value.sessions) || !value.sessions.every(isSession)
    || !object(value.title_defaults) || !mode(value.title_defaults.stream) || !mode(value.title_defaults.session)) {
    throw new Error('Invalid local catalog context');
  }
  const session = value.session;
  if (session && (!value.stream || session.host_id !== value.stream.owner.host_id || session.stream_id !== value.stream.id)
    || value.sessions.length !== (session ? 1 : 0)
    || value.sessions.some(row => row.id !== session?.id || row.host_id !== session?.host_id
      || row.harness !== session?.harness || row.harness_session_id !== session?.harness_session_id
      || row.stream_id !== session?.stream_id)) {
    throw new Error('Inconsistent addressed catalog context');
  }
  return value as LocalContext & Record<string, unknown>;
}
