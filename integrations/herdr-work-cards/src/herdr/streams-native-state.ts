import { createHash, randomUUID } from 'node:crypto';
import { constants } from 'node:fs';
import { lstat, mkdir, open, rename, unlink } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { isDeepStrictEqual } from 'node:util';
import { parseDocument } from 'yaml';

export const streamsDigest = (value: unknown): string => createHash('sha256').update(JSON.stringify(value)).digest('hex');
export interface StreamsAttempt {
  version: 1; key: string; phase: 'pending' | 'ready' | 'closed'; nonce: string;
  paneId?: string; terminalId?: string; tabId?: string;
}
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
    let pending = false;
    try {
      if (!(await lstat(`${this.path(key)}.pending`)).isFile()) throw new Error('Native pending evidence must be a regular file');
      pending = true;
    } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
    let file;
    try { file = await open(this.path(key), constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK); }
    catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT' && !pending) return null;
      throw new Error('Uncertain native launch evidence is unavailable; inspect before retrying', { cause: error });
    }
    try {
      if (!(await file.stat()).isFile()) throw new Error('Native evidence must be a regular file');
      const source = await file.readFile('utf8');
      if (parseDocument(source, { uniqueKeys: true }).errors.length) throw new Error('Malformed native launch evidence');
      const value = JSON.parse(source) as StreamsAttempt;
      if (value.version !== 1 || value.key !== key || !['pending', 'ready', 'closed'].includes(value.phase)
        || !/^[A-Za-z0-9_-]+$/.test(value.nonce) || ['paneId', 'terminalId', 'tabId'].some(k => value[k as keyof StreamsAttempt] !== undefined && typeof value[k as keyof StreamsAttempt] !== 'string')) throw new Error('Malformed native launch evidence');
      if (value.phase !== 'pending' && ![value.paneId, value.terminalId, value.tabId].every(v => typeof v === 'string' && v.length > 0)) throw new Error('Incomplete native launch evidence');
      return pending ? { ...value, phase: 'pending' } : value;
    } finally { await file.close(); }
  }
  async discardUnlaunched(expected: StreamsAttempt, previous: StreamsAttempt | null): Promise<void> {
    if (expected.phase !== 'pending' || previous && (previous.key !== expected.key || previous.phase === 'pending')
      || !isDeepStrictEqual(await this.read(expected.key), expected)) throw new Error('Native reservation changed; unlaunched rollback refused');
    const path = this.path(expected.key), directory = await open(this.directory, constants.O_RDONLY);
    try {
      // Keep the independent uncertainty gate until the prior record (or absence) is durable.
      if (previous) await this.replace(previous, true);
      else { await unlink(path); await directory.sync(); }
      try {
        await unlink(`${path}.pending`);
        await directory.sync();
      } catch (error) {
        // A failed final sync must not authorize retry from an uncertain rollback.
        await this.write(expected);
        throw error;
      }
    } finally { await directory.close(); }
  }
  async write(value: StreamsAttempt): Promise<void> { await this.replace(value); }
  private async replace(value: StreamsAttempt, retainGate = false): Promise<void> {
    await this.prepare();
    const current = await this.read(value.key);
    if (isDeepStrictEqual(current, value)) return;
    const path = this.path(value.key), temporary = join(this.directory, `.${randomUUID()}.tmp`);
    // The uncertainty gate is independent of the replaceable record. It is removed
    // only AFTER readiness bytes and their directory entry are synced. A ready
    // rename followed by fsync failure therefore cannot masquerade as completion.
    if (value.phase === 'pending') {
      const gate = await open(`${path}.pending`, constants.O_WRONLY | constants.O_CREAT | constants.O_NOFOLLOW, 0o600);
      try { if (!(await gate.stat()).isFile()) throw new Error('Invalid native uncertainty gate'); await gate.sync(); } finally { await gate.close(); }
      const directory = await open(this.directory, constants.O_RDONLY);
      try { await directory.sync(); } finally { await directory.close(); }
    }
    const file = await open(temporary, 'wx', 0o600);
    try {
      try { await file.writeFile(JSON.stringify(value) + '\n'); await file.sync(); } finally { await file.close(); }
      await rename(temporary, path);
      const directory = await open(this.directory, constants.O_RDONLY);
      try { await directory.sync(); } finally { await directory.close(); }
      if ((value.phase === 'ready' || value.phase === 'closed') && !retainGate) {
        try { await unlink(`${path}.pending`); } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
        // A crash may restore the gate and conservatively suspend a proven launch;
        // it cannot lose the already-synced ready record and authorize duplicates.
      }
    } finally { try { await unlink(temporary); } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; } }
  }
}
