import { execFileSync } from 'node:child_process';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createJiti } from 'jiti';

const jiti = createJiti(import.meta.url);
const { StreamCli } = await jiti.import('../../extensions/stream-checkpoint/streams.ts');
const root = fileURLToPath(new URL('../../', import.meta.url));

/** Real format-5 CLI transport, always isolated from the user's catalog and frontend. */
export function isolatedEnv(cwd, environment = {}) {
  const env = { ...process.env, ...environment, HOME: environment?.HOME ?? cwd };
  for (const key of Object.keys(env)) {
    if (/^(HERDR_|CMUX_|STREAMS_|PI_CODING_AGENT_|WORK_CARDS_|HC_)/.test(key)) delete env[key];
  }
  Object.assign(env, {
    GIT_CONFIG_GLOBAL: '/dev/null',
    GIT_CONFIG_SYSTEM: '/dev/null',
    GIT_CONFIG_NOSYSTEM: '1',
    XDG_CONFIG_HOME: environment?.XDG_CONFIG_HOME ?? resolve(cwd, 'config'),
    XDG_DATA_HOME: environment?.XDG_DATA_HOME ?? resolve(cwd, 'data'),
    XDG_STATE_HOME: environment?.XDG_STATE_HOME ?? resolve(cwd, 'state'),
    PI_CODING_AGENT_DIR: resolve(env.HOME, 'pi'),
    STREAMS_BIN_PATH: resolve(root, process.env.CARGO_TARGET_DIR ?? 'target', 'debug', 'streams'),
    ...environment,
  });
  return env;
}

export function initIsolatedCatalog(cwd, env) {
  execFileSync(env.STREAMS_BIN_PATH, ['catalog', 'init', '--sync', 'none', '--json'], {
    cwd,
    env,
    encoding: 'utf8',
  });
}

export function testCli(cwd, environment, options = {}) {
  const env = isolatedEnv(cwd, environment);
  if (options.init !== false) {
    initIsolatedCatalog(cwd, env, options.hostName ?? 'test-host');
  }
  const cli = new StreamCli(cwd, undefined, env);
  return Object.assign(cli, { env, stop: async () => {} });
}

export const childCli = (cwd, _sessionId, environment) => testCli(cwd, environment);
