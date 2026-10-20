import { createHash, randomUUID } from 'node:crypto';
import { constants } from 'node:fs';
import { link, lstat, mkdir, open, realpath, rename, unlink } from 'node:fs/promises';
import { isAbsolute, join, normalize, resolve } from 'node:path';
import { isDeepStrictEqual } from 'node:util';

export interface NameScope {
  storePath: string;
  socket: string;
  workspaceId: string;
  bindingNonce: string;
  kind: 'workspace' | 'tab';
  surfaceId: string;
}
export interface NameOwner { terminalId: string; nonce: string }
interface AppliedName { owner: NameOwner; lastApplied: string }
export type NameProvenance = { version: 1; scope: NameScope } & (
  | ({ mode: 'auto' } & AppliedName)
  | ({ mode: 'manual' } & (AppliedName | { owner: null; lastApplied: null }))
);
export interface NameSnapshot { path: string; revision: string; record: NameProvenance }
export interface NameInput {
  scope: NameScope;
  provenance: NameProvenance | null;
  owner: NameOwner;
  currentLabel: string;
  desiredName: string;
  /** Caller has verified this exact observed label is a native blank/numeric default. */
  defaultLabelProven: boolean;
  /** Deliberate reclaim; caller still verifies every native authority guard. */
  intent?: 'automatic' | 'explicit';
}
export type NameDecision =
  | { kind: 'noop' }
  | { kind: 'adopt'; afterSuccess: NameProvenance }
  | { kind: 'manual'; provenance: NameProvenance }
  | { kind: 'rename'; name: string; afterSuccess: NameProvenance };

function object(value: unknown, keys: string[]): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid naming object');
  const record = value as Record<string, unknown>;
  if (Object.keys(record).length !== keys.length || keys.some(k => !Object.hasOwn(record, k))) throw new Error('Invalid naming fields');
  return record;
}
function text(value: unknown, nonempty = true): string {
  if (typeof value !== 'string' || (nonempty && !value.trim()) || /[\p{Cc}\p{Cf}\p{Cs}\p{Zl}\p{Zp}]/u.test(value)) throw new Error('Invalid naming text');
  return value;
}
function token(value: unknown): string {
  const result = text(value);
  if (!/^[A-Za-z0-9_-]+$/.test(result)) throw new Error('Invalid naming identity token');
  return result;
}
function surfaceToken(value: unknown): string {
  const result = text(value);
  if (!/^[A-Za-z0-9_:-]+$/.test(result)) throw new Error('Invalid naming surface identity');
  return result;
}
function absolute(value: unknown): string {
  const path = text(value);
  if (!isAbsolute(path) || normalize(path) !== path) throw new Error('Invalid naming absolute path');
  return path;
}
function validateScope(value: unknown): NameScope {
  const v = object(value, ['storePath', 'socket', 'workspaceId', 'bindingNonce', 'kind', 'surfaceId']);
  if (v.kind !== 'workspace' && v.kind !== 'tab') throw new Error('Invalid naming surface kind');
  return { storePath: absolute(v.storePath), socket: absolute(v.socket), workspaceId: token(v.workspaceId), bindingNonce: token(v.bindingNonce), kind: v.kind, surfaceId: surfaceToken(v.surfaceId) };
}
function validateOwner(value: unknown): NameOwner {
  const v = object(value, ['terminalId', 'nonce']);
  return { terminalId: token(v.terminalId), nonce: token(v.nonce) };
}
function validateProvenance(value: unknown, scope: NameScope): NameProvenance {
  const v = object(value, ['version', 'scope', 'mode', 'owner', 'lastApplied']);
  if (v.version !== 1 || (v.mode !== 'auto' && v.mode !== 'manual')) throw new Error('Invalid naming version/mode');
  const actual = validateScope(v.scope);
  if (!isDeepStrictEqual(actual, scope)) throw new Error('Naming scope mismatch');
  if (v.mode === 'manual' && v.owner === null && v.lastApplied === null) return { version: 1, scope: actual, mode: 'manual', owner: null, lastApplied: null };
  return { version: 1, scope: actual, mode: v.mode, owner: validateOwner(v.owner), lastApplied: text(v.lastApplied) };
}

/** Pure policy, not native authority. Caller verifies eligibility/actual first pane,
 * binding, context and observed label under the native workspace lock. No predecessor
 * liveness is required. Persist manual pins immediately; persist afterSuccess ONLY
 * after verified native success (or a fresh matching-label observation for adopt).
 * A failed cache read must suspend naming, not become null.
 * A native success followed by a cache failure is partial progress: retry from a fresh
 * read/observation, preserving manual protection when a prior automatic label drifted.
 */
