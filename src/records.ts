import { createHash, randomUUID } from 'node:crypto';
import { constants } from 'node:fs';
import { link, lstat, mkdir, open, readdir, realpath, rename, stat, unlink } from 'node:fs/promises';
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { isDeepStrictEqual } from 'node:util';
import { homedir } from 'node:os';
import { parseDocument, stringify } from 'yaml';
import {
  absolute, applyOwnUpdate, applyReferenceRemoval, applySharedAdditions, documentIdentity, object, opaque, referenceIdentity,
  text, validateAgent, validateBoundary, validateMembership, validateReferenceIdentity, validateWorkspace,
  type AgentRecord, type Authority, type DocumentIdentity, type OwnUpdate, type ReferenceBoundary,
  type ReferenceIdentity, type ReferenceSelection, type Snapshot, type WorkspaceRecord,
} from './records/schema.js';
export * from './records/schema.js';

export function recordDirectory(env: NodeJS.ProcessEnv = process.env): string {
  return join(env.XDG_STATE_HOME || join(env.HOME || homedir(), '.local', 'state'), 'herdr', 'cards');
}

export interface JoinInput {
  workspaceId: string; sessionId: string; sessionFile?: string; cwd: string;
  membershipToken: string; boundary: ReferenceBoundary; title: string;
  expected?: Authority;
}

function code(error: unknown): string | undefined { return (error as NodeJS.ErrnoException)?.code; }
function hash(bytes: Buffer | string): string { return createHash('sha256').update(bytes).digest('hex'); }
function snapshot<T>(path: string, record: T, bytes: Buffer): Snapshot<T> { return { path, record, revision: hash(bytes) }; }
function revision(current: { revision: string }, expected: string): void {
  if (current.revision !== expected) throw new Error('Stale revision conflict; reread before retrying');
}
async function optional<T>(read: () => Promise<T>): Promise<T | undefined> {
  try { return await read(); } catch (e) { if (code(e) !== 'ENOENT') throw e; return undefined; }
}
async function removeTemp(path: string): Promise<void> {
  try { await unlink(path); } catch (e) { if (code(e) !== 'ENOENT') throw e; }
}
async function regularDirectory(path: string): Promise<void> {
  if (!(await lstat(path)).isDirectory()) throw new Error(`Record directory ${path} must be a real directory, not a symlink`);
}
async function readRecord<T>(path: string, validate: (v: unknown) => T): Promise<Snapshot<T>> {
  await regularDirectory(dirname(path));
  const file = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    if (!(await file.stat()).isFile()) throw new Error(`Record ${path} must be a regular file`);
    const bytes = await file.readFile();
    const yaml = parseDocument(new TextDecoder('utf-8', { fatal: true }).decode(bytes), { uniqueKeys: true });
    if (yaml.errors.length) throw new Error(`Invalid record ${path}: ${yaml.errors.map(e => e.message).join('; ')}`);
    return snapshot(path, validate(yaml.toJS({ maxAliasCount: 0 })), bytes);
  } finally { await file.close(); }
}

export async function discoverCheckout(cwd: string): Promise<string> {
  return discoverCanonicalCheckout(await realpath(absolute(cwd)));
}

async function discoverCanonicalCheckout(actual: string): Promise<string> {
  if (!(await stat(actual)).isDirectory()) throw new Error('Working directory must be a directory');
  let candidate = actual;
  for (;;) {
    for (const marker of ['.git', '.jj']) {
      const info = await optional(() => lstat(join(candidate, marker)));
      if (info && (info.isDirectory() || (marker === '.git' && info.isFile()))) return candidate;
    }
    const parent = dirname(candidate);
    if (parent === candidate) return actual;
    candidate = parent;
  }
}

