import { isAbsolute, normalize } from 'node:path';
import { resolveStreamsBin } from '../streams-bin.js';
import { runProcess } from './subprocess.js';
import type { CardStatus } from './view.js';

export const STREAMS_READ_ENV_KEYS = ['HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_STATE_HOME', 'PI_CODING_AGENT_DIR', 'HERDR_BIN_PATH', 'STREAMS_BIN_PATH', 'PATH'] as const;

export interface StreamsCardScope {
  catalogSource: string; hostId: string; streamId: string; sessionId: string; nativeSessionId: string;
}
export interface StreamsDocument {
  repository: string; ref: string; path: string; description: string;
  commit?: string; source_session_id?: string;
}
export interface StreamsCheckoutBinding { repository: string; checkout_id: string; root: string }
export type StreamsDocumentDescriptor =
  | { kind: 'local' | 'origin'; checkout_id: string; root: string; file: string; revision?: never }
  | { kind: 'revision'; checkout_id: string; root: string; file: null; revision: string };
export type StreamsDocumentAlternate = StreamsDocumentDescriptor & { label: string };
export type StreamsDocumentLocation = {
  repository: string; path: string; alternates?: readonly StreamsDocumentAlternate[];
} & ((StreamsDocumentDescriptor & { error: null })
  | { kind: null; checkout_id: string | null; root: string | null; file: null; revision?: never; error: string });
