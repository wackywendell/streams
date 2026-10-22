export type Status = 'active' | 'waiting' | 'blocked' | 'parked' | 'done';
export type Mode = 'automatic' | 'manual';
export type Link = { url: string; description: string };
export type Document = {
  repository: string; ref: string; path: string; description: string;
  commit?: string | null; source_session_id?: string | null;
};
export type StreamWire = {
  id: string; title: string; title_mode: Mode; goal: string | null; goal_short: string | null;
  goal_mode: Mode; status: Status; archived: boolean; owner: { host_id: string };
  repositories: Record<string, { url: string }>; documents: Document[]; issues: Link[]; prs: Link[];
};
export type SessionWire = {
  id: string; host_id: string; stream_id: string; harness: string; harness_session_id: string;
  previous_stream_ids?: string[];
  title: string; title_mode: Mode; status: Status; state: string; state_short: string | null; remaining: string;
  checkout: { repository: string; repository_url?: string | null; ref: string; directory_within_checkout: string } | null;
};
export type LocalContext = {
  catalog_source: string; host_id: string; stream: StreamWire | null; session: SessionWire | null;
  sessions: SessionWire[]; authority: { host_id: string; token: string } | null;
};

const object = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);
const text = (value: unknown): value is string => typeof value === 'string' && !value.includes('\0');
const identity = (value: unknown): value is string => text(value) && value.trim().length > 0;
const nullableText = (value: unknown) => value === null || text(value);
const provenance = (value: unknown) => value === undefined || value === null || identity(value);
const mode = (value: unknown): value is Mode => value === 'automatic' || value === 'manual';
const status = (value: unknown): value is Status => typeof value === 'string' && ['active', 'waiting', 'blocked', 'parked', 'done'].includes(value);
const link = (value: unknown): value is Link => object(value) && text(value.url) && text(value.description);

export function isStream(value: unknown): value is StreamWire {
  if (!object(value) || !identity(value.id) || !text(value.title) || !mode(value.title_mode)
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
    && [value.title, value.state, value.remaining].every(text) && mode(value.title_mode) && status(value.status)
    && nullableText(value.state_short)
    && (value.previous_stream_ids === undefined || Array.isArray(value.previous_stream_ids)
      && value.previous_stream_ids.every(identity)
      && value.previous_stream_ids.every((id, i, ids) => id !== (i + 1 < ids.length ? ids[i + 1] : value.stream_id)))
    && (value.checkout === null || object(value.checkout)
      && identity(value.checkout.repository) && identity(value.checkout.ref) && text(value.checkout.directory_within_checkout)
      && provenance(value.checkout.repository_url));
}

export function parseLocalContext(value: unknown): LocalContext & Record<string, unknown> {
  if (!object(value) || Object.hasOwn(value, 'error') || !identity(value.catalog_source) || !identity(value.host_id)
    || !(value.stream === null || isStream(value.stream)) || !(value.session === null || isSession(value.session))
    || !Array.isArray(value.sessions) || !value.sessions.every(isSession)
    || !(value.authority === null || object(value.authority) && identity(value.authority.host_id) && identity(value.authority.token))) {
    throw new Error('Invalid local catalog context');
  }
  return value as LocalContext & Record<string, unknown>;
}
