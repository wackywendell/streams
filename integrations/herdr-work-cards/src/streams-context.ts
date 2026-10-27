export type Status = 'active' | 'waiting' | 'blocked' | 'parked' | 'done';
export type Mode = 'automatic' | 'manual';
export type TitleMode = Mode | 'inherit';
export type TitleDefaults = { stream: Mode; session: Mode };
export type Link = { url: string; description: string };
export type Document = {
  repository: string; ref: string; path: string; description: string;
  commit?: string | null; source_session_id?: string | null;
};
export type StreamWire = {
  id: string; title: string; title_mode: TitleMode; goal: string | null; goal_short: string | null;
  goal_mode: Mode; status: Status; archived: boolean; owner: { host_id: string };
  repositories: Record<string, { url: string }>; documents: Document[]; issues: Link[]; prs: Link[];
};
export type SessionWire = {
  id: string; host_id: string; stream_id: string; harness: string; harness_session_id: string;
  previous_stream_ids?: string[];
  artifacts?: Record<string, { name: string; description: string }[]>;
  title: string; title_mode: TitleMode; status: Status;
  recap?: string; recap_short?: string | null;
  state: string; state_short: string | null; remaining: string; remaining_short?: string | null;
  checkout: { repository: string; repository_url: string; ref: string; directory_within_checkout: string } | null;
};
export type LocalContext = {
  catalog_source: string; host_id: string; stream: StreamWire | null; session: SessionWire | null;
  sessions: SessionWire[]; title_defaults: TitleDefaults;
};

const object = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);
const text = (value: unknown): value is string => typeof value === 'string' && !value.includes('\0');
const identity = (value: unknown): value is string => text(value) && value.trim().length > 0;
const nullableText = (value: unknown) => value === null || text(value);
const provenance = (value: unknown) => value === undefined || value === null || identity(value);
const mode = (value: unknown): value is Mode => value === 'automatic' || value === 'manual';
const titleMode = (value: unknown): value is TitleMode => mode(value) || value === 'inherit';
const status = (value: unknown): value is Status => typeof value === 'string' && ['active', 'waiting', 'blocked', 'parked', 'done'].includes(value);
const link = (value: unknown): value is Link => object(value) && text(value.url) && text(value.description);

export function isStream(value: unknown): value is StreamWire {
  if (!object(value) || !identity(value.id) || !text(value.title) || !titleMode(value.title_mode)
    || !nullableText(value.goal) || !nullableText(value.goal_short) || !mode(value.goal_mode)
    || !status(value.status) || typeof value.archived !== 'boolean' || !object(value.owner) || !identity(value.owner.host_id)
    || !object(value.repositories) || !Object.entries(value.repositories).every(([alias, repo]) => identity(alias) && object(repo) && identity(repo.url))
    || !Array.isArray(value.documents) || !value.documents.every(doc => object(doc)
      && [doc.repository, doc.ref, doc.path].every(identity) && text(doc.description)
      && provenance(doc.commit) && provenance(doc.source_session_id)
      && Object.hasOwn(value.repositories as object, doc.repository as string))
    || !Array.isArray(value.issues) || !value.issues.every(link) || !Array.isArray(value.prs) || !value.prs.every(link)) return false;
  return new Set(value.documents.map(doc => JSON.stringify([doc.repository, doc.path]))).size === value.documents.length;
}

export function isSession(value: unknown): value is SessionWire {
  return object(value) && [value.id, value.host_id, value.stream_id, value.harness, value.harness_session_id].every(identity)
    && [value.title, value.state, value.remaining].every(text) && titleMode(value.title_mode) && status(value.status)
    && nullableText(value.state_short)
    && (value.recap === undefined || text(value.recap))
    && (value.recap_short === undefined || nullableText(value.recap_short))
    && (value.remaining_short === undefined || nullableText(value.remaining_short))
    && (value.previous_stream_ids === undefined || Array.isArray(value.previous_stream_ids)
      && value.previous_stream_ids.every(identity)
      && value.previous_stream_ids.every((id, i, ids) => id !== (i + 1 < ids.length ? ids[i + 1] : value.stream_id)))
    && (value.artifacts === undefined || object(value.artifacts) && Object.entries(value.artifacts).every(([stream, entries]) =>
      (stream === value.stream_id || Array.isArray(value.previous_stream_ids) && value.previous_stream_ids.includes(stream))
      && Array.isArray(entries) && entries.every(entry => object(entry) && identity(entry.name) && identity(entry.description))
      && new Set(entries.map(entry => entry.name)).size === entries.length))
    && (value.checkout === null || object(value.checkout)
      && identity(value.checkout.repository) && identity(value.checkout.ref) && text(value.checkout.directory_within_checkout)
      && identity(value.checkout.repository_url));
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