/** Metadata only. Locks fail closed and are never stolen; retry from current state. */
export class RecordStore {
  readonly directory: string;
  constructor(directory: string) { this.directory = resolve(directory); }
  workspacePath(id: string): string { return join(this.directory, 'workspaces', `${opaque(id)}.yaml`); }
  agentPath(sessionId: string): string { return join(this.directory, 'agents', `${hash(text(sessionId, true))}.yaml`); }
  async readWorkspace(id: string): Promise<Snapshot<WorkspaceRecord>> {
    const result = await readRecord(this.workspacePath(id), validateWorkspace);
    if (result.record.id !== id) throw new Error('Workspace identity mismatch');
    return result;
  }
  async readAgent(sessionId: string): Promise<Snapshot<AgentRecord>> {
    const result = await readRecord(this.agentPath(sessionId), validateAgent);
    if (result.record.sessionId !== sessionId) throw new Error('Agent session identity mismatch');
    return result;
  }
  async listWorkspaces(): Promise<Snapshot<WorkspaceRecord>[]> {
    const names = await optional(() => readdir(join(this.directory, 'workspaces'))) ?? [];
    return Promise.all(names.filter(n => n.endsWith('.yaml')).sort().map(n => this.readWorkspace(n.slice(0, -5))));
  }
  async listAgents(): Promise<Snapshot<AgentRecord>[]> {
    const names = await optional(() => readdir(join(this.directory, 'agents'))) ?? [];
    return Promise.all(names.filter(n => n.endsWith('.yaml')).sort().map(async name => {
      if (!/^[a-f0-9]{64}\.yaml$/.test(name)) throw new Error('Invalid agent filename');
      const result = await readRecord(join(this.directory, 'agents', name), validateAgent);
      if (this.agentPath(result.record.sessionId) !== result.path) throw new Error('Agent filename identity mismatch');
      return result;
    }));
  }
  private async prepare(): Promise<void> {
    await mkdir(this.directory, { recursive: true, mode: 0o700 });
    for (const child of ['workspaces', 'agents']) {
      const path = join(this.directory, child);
      await mkdir(path, { recursive: true, mode: 0o700 });
      await regularDirectory(path);
    }
  }
  private async lock<T>(path: string, operation: () => Promise<T>): Promise<T> {
    let handle;
    try { handle = await open(path, 'wx', 0o600); }
    catch (e) {
      if (code(e) === 'EEXIST') throw new Error(`Record lock exists at ${path}; inspect owner/process before removing it; reread and retry`);
      throw e;
    }
    try { await handle.writeFile(`pid=${process.pid}\n`); return await operation(); }
    finally { try { await handle.close(); } finally { await unlink(path); } }
  }
  private async topology<T>(operation: () => Promise<T>, signal?: AbortSignal): Promise<T> {
    signal?.throwIfAborted();
    await this.prepare();
    return this.lock(join(this.directory, '.topology.lock'), async () => { signal?.throwIfAborted(); return operation(); });
  }
  private async records<T>(paths: string[], operation: () => Promise<T>): Promise<T> {
    const sorted = [...new Set(paths)].sort();
    const acquire = (i: number): Promise<T> => i === sorted.length ? operation() : this.lock(`${sorted[i]}.lock`, () => acquire(i + 1));
    return acquire(0);
  }
  private async publish<T>(path: string, record: T, current?: Snapshot<T>, signal?: AbortSignal): Promise<Snapshot<T>> {
    signal?.throwIfAborted();
    if (current && isDeepStrictEqual(current.record, record)) return current;
    const bytes = Buffer.from(stringify(record));
    const temporary = `${path}.${randomUUID()}.tmp`;
    const handle = await open(temporary, 'wx', 0o600);
    try {
      try { await handle.writeFile(bytes); await handle.sync(); }
      finally { await handle.close(); }
      // No await between cancellation and publication. Atomic visibility, not directory-fsync crash durability.
      signal?.throwIfAborted();
      if (current) await rename(temporary, path);
      else await link(temporary, path);
      return snapshot(path, record, bytes);
    } finally { await removeTemp(temporary); }
  }
  private async publishWorkspace(record: WorkspaceRecord, current?: Snapshot<WorkspaceRecord>, signal?: AbortSignal): Promise<Snapshot<WorkspaceRecord>> {
    const validated = validateWorkspace(record);
    if (current && isDeepStrictEqual(current.record, validated)) return this.publish(current.path, validated, current, signal);
    // A genuine mutation must not restore an old raw revision after remove/re-add (ABA).
    return this.publish(this.workspacePath(validated.id), { ...validated, mutationToken: randomUUID() }, current, signal);
  }
  private async caller(authority: Authority): Promise<Snapshot<AgentRecord>> {
    const v = object(authority, ['sessionId', 'workspaceId', 'membershipToken', 'evidenceGeneration']);
    for (const value of Object.values(v)) text(value, true);
    const a = await this.readAgent(authority.sessionId);
    if (a.record.workspaceId !== authority.workspaceId || a.record.membershipToken !== authority.membershipToken) throw new Error('Stale workspace membership');
    if (a.record.evidenceGeneration !== authority.evidenceGeneration) throw new Error('Stale evidence generation');
    return a;
  }
  async createWorkspace(input: { id: string; creationToken: string; title: string }, signal?: AbortSignal): Promise<Snapshot<WorkspaceRecord>> {
    object(input, ['id', 'creationToken', 'title']);
    const record = validateWorkspace({ version: 1, ...input, mutationToken: randomUUID(), titleMode: 'auto', goal: '', planSources: [], sourceFingerprint: null, checkouts: {}, documents: [], issues: [], prs: [], exclusions: [] });
    const path = this.workspacePath(record.id);
    return this.topology(() => this.records([path], async () => {
      const current = await optional(() => this.readWorkspace(record.id));
      if (current) {
        if (current.record.creationToken !== record.creationToken) throw new Error('Creation token conflict');
        return current;
      }
      return this.publishWorkspace(record, undefined, signal);
    }), signal);
  }
  private register(record: WorkspaceRecord, directory: string): { record: WorkspaceRecord; checkout: string } {
    const existing = Object.entries(record.checkouts).find(([, c]) => c.directory === directory);
    if (existing) return { record, checkout: existing[0] };
    const checkout = randomUUID();
    return { record: validateWorkspace({ ...record, checkouts: { ...record.checkouts, [checkout]: { directory } } }), checkout };
  }
  async join(input: JoinInput, signal?: AbortSignal): Promise<Snapshot<AgentRecord>> {
    object(input, ['workspaceId', 'sessionId', 'sessionFile', 'cwd', 'membershipToken', 'boundary', 'title', 'expected'], ['workspaceId', 'sessionId', 'cwd', 'membershipToken', 'boundary', 'title']);
    const initial = validateAgent({ version: 1, workspaceId: input.workspaceId, sessionId: input.sessionId, ...(input.sessionFile !== undefined ? { sessionFile: input.sessionFile } : {}), cwd: input.cwd, checkout: 'pending', membershipToken: input.membershipToken, boundary: input.boundary, evidenceGeneration: randomUUID(), title: input.title, titleMode: 'auto', status: 'active', now: '', remaining: '' });
    const actual = await realpath(initial.cwd);
    const directory = await discoverCanonicalCheckout(actual);
    return this.topology(() => this.records([this.workspacePath(initial.workspaceId), this.agentPath(initial.sessionId)], async () => {
      const previous = await optional(() => this.readAgent(initial.sessionId));
      if (previous && previous.record.workspaceId === initial.workspaceId && previous.record.membershipToken === initial.membershipToken) {
        if (!isDeepStrictEqual(previous.record.boundary, initial.boundary) || previous.record.cwd !== actual) throw new Error('Conflicting membership retry');
        return previous;
      }
      if (previous) {
        if (!input.expected || input.expected.sessionId !== initial.sessionId) throw new Error('Existing membership requires expected caller');
        await this.caller(input.expected);
        if (initial.membershipToken === previous.record.membershipToken) throw new Error('Move must change membership token');
      } else if (input.expected) throw new Error('Stale membership: agent absent');
      const workspace = await this.readWorkspace(initial.workspaceId);
      const registered = this.register(workspace.record, directory);
      // Safe partial progress: publish destination checkout before the agent points at it.
      await this.publishWorkspace(registered.record, workspace, signal);
      const next = validateAgent({ ...initial, ...(previous ? { title: previous.record.title, titleMode: previous.record.titleMode, status: previous.record.status, now: previous.record.now, remaining: previous.record.remaining, ...(previous.record.displayAttempt ? { displayAttempt: previous.record.displayAttempt } : {}) } : {}), cwd: actual, checkout: registered.checkout });
      validateMembership(registered.record, next);
      return this.publish(this.agentPath(initial.sessionId), next, previous, signal);
    }), signal);
  }
  private async shared(authority: Authority, change: (w: Snapshot<WorkspaceRecord>, a: Snapshot<AgentRecord>) => WorkspaceRecord, signal?: AbortSignal): Promise<Snapshot<WorkspaceRecord>> {
    return this.topology(() => this.records([this.workspacePath(authority.workspaceId), this.agentPath(authority.sessionId)], async () => {
      const a = await this.caller(authority); const w = await this.readWorkspace(authority.workspaceId);
      return this.publishWorkspace(change(w, a), w, signal);
    }), signal);
  }
  async addReferences(authority: Authority, additions: unknown, options: { explicit?: boolean; revision?: string; signal?: AbortSignal } = {}): Promise<Snapshot<WorkspaceRecord>> {
    return this.shared(authority, w => {
      if (options.revision !== undefined) revision(w, options.revision);
      return applySharedAdditions(w.record, additions, options.explicit);
    }, options.signal);
  }
  async removeReference(authority: Authority, selection: ReferenceSelection, signal?: AbortSignal): Promise<Snapshot<WorkspaceRecord>> {
    return this.shared(authority, w => applyReferenceRemoval(w, selection), signal);
  }
  async describeReference(authority: Authority, identity: ReferenceIdentity, description: string, expected: string, signal?: AbortSignal): Promise<Snapshot<WorkspaceRecord>> {
    text(description); const validated = validateReferenceIdentity(identity);
    return this.shared(authority, w => {
      revision(w, expected); const next = structuredClone(w.record);
      const ref = [...next.documents, ...next.issues, ...next.prs].find(r => referenceIdentity(r) === referenceIdentity(validated));
      if (!ref) throw new Error('Reference not found'); ref.description = description; return next;
    }, signal);
  }
  async registerCheckout(authority: Authority, cwd: string, expected: string, signal?: AbortSignal): Promise<Snapshot<WorkspaceRecord>> {
    const directory = await discoverCheckout(cwd);
    return this.shared(authority, w => { revision(w, expected); return this.register(w.record, directory).record; }, signal);
  }
  async removeCheckout(authority: Authority, checkout: string, expected: string, signal?: AbortSignal): Promise<Snapshot<WorkspaceRecord>> {
    opaque(checkout);
    return this.topology(async () => {
      const agents = await this.listAgents();
      return this.records([this.workspacePath(authority.workspaceId), this.agentPath(authority.sessionId)], async () => {
        await this.caller(authority); const w = await this.readWorkspace(authority.workspaceId); revision(w, expected);
        if (agents.some(a => a.record.workspaceId === w.record.id && a.record.checkout === checkout) || [...w.record.documents, ...w.record.exclusions].some(r => 'checkout' in r && r.checkout === checkout)) throw new Error('Checkout used by agent, reference or exclusion');
        if (!Object.hasOwn(w.record.checkouts, checkout)) throw new Error('Unknown checkout');
        const next = structuredClone(w.record); delete next.checkouts[checkout]; return this.publishWorkspace(next, w, signal);
      });
    }, signal);
  }
  private async own(authority: Authority, expected: string, change: (a: AgentRecord) => AgentRecord, signal?: AbortSignal): Promise<Snapshot<AgentRecord>> {
    return this.topology(() => this.records([this.agentPath(authority.sessionId)], async () => {
      const a = await this.caller(authority); revision(a, expected);
      return this.publish(a.path, validateAgent(change(a.record)), a, signal);
    }), signal);
  }
  async updateOwn(authority: Authority, patch: OwnUpdate, expected: string, signal?: AbortSignal): Promise<Snapshot<AgentRecord>> {
    return this.own(authority, expected, a => applyOwnUpdate(a, patch), signal);
  }
  async setOwnTitle(authority: Authority, title: string, expected: string, signal?: AbortSignal): Promise<Snapshot<AgentRecord>> {
    return this.own(authority, expected, a => ({ ...a, title, titleMode: 'manual' }), signal);
  }
  async setWorkspaceTitle(authority: Authority, title: string, expected: string, signal?: AbortSignal): Promise<Snapshot<WorkspaceRecord>> {
    return this.shared(authority, w => { revision(w, expected); return { ...w.record, title, titleMode: 'manual' }; }, signal);
  }
  async applyPlan(authority: Authority, input: { goal: string; sources: DocumentIdentity[]; fingerprint: string | null; title?: string }, expected: string, signal?: AbortSignal): Promise<Snapshot<WorkspaceRecord>> {
    object(input, ['goal', 'sources', 'fingerprint', 'title'], ['goal', 'sources', 'fingerprint']);
    if (input.title !== undefined) text(input.title, true);
    return this.shared(authority, w => {
      revision(w, expected);
      return { ...w.record, goal: input.goal, planSources: input.sources, sourceFingerprint: input.fingerprint, title: w.record.titleMode === 'auto' ? (input.title ?? w.record.title) : w.record.title };
    }, signal);
  }
  async resetEvidence(authority: Authority, boundary: ReferenceBoundary, expected: string, signal?: AbortSignal): Promise<Snapshot<AgentRecord>> {
    validateBoundary(boundary);
    return this.own(authority, expected, a => ({ ...a, boundary, evidenceGeneration: randomUUID() }), signal);
  }
  async setDisplayAttempt(authority: Authority, attempt: { attempt: string; nonce: string }, expected: string, signal?: AbortSignal): Promise<Snapshot<AgentRecord>> {
    return this.own(authority, expected, a => ({ ...a, displayAttempt: attempt }), signal);
  }
  async deleteOwn(authority: Authority, expected: string, signal?: AbortSignal): Promise<void> {
    return this.topology(() => this.records([this.agentPath(authority.sessionId)], async () => {
      const a = await this.caller(authority); revision(a, expected); signal?.throwIfAborted(); await unlink(a.path);
    }), signal);
  }
  async deleteWorkspace(id: string, expected: string, signal?: AbortSignal): Promise<void> {
    return this.topology(() => this.records([this.workspacePath(id)], async () => {
      const w = await this.readWorkspace(id); revision(w, expected);
      if ((await this.listAgents()).some(a => a.record.workspaceId === id)) throw new Error('Workspace has referring agent records');
      signal?.throwIfAborted(); await unlink(w.path);
    }), signal);
  }
}

/** Validate and resolve at open time. Selection/removal deliberately does not call this. */
export async function resolveWorkspaceDocument(workspace: WorkspaceRecord, identity: DocumentIdentity): Promise<string> {
  const ref = documentIdentity(identity);
  const checkout = workspace.checkouts[ref.checkout];
  if (!checkout) throw new Error('Unknown checkout');
  // Do not silently relocate a registered checkout after it is replaced by a symlink.
  const root = await realpath(checkout.directory);
  if (root !== checkout.directory) throw new Error('Checkout canonical directory changed');
  const target = await realpath(resolve(root, ref.path));
  const local = relative(root, target);
  if (local === '..' || local.startsWith(`..${sep}`) || isAbsolute(local)) {
    throw new Error(`Document ${ref.path} resolves outside the checkout ${root}`);
  }
  if (!(await stat(target)).isFile()) throw new Error(`Document ${ref.path} must be a regular file`);
  return target;
}

