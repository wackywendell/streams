import { createHash, randomUUID } from 'node:crypto';
import { constants } from 'node:fs';
import { lstat, mkdir, open, rename, unlink } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { isDeepStrictEqual } from 'node:util';

export const streamsDigest = (value: unknown): string => createHash('sha256').update(JSON.stringify(value)).digest('hex');
interface StreamsAttemptIdentity { version: 1; key: string; nonce: string }
export interface ResourceTuple { paneId: string; terminalId: string; tabId: string }
export type UnlaunchedStreamsAttempt = StreamsAttemptIdentity & {
  phase: 'pending'; paneId?: never; terminalId?: never; tabId?: never;
};
export type ReservedStreamsAttempt = StreamsAttemptIdentity & ResourceTuple & { phase: 'pending' };
export type StreamsAttempt = UnlaunchedStreamsAttempt | ReservedStreamsAttempt
  | (StreamsAttemptIdentity & ResourceTuple & { phase: 'ready' })
  | (StreamsAttemptIdentity & ResourceTuple & { phase: 'closed' });
export const isReservedStreamsAttempt = (attempt: StreamsAttempt): attempt is ReservedStreamsAttempt =>
  attempt.phase === 'pending' && attempt.paneId !== undefined;
/** Host-local launch evidence, never a catalog. Caller holds the native workspace lock.
 * Pending evidence is published and synced BEFORE launching, and is never automatically
 * discarded on uncertain failures. Even an explicit retry cannot duplicate that launch.
 */
export class StreamsNativeState {
  readonly directory: string;
  constructor(directory: string) { this.directory = resolve(directory); }
  async prepare(): Promise<void> {
    await mkdir(this.directory, { recursive: true, mode: 0o700 });
    if (!(await lstat(this.directory)).isDirectory()) throw new Error('Native state must be a real directory, not a leaf symlink');
  }
  private path(key: string): string {
    if (!/^[a-f0-9]{64}$/.test(key)) throw new Error('Invalid native state key');
    return join(this.directory, `${key}.json`);
  }
  async read(key: string): Promise<StreamsAttempt | null> {
    await this.prepare();
    let file;
    try { file = await open(this.path(key), constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK); }
    catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') return null;
      throw new Error('Uncertain native launch evidence is unavailable; inspect before retrying', { cause: error });
    }
    try {
      if (!(await file.stat()).isFile()) throw new Error('Native evidence must be a regular file');
      const source = await file.readFile('utf8');
      const parsed: unknown = JSON.parse(source);
      if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) throw new Error('Malformed native launch evidence');
      const value = parsed as Record<string, unknown>;
      if (value.version !== 1 || value.key !== key || !['pending', 'ready', 'closed'].includes(value.phase as string)
        || typeof value.nonce !== 'string' || !/^[A-Za-z0-9_-]+$/.test(value.nonce)) throw new Error('Malformed native launch evidence');
      const resources = [value.paneId, value.terminalId, value.tabId];
      if (!resources.every(v => typeof v === 'string' && v.length > 0)
        && !(value.phase === 'pending' && resources.every(v => v === undefined))) throw new Error('Incomplete native launch evidence');
      return parsed as StreamsAttempt;
    } finally { await file.close(); }
  }
  async discardUnlaunched(expected: StreamsAttempt, previous: StreamsAttempt | null): Promise<void> {
    if (expected.phase !== 'pending' || previous && (previous.key !== expected.key || previous.phase === 'pending')
      || !isDeepStrictEqual(await this.read(expected.key), expected)) throw new Error('Native reservation changed; unlaunched rollback refused');
    if (previous) { await this.write(previous); return; }
    const path = this.path(expected.key), directory = await open(this.directory, constants.O_RDONLY);
    try { await unlink(path); await directory.sync(); }
    finally { await directory.close(); }
  }
  async write(value: StreamsAttempt): Promise<void> {
    await this.prepare();
    const current = await this.read(value.key);
    if (isDeepStrictEqual(current, value)) return;
    const path = this.path(value.key), temporary = join(this.directory, `.${randomUUID()}.tmp`);
    const file = await open(temporary, 'wx', 0o600);
    try {
      try { await file.writeFile(JSON.stringify(value) + '\n'); await file.sync(); } finally { await file.close(); }
      await rename(temporary, path);
      const directory = await open(this.directory, constants.O_RDONLY);
      try { await directory.sync(); } finally { await directory.close(); }
    } finally { try { await unlink(temporary); } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; } }
  }
}
