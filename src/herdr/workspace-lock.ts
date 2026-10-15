import { createHash } from 'node:crypto';
import { open, unlink } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

// All cooperating callers use the socket's directory, not their possibly different TMPDIRs.
export async function withWorkspaceLock<T>(socket: string, workspaceId: string, operation: () => Promise<T>, options: { waitMs?: number; signal?: AbortSignal } = {}): Promise<T> {
  const key = createHash('sha256').update(`${socket}\0${workspaceId}`).digest('hex');
  const path = join(dirname(socket), `.work-cards-${key}.lock`);
  const { signal, waitMs = 0 } = options, deadline = performance.now() + waitMs;
  const timeout = () => new Error(`Work-cards lock acquisition timed out at ${path}; lock retained by its owner, never stolen.`);
  let lock, waited = false;
  while (!lock) {
    signal?.throwIfAborted();
    if (waited && performance.now() >= deadline) throw timeout();
    try { lock = await open(path, 'wx', 0o600); }
    catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error;
      if (waitMs <= 0) throw new Error(`Work-cards lock exists at ${path}. Inspect the owner/process before manually removing the lock; never steal it. Retry on the next poll or explicit open.`);
      const remaining = deadline - performance.now();
      if (remaining <= 0) throw timeout();
      waited = true;
      await delay(Math.min(25, remaining), undefined, { signal });
    }
  }
  try {
    await lock.writeFile(`pid=${process.pid}\n`);
    signal?.throwIfAborted();
    if (waited && performance.now() >= deadline) throw timeout();
    // Only acquisition is retried. A callback failure may represent partial native effects.
    return await operation();
  } finally { try { await lock.close(); } finally { await unlink(path); } }
}

