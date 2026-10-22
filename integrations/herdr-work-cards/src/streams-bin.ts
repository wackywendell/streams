import { accessSync, constants, realpathSync, statSync } from 'node:fs';
import { isAbsolute, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const packageRoot = fileURLToPath(new URL('../../..', import.meta.url));

function executable(path: string, selector: string): string {
  if (!isAbsolute(path)) throw Error(`${selector} must name an absolute executable path`);
  try {
    const binary = realpathSync(path);
    if (!statSync(binary).isFile()) throw Error('not a regular file');
    accessSync(binary, constants.X_OK);
    return binary;
  } catch (error) {
    throw Error(`${selector} must name a regular executable: ${path} (${String(error)})`);
  }
}

/** Pin reporting and native subprocesses to the same Rust CLI, never PATH or .venv. */
export function resolveStreamsBin(env: NodeJS.ProcessEnv = process.env): string {
  if (env.STREAMS_BIN_PATH !== undefined || env.STREAMS_RUST_BIN !== undefined) {
    const selected = env.STREAMS_BIN_PATH !== undefined
      ? executable(env.STREAMS_BIN_PATH, 'STREAMS_BIN_PATH')
      : executable(env.STREAMS_RUST_BIN!, 'STREAMS_RUST_BIN');
    if (env.STREAMS_BIN_PATH !== undefined && env.STREAMS_RUST_BIN !== undefined
      && executable(env.STREAMS_RUST_BIN, 'STREAMS_RUST_BIN') !== selected) {
      throw Error('STREAMS_BIN_PATH and STREAMS_RUST_BIN select different executables; use one Rust CLI for reporting and native cards');
    }
    return selected;
  }
  const binary = join(resolve(packageRoot, env.CARGO_TARGET_DIR ?? 'target'), 'debug', 'streams');
  try {
    return executable(binary, 'Checkout Rust CLI');
  } catch (error) {
    throw Error(`${String(error)}. Build this checkout with cargo build --locked --bin streams in ${packageRoot} (using the same CARGO_TARGET_DIR), or set STREAMS_BIN_PATH to an absolute Rust executable`);
  }
}
