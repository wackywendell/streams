import { spawn } from 'node:child_process';

/** Cancellation settles only after the command's process group and stdio have closed. */
export function runProcess(command: string, args: string[], options: {
  env: NodeJS.ProcessEnv; cwd?: string; timeoutMs: number; maxBuffer?: number; signal?: AbortSignal;
}): Promise<string> {
  const { signal, timeoutMs, maxBuffer = 1024 * 1024 } = options;
  signal?.throwIfAborted();
  if (!Number.isSafeInteger(timeoutMs) || timeoutMs <= 0 || !Number.isSafeInteger(maxBuffer) || maxBuffer <= 0) {
    throw new Error('Process deadline and output limit must be positive integers');
  }
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd: options.cwd, env: options.env, detached: true, stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '', stderr = '', bytes = 0, failure: Error | undefined, terminationFailure: Error | undefined;
    const terminate = (error: Error) => {
      failure ??= error;
      if (child.pid) {
        try { process.kill(-child.pid, 'SIGKILL'); }
        catch (cause) { if ((cause as NodeJS.ErrnoException).code !== 'ESRCH') terminationFailure ??= cause as Error; }
      }
    };
    const abort = () => terminate(signal?.reason instanceof Error ? signal.reason : new Error('Process aborted'));
    signal?.addEventListener('abort', abort, { once: true });
    const timer = setTimeout(() => terminate(new Error(`${command} timed out; effects may be partial`)), timeoutMs);
    for (const [stream, append] of [
      [child.stdout, (text: string) => { stdout += text; }],
      [child.stderr, (text: string) => { stderr += text; }],
    ] as const) {
      stream.setEncoding('utf8').on('data', (text: string) => {
        bytes += Buffer.byteLength(text);
        if (bytes > maxBuffer) terminate(new Error(`${command} output limit exceeded; effects may be partial`));
        else append(text);
      });
    }
    child.on('error', error => { failure ??= error; });
    child.on('close', (code, exitSignal) => {
      clearTimeout(timer); signal?.removeEventListener('abort', abort);
      if (failure && terminationFailure) reject(new AggregateError([failure, terminationFailure],
        `${failure.message}; process group termination failed: ${terminationFailure.message}; effects may be partial`, { cause: failure }));
      else if (failure) reject(failure);
      else if (code !== 0) reject(new Error(`${command} failed (${code ?? exitSignal}): ${stderr}`));
      else resolve(stdout);
    });
    if (signal?.aborted) abort();
  });
}
