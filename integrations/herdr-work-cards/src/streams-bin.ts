import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const packageRoot = fileURLToPath(new URL('../../..', import.meta.url));

/** Pin subprocesses to this installation, not a sibling checkout's PATH. */
export function resolveStreamsBin(env: NodeJS.ProcessEnv = process.env): string {
  const own = join(packageRoot, '.venv', 'bin', 'streams');
  return env.STREAMS_BIN_PATH || (existsSync(own) ? own : 'streams');
}
