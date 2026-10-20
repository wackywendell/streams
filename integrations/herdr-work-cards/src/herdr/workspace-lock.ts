import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants } from 'node:fs';
import { access, lstat, open, stat, type FileHandle } from 'node:fs/promises';
import { dirname, isAbsolute, join } from 'node:path';

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
  return new Error(`Unsafe work-cards lock at ${path}. Inspect the file; never automatically remove or steal this lock.`);
}

async function openLock(path: string): Promise<FileHandle> {
  let handle: FileHandle;
  try { handle = await open(path, constants.O_CREAT | constants.O_RDWR | constants.O_NOFOLLOW | constants.O_NONBLOCK, 0o600); }
  catch (error) {
    const code = (error as NodeJS.ErrnoException).code;
    const entry = await lstat(path).catch(() => undefined);
    if (code === 'ELOOP' || code === 'EISDIR' || (entry && (!entry.isFile() || entry.uid !== process.getuid!()))) throw inspectionError(path);
    throw error;
  }
  try {
    const identity = await handle.stat();
    if (!identity.isFile() || identity.uid !== process.getuid!()) throw inspectionError(path);
    return handle;
  } catch (error) { await handle.close(); throw error; }
}

let cachedPython: { pathEnv: string; executable: string } | undefined;

async function pythonExecutable(): Promise<string> {
  const pathEnv = process.env.PATH ?? '';
  if (cachedPython && cachedPython.pathEnv === pathEnv) return cachedPython.executable;
  // PATH is caller-owned; ignore relative entries so the workspace cannot supply
  // an interpreter accidentally. Absolute fallbacks also support minimal PATHs.
  const candidates = [...pathEnv.split(':').filter(isAbsolute).map(path => join(path, 'python3')),
    '/usr/bin/python3', '/usr/local/bin/python3', '/opt/homebrew/bin/python3', '/bin/python3'];
  for (const candidate of new Set(candidates)) {
    try { await access(candidate, constants.X_OK); if ((await stat(candidate)).isFile()) { cachedPython = { pathEnv, executable: candidate }; return candidate; } }
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