export interface StreamsNativeOwner { frontend: 'herdr'; pane_id: string; tab_id: string; workspace_id: string }
export interface StreamsStream {
  id: string; title: string; title_mode: string; goal: string | null; goal_short: string | null;
  repositories: Record<string, { url: string }>;
  documents: StreamsDocument[];
  issues: { url: string; description: string }[];
  prs: { url: string; description: string }[];
}
export interface StreamsSession {
  id: string; host_id: string; stream_id: string; harness: string; harness_session_id: string;
  title: string; title_mode: string; status: CardStatus; state: string; state_short: string | null; remaining: string;
}
export interface StreamsCardSnapshot {
  scope: StreamsCardScope; stream: StreamsStream; session: StreamsSession; native: StreamsNativeOwner;
  checkouts: readonly StreamsCheckoutBinding[]; documentLocations: readonly StreamsDocumentLocation[];
}
const object = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);
const text = (value: unknown): value is string => typeof value === 'string' && !value.includes('\0');
const identity = (value: unknown): value is string => text(value) && value.trim().length > 0;
const absolute = (value: unknown): value is string => identity(value) && isAbsolute(value) && normalize(value) === value;
const uuid = (value: unknown): value is string => typeof value === 'string' && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value);
const statuses = ['active', 'waiting', 'blocked', 'parked', 'done'];
const scopeKeys = ['catalogSource', 'hostId', 'streamId', 'sessionId', 'nativeSessionId'] as const;
export function sameStreamsScope(left: StreamsCardScope, right: StreamsCardScope): boolean {
  return scopeKeys.every(key => left[key] === right[key]);
}
function session(value: unknown): value is StreamsSession {
  return object(value) && [value.id, value.host_id, value.stream_id, value.harness, value.harness_session_id].every(identity)
    && [value.title, value.title_mode, value.state, value.remaining].every(text) && (value.state_short === null || text(value.state_short))
    && typeof value.status === 'string' && statuses.includes(value.status);
}
function stream(value: unknown): value is StreamsStream {
  const link = (item: unknown) => object(item) && text(item.url) && text(item.description);
  if (!object(value) || !identity(value.id) || ![value.title, value.title_mode].every(text) || !(value.goal_short === null || text(value.goal_short))
    || !(value.goal === null || text(value.goal)) || !object(value.repositories)
    || !Object.entries(value.repositories).every(([alias, repo]) => identity(alias) && object(repo) && identity(repo.url))
    || !Array.isArray(value.documents) || !value.documents.every(doc => object(doc)
      && [doc.repository, doc.ref, doc.path].every(identity) && text(doc.description)
      && (doc.commit === undefined || identity(doc.commit))
      && (doc.source_session_id === undefined || identity(doc.source_session_id))
      && Object.hasOwn(value.repositories as object, doc.repository as string))
    || !Array.isArray(value.issues) || !value.issues.every(link) || !Array.isArray(value.prs) || !value.prs.every(link)) return false;
  return new Set(value.documents.map(doc => JSON.stringify([doc.repository, doc.path]))).size === value.documents.length;
}
function context(value: unknown, nativeSessionId: string): StreamsCardSnapshot {
  if (!object(value) || Object.hasOwn(value, 'error') || !identity(value.catalog_source) || !identity(value.host_id)
    || !stream(value.stream) || !session(value.session) || !Array.isArray(value.sessions) || !value.sessions.every(session)
    || !(value.authority === null || object(value.authority) && identity(value.authority.host_id) && identity(value.authority.token))) {
    throw new Error('Invalid or unbound Streams context; exact Stream and Session records are required');
  }
  const own = value.session;
  if (own.host_id !== value.host_id || own.harness !== 'pi' || own.harness_session_id !== nativeSessionId
    || own.stream_id !== value.stream.id || value.sessions.filter(s => s.host_id === own.host_id
      && s.harness === 'pi' && s.harness_session_id === nativeSessionId).length !== 1
    || !value.sessions.some(s => s.id === own.id && s.host_id === own.host_id && s.stream_id === own.stream_id
      && s.harness === 'pi' && s.harness_session_id === nativeSessionId)) {
    throw new Error('Streams context does not match the exact host/pi/native session and Stream');
  }
  if (!object(value.native) || value.native.frontend !== 'herdr'
    || ![value.native.pane_id, value.native.tab_id, value.native.workspace_id].every(identity)) throw new Error('Missing exact Herdr native attestation');
  const repositories = value.stream.repositories;
  if (!Array.isArray(value.checkouts) || !value.checkouts.every(b => object(b) && identity(b.repository)
    && Object.hasOwn(repositories, b.repository) && identity(b.checkout_id) && absolute(b.root))) throw new Error('Invalid Streams checkout binding');
  const checkouts = value.checkouts as unknown as StreamsCheckoutBinding[];
  if (new Set(checkouts.map(b => JSON.stringify([b.repository, b.checkout_id]))).size !== checkouts.length) throw new Error('Duplicate Streams checkout binding');
  if (!Array.isArray(value.document_locations) || value.document_locations.length !== value.stream.documents.length) throw new Error('Exactly one app document location is required per reference');
  const locations = value.document_locations;
  for (const doc of value.stream.documents) {
    const matches = locations.filter(l => object(l) && l.repository === doc.repository && l.path === doc.path);
    if (matches.length !== 1) throw new Error('Missing or duplicate app document location');
    const l = matches[0] as Record<string, unknown>;
    const descriptor = (a: Record<string, unknown>) => identity(a.checkout_id) && absolute(a.root)
      && (a.kind === 'revision'
        ? a.file === null && identity(a.revision) && a.revision === (doc.commit ?? doc.ref)
        : (a.kind === 'local' || a.kind === 'origin') && absolute(a.file) && a.revision === undefined);
    if (l.error === null ? !descriptor(l) : !(identity(l.error) && l.kind === null && l.file === null
      && l.revision === undefined && (l.checkout_id === null || identity(l.checkout_id))
      && (l.root === null || absolute(l.root)) && (l.checkout_id === null) === (l.root === null))) throw new Error('Invalid app document location');
    if (l.checkout_id !== null && !checkouts.some(b => b.repository === doc.repository && b.checkout_id === l.checkout_id && b.root === l.root)) throw new Error('App document location has no matching checkout binding');
    if (l.alternates !== undefined) {
      if (!Array.isArray(l.alternates) || !l.alternates.every(a => object(a)
        && descriptor(a) && identity(a.label)
        && checkouts.some(b => b.repository === doc.repository && b.checkout_id === a.checkout_id && b.root === a.root))) {
        throw new Error('Invalid app document alternate location');
      }
    }
  }
  // Never retain authority or another Session's progress in a UI snapshot.
  return structuredClone({
    scope: { catalogSource: value.catalog_source, hostId: value.host_id, streamId: value.stream.id, sessionId: own.id, nativeSessionId },
    stream: value.stream, session: own, native: value.native as unknown as StreamsNativeOwner,
    checkouts, documentLocations: locations as unknown as StreamsDocumentLocation[],
  });
}
export interface StreamsCardSourceOptions {
  nativeSessionId: string;
  expectedScope?: StreamsCardScope;
  readContext: (nativeSessionId: string, signal?: AbortSignal) => Promise<unknown>;
}
export class StreamsCardSource {
  private captured?: StreamsCardScope;
  private switched = false;
  private readonly options: StreamsCardSourceOptions;
  constructor(options: StreamsCardSourceOptions) {
    if (!uuid(options.nativeSessionId)) throw new Error('A native Pi session UUID is required');
    this.options = { ...options };
    this.captured = options.expectedScope && { ...options.expectedScope };
  }
  async read(signal?: AbortSignal): Promise<StreamsCardSnapshot> {
    signal?.throwIfAborted();
    if (this.switched) throw new Error('Streams scope changed; create a new source for the new attachment');
    const value = await this.options.readContext(this.options.nativeSessionId, signal);
    signal?.throwIfAborted();
    const fresh = context(value, this.options.nativeSessionId);
    if (this.captured && !sameStreamsScope(this.captured, fresh.scope)) {
      this.switched = true;
      throw new Error('Streams scope changed; create a new source for the new attachment');
    }
    this.captured ??= { ...fresh.scope };
    if (this.switched) throw new Error('Streams scope changed during read');
    return fresh;
  }
}
export interface StreamsContextReaderOptions {
  cwd?: string; env?: NodeJS.ProcessEnv; timeoutMs?: number; maxBuffer?: number;
  owner?: StreamsNativeOwner;
}
/** Owner-targeted read-only app boundary. Native effects separately verify live tuples. */
export function createStreamsContextReader(options: StreamsContextReaderOptions = {}): (nativeSessionId: string, signal?: AbortSignal) => Promise<unknown> {
  const { cwd, timeoutMs = 15000, maxBuffer = 1024 * 1024, owner } = options;
  const env = { ...(options.env ?? process.env), ...(owner ? {
    HERDR_PANE_ID: owner.pane_id, HERDR_TAB_ID: owner.tab_id, HERDR_WORKSPACE_ID: owner.workspace_id,
  } : {}) };
  if (!Number.isSafeInteger(timeoutMs) || timeoutMs <= 0 || !Number.isSafeInteger(maxBuffer) || maxBuffer <= 0) throw new Error('Streams CLI timeout and output limit must be positive integers');
  return async (nativeSessionId, signal) => {
    signal?.throwIfAborted();
    if (!uuid(nativeSessionId)) throw new Error('A native Pi session UUID is required');
    let output: string;
    try {
      output = await runProcess(resolveStreamsBin(env), ['agent', 'context', '--harness', 'pi', '--session-id', nativeSessionId, '--attest', '--json'],
        { cwd, env, timeoutMs, maxBuffer, signal });
    } catch (error) { throw new Error(`Streams context read failed: ${(error as Error).message}`, { cause: error }); }
    let value: unknown;
    try { value = JSON.parse(output); } catch { throw new Error('Streams context returned malformed JSON'); }
    const saved = context(value, nativeSessionId);
    if (owner && (saved.native.pane_id !== owner.pane_id || saved.native.tab_id !== owner.tab_id || saved.native.workspace_id !== owner.workspace_id)) throw new Error('App attestation does not match the exact requested owner tuple');
    return value;
  };
}
