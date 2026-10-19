import { spawn } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { constants } from 'node:fs';
import { access, link, lstat, open, stat, unlink, type FileHandle } from 'node:fs/promises';
import { dirname, isAbsolute, join } from 'node:path';

const marker = 'work-cards workspace lock v1\n';
const startupMs = 5000;
// flock belongs to the open file description shared with Node's retained fd, not
// to this short-lived helper. stdin EOF prevents an orphaned waiter acquiring.
const helper = `import fcntl, os, select, sys, time
budget = float(sys.argv[1]) / 1000
started = time.monotonic()
os.write(2, b'ready\\n')
while True:
    try:
        fcntl.flock(3, fcntl.LOCK_EX | fcntl.LOCK_NB)
        sys.exit(0)
    except BlockingIOError:
        remaining = budget - (time.monotonic() - started)
        if remaining <= 0:
            sys.exit(2)
        if select.select([0], [], [], min(0.025, remaining))[0]:
            sys.exit(3)
        if time.monotonic() - started >= budget:
            sys.exit(2)
`;

function inspectionError(path: string): Error {
  return new Error(`Unsafe or legacy work-cards lock at ${path}. Inspect the file and stop all legacy clients before activation or manual recovery; never automatically remove or steal this lock.`);
}

async function openLock(path: string): Promise<FileHandle> {
  // A legacy owner may unlink between our link/open. Retry that race, but not a
  // missing socket directory or an indefinitely changing pathname.
  for (let attempt = 0; attempt < 8; attempt++) {
    let handle: FileHandle;
    try { handle = await open(path, constants.O_RDWR | constants.O_NOFOLLOW | constants.O_NONBLOCK); }
    catch (error) {
      const code = (error as NodeJS.ErrnoException).code;
      if (code !== 'ENOENT') {
        const entry = await lstat(path).catch(() => undefined);
        if (code === 'ELOOP' || code === 'EISDIR' || (entry && (!entry.isFile() || entry.uid !== process.getuid!()))) throw inspectionError(path);
        throw error;
      }
      const temporary = `${path}.${process.pid}.${randomUUID()}.tmp`;
      const initial = await open(temporary, 'wx', 0o600);
      try {
        await initial.writeFile(marker);
        await initial.sync();
        try { await link(temporary, path); }
        catch (error) { if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error; }
      } finally {
        try { await initial.close(); } finally { await unlink(temporary); }
      }
      // Also makes publication durable across a crash, not just atomic to readers.
      const directory = await open(dirname(path), constants.O_RDONLY);
      try { await directory.sync(); } finally { await directory.close(); }
      continue;
    }
    try {
      const identity = await handle.stat();
      if (!identity.isFile() || identity.uid !== process.getuid!() || identity.size !== Buffer.byteLength(marker)) throw inspectionError(path);
      const contents = Buffer.alloc(Buffer.byteLength(marker) + 1);
      const { bytesRead } = await handle.read(contents, 0, contents.length, 0);
      if (contents.subarray(0, bytesRead).toString() !== marker) throw inspectionError(path);
      return handle;
    } catch (error) { await handle.close(); throw error; }
  }
  throw new Error(`Work-cards lock pathname keeps changing at ${path}; inspect and stop legacy clients before retrying.`);
}

async function pythonExecutable(): Promise<string> {
  // PATH is caller-owned; ignore relative entries so the workspace cannot supply
  // an interpreter accidentally. Absolute fallbacks also support minimal PATHs.
  const candidates = [...(process.env.PATH ?? '').split(':').filter(isAbsolute).map(path => join(path, 'python3')),
    '/usr/bin/python3', '/usr/local/bin/python3', '/opt/homebrew/bin/python3', '/bin/python3'];
  for (const candidate of new Set(candidates)) {
    try { await access(candidate, constants.X_OK); if ((await stat(candidate)).isFile()) return candidate; }
    catch { /* Try the next executable location. */ }
  }
  throw new Error('Work-cards locking requires python3 with fcntl; install Python in a trusted PATH or standard system location.');
}

async function acquire(handle: FileHandle, path: string, waitMs: number, signal?: AbortSignal): Promise<void> {
  const executable = await pythonExecutable();
  signal?.throwIfAborted();
  const env = { ...process.env };
  delete env.PYTHONHOME;
  delete env.PYTHONPATH;
  await new Promise<void>((resolve, reject) => {
    const child = spawn(executable, ['-I', '-S', '-c', helper, String(Math.max(0, waitMs))], {
      env, stdio: ['pipe', 'ignore', 'pipe', handle.fd],
    });
    let failure: unknown, stderr = '', ready = false;
    const stop = (error: unknown) => {
      if (failure === undefined) failure = error;
      child.kill('SIGKILL');
    };
    const abort = () => stop(signal!.reason);
    const startup = setTimeout(() => stop(new Error(`Work-cards lock helper startup timed out at ${path}.`)), startupMs);
    let completion: ReturnType<typeof setTimeout> | undefined;
    child.stderr!.on('data', (data: Buffer) => {
      stderr = (stderr + data.toString()).slice(0, 4096);
      if (!ready && stderr.startsWith('ready\n')) {
        ready = true;
        clearTimeout(startup);
        // A separate watchdog bounds a stalled helper after it announces startup.
        completion = setTimeout(() => stop(new Error(`Work-cards lock helper stalled at ${path}.`)), Math.max(0, waitMs) + startupMs);
      }
    });
    child.on('error', error => { failure ??= error; });
    child.once('close', code => {
      clearTimeout(startup);
      clearTimeout(completion);
      signal?.removeEventListener('abort', abort);
      child.stdin!.destroy();
      // Rejection happens only after close: the child can no longer retain fd3.
      if (failure !== undefined) reject(failure);
      else if (code === 0 && ready) resolve();
      else if (code === 2) reject(new Error(waitMs > 0
        ? `Work-cards lock acquisition timed out at ${path}; lock retained by its owner, never stolen.`
        : `Work-cards lock is busy at ${path}. Inspect the owner/process; retry on the next poll or explicit open, never remove the lock.`));
      else reject(new Error(`Work-cards lock helper failed at ${path} (exit ${code}): ${stderr.trim()}`));
    });
    signal?.addEventListener('abort', abort, { once: true });
    if (signal?.aborted) abort();
  });
}

// All cooperating callers use the socket's directory, not their possibly different TMPDIRs.
export async function withWorkspaceLock<T>(socket: string, workspaceId: string, operation: () => Promise<T>, options: { waitMs?: number; signal?: AbortSignal } = {}): Promise<T> {
  const key = createHash('sha256').update(`${socket}\0${workspaceId}`).digest('hex');
  const path = join(dirname(socket), `.work-cards-${key}.lock`);
  const { signal, waitMs = 0 } = options;
  signal?.throwIfAborted();
  const lock = await openLock(path);
  try {
    await acquire(lock, path, waitMs, signal);
    signal?.throwIfAborted();
    const [identity, current] = await Promise.all([lock.stat(), lstat(path)]);
    if (!current.isFile() || current.dev !== identity.dev || current.ino !== identity.ino) throw inspectionError(path);
    signal?.throwIfAborted();
    // Only acquisition is retried. A callback failure may represent partial native effects.
    return await operation();
  } finally { await lock.close(); }
}
