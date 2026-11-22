import { createHash } from 'node:crypto';
import { accessSync, constants, existsSync, readdirSync, readFileSync, realpathSync, statSync } from 'node:fs';
import { delimiter, isAbsolute, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const packageRoot = fileURLToPath(new URL('../..', import.meta.url));
export const checkoutRoot = resolve(packageRoot);

/**
 * Digest of this checkout's Rust sources, exactly as build.rs embeds it in the binary;
 * null for a packaged extension, which ships no sources to compare.
 */
export function checkoutSource(): string | null {
  if (!existsSync(join(packageRoot, 'build.rs'))) return null;
  const paths = ['Cargo.toml', 'Cargo.lock', 'build.rs'];
  const walk = (relative: string) => {
    for (const entry of readdirSync(join(packageRoot, relative), {withFileTypes: true})) {
      const path = `${relative}/${entry.name}`;
      if (entry.isDirectory()) walk(path); else paths.push(path);
    }
  };
  walk('src');
  paths.sort();
  const hash = createHash('sha256');
  for (const path of paths) {
    const bytes = readFileSync(join(packageRoot, path));
    hash.update(`${path}\0${bytes.length}\0`);hash.update(bytes);
  }
  return hash.digest('hex');
}

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

/** The first regular executable named `streams` in an absolute `PATH` directory. */
function installedStreamsBin(path: string | undefined): string | null {
  for (const directory of (path ?? '').split(delimiter)) {
    try {
      return executable(join(directory, 'streams'), 'PATH');
    } catch {}
  }
  return null;
}

/**
 * Pin reporting and native subprocesses to one Rust CLI: `STREAMS_BIN_PATH`, then `streams`
 * on `PATH`, then this checkout's debug build. A checkout refuses a child built from other
 * sources, so a stale installed binary fails loudly instead of running.
 */
export function resolveStreamsBin(env: NodeJS.ProcessEnv = process.env): string {
  if (env.STREAMS_BIN_PATH !== undefined) {
    return executable(env.STREAMS_BIN_PATH, 'STREAMS_BIN_PATH');
  }
  const installed = installedStreamsBin(env.PATH);
  if (installed !== null) return installed;
  const binary = join(resolve(packageRoot, env.CARGO_TARGET_DIR ?? 'target'), 'debug', 'streams');
  try {
    return executable(binary, 'Checkout Rust CLI');
  } catch (error) {
    throw Error(`${String(error)}. Install streams on PATH with just install-cli, build this checkout with cargo build --locked --bin streams in ${packageRoot} (using the same CARGO_TARGET_DIR), or set STREAMS_BIN_PATH to an absolute Rust executable`);
  }
}