export function decideName(input: NameInput): NameDecision {
  const v = object(input, ['scope', 'provenance', 'owner', 'currentLabel', 'desiredName', 'defaultLabelProven', ...(Object.hasOwn(input, 'intent') ? ['intent'] : [])]);
  const scope = validateScope(v.scope); const owner = validateOwner(v.owner);
  const current = text(v.currentLabel, false); const desired = text(v.desiredName);
  if (typeof v.defaultLabelProven !== 'boolean') throw new Error('Invalid default-label proof');
  const prior = v.provenance === null ? null : validateProvenance(v.provenance, scope);
  if (v.intent !== undefined && v.intent !== 'automatic' && v.intent !== 'explicit') throw new Error('Invalid naming intent');
  if (v.intent === 'explicit') {
    const afterSuccess: NameProvenance = { version: 1, scope, mode: 'auto', owner, lastApplied: desired };
    return current === desired ? { kind: 'adopt', afterSuccess } : { kind: 'rename', name: desired, afterSuccess };
  }
  if (prior?.mode === 'manual') return { kind: 'manual', provenance: prior };
  if (!prior && current === desired) return { kind: 'adopt', afterSuccess: { version: 1, scope, mode: 'auto', owner, lastApplied: desired } };
  if (prior ? current !== prior.lastApplied : !(v.defaultLabelProven && /^\s*\d*\s*$/.test(current))) {
    const provenance: NameProvenance = prior
      ? { ...prior, mode: 'manual' }
      : { version: 1, scope, mode: 'manual', owner: null, lastApplied: null };
    return { kind: 'manual', provenance };
  }
  if (current === desired) return { kind: 'noop' };
  return { kind: 'rename', name: desired, afterSuccess: { version: 1, scope, mode: 'auto', owner, lastApplied: desired } };
}

function code(error: unknown): string | undefined { return (error as NodeJS.ErrnoException)?.code; }
function hash(bytes: string | Buffer): string { return createHash('sha256').update(bytes).digest('hex'); }
async function directory(path: string): Promise<void> {
  if (!(await lstat(path)).isDirectory()) throw new Error('Naming cache requires a real directory, not a symlink');
}
async function removeTemp(path: string): Promise<void> {
  try { await unlink(path); } catch (e) { if (code(e) !== 'ENOENT') throw e; }
}

/** Local native-surface provenance only. The existing state root must exist.
 * Canonical aliases share keys; owners/roles never participate in filenames.
 *
 * Caller MUST hold the existing native workspace lock across read, context/label
 * recheck, native rename/verification and write. No native lock is acquired here,
 * and no store transaction may acquire one. Raw-byte CAS detects stale snapshots;
 * it is not an atomic compare-and-swap against uncooperating filesystem writers.
 * Native user actions are not frozen by the lock. Publication is atomically visible,
 * not a claim of directory-fsync crash durability or native/cache atomicity.
 */
export class NameStore {
  readonly directory: string;
  constructor(directory: string) { this.directory = resolve(absolute(directory)); }

  async scope(input: Omit<NameScope, 'storePath'>): Promise<NameScope> {
    const v = object(input, ['socket', 'workspaceId', 'bindingNonce', 'kind', 'surfaceId']);
    const canonical = await realpath(this.directory); await directory(canonical);
    return validateScope({ ...v, storePath: canonical });
  }

  path(scope: NameScope): string {
    const valid = validateScope(scope);
    return join(valid.storePath, '.native-names', `${hash(JSON.stringify(valid))}.json`);
  }

  private async root(scope: NameScope): Promise<string> {
    const valid = validateScope(scope);
    if (await realpath(this.directory) !== valid.storePath) throw new Error('Naming store canonical identity changed');
    await directory(valid.storePath);
    return join(valid.storePath, '.native-names');
  }

  /** Missing is null; malformed, mismatched, symlink and nonregular entries throw. Never creates files. */
  async read(scope: NameScope): Promise<NameSnapshot | null> {
    const cache = await this.root(scope);
    try { await directory(cache); } catch (e) { if (code(e) === 'ENOENT') return null; throw e; }
    const path = this.path(scope);
    // lstat distinguishes dangling symlinks from genuinely missing entries.
    try { if (!(await lstat(path)).isFile()) throw new Error('Naming cache must be a regular file, not a symlink'); }
    catch (e) { if (code(e) === 'ENOENT') return null; throw e; }
    const file = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
    try {
      if (!(await file.stat()).isFile()) throw new Error('Naming cache must be a regular file');
      const bytes = await file.readFile();
      const source = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
      const parsed: unknown = JSON.parse(source);
      const record = validateProvenance(parsed, validateScope(scope));
      return { path, record, revision: hash(bytes) };
    } finally { await file.close(); }
  }

  /** expected=null is no-clobber creation; otherwise supply the last raw read revision.
   * Never call with rename intent before verified native success. Semantic no-ops
   * preserve existing bytes/mtime, including harmless JSON formatting differences.
   */
  async write(scope: NameScope, record: NameProvenance, expected: string | null, signal?: AbortSignal): Promise<NameSnapshot> {
    const valid = validateScope(scope); const next = validateProvenance(record, valid);
    if (expected !== null && !/^[a-f0-9]{64}$/.test(expected)) throw new Error('Invalid naming revision');
    signal?.throwIfAborted();
    const cache = await this.root(valid);
    const current = await this.read(valid);
    if ((current?.revision ?? null) !== expected) throw new Error('Naming revision conflict; reread before retrying');
    if (current && isDeepStrictEqual(current.record, next)) { signal?.throwIfAborted(); return current; }
    try { await mkdir(cache, { mode: 0o700 }); } catch (e) { if (code(e) !== 'EEXIST') throw e; }
    await directory(cache);
    const path = this.path(valid); const temporary = join(cache, `.${randomUUID()}.tmp`);
    const bytes = Buffer.from(JSON.stringify(next) + '\n');
    const file = await open(temporary, 'wx', 0o600);
    try {
      try { await file.writeFile(bytes); await file.sync(); } finally { await file.close(); }
      // No await between the last cancellation check and publication.
      signal?.throwIfAborted();
      if (current) await rename(temporary, path);
      else await link(temporary, path);
      return { path, record: next, revision: hash(bytes) };
    } finally { await removeTemp(temporary); }
  }
}
