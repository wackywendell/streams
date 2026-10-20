import { createHash } from 'node:crypto';
import { realpath, stat } from 'node:fs/promises';
import { isAbsolute, normalize, relative, resolve, sep } from 'node:path';
import { isDeepStrictEqual } from 'node:util';
import { resolveStreamsBin } from '../streams-bin.js';
import { runProcess } from './subprocess.js';
import type { StreamsCardSnapshot } from './streams-source.js';
import type { StreamsDocumentTarget } from './streams-view.js';

export interface ResolvedStreamsDocument {
  root: string;
  file: string;
  key: string;
}

function relativeDocumentPath(value: string): string {
  if (typeof value !== 'string' || !value.trim() || value.includes('\0') || value.includes('\\') || isAbsolute(value)) {
    throw new Error('Invalid relative document path');
  }
  const normalized = normalize(value);
  if (normalized === '.' || normalized === '..' || normalized.startsWith('../') || normalized.includes('/../') || normalized !== value.replace(/\/+$/, '')) {
    throw new Error('Invalid relative document path');
  }
  return normalized;
}

export async function resolveCheckoutDocument(directory: string, path: string): Promise<string> {
  if (typeof directory !== 'string' || !directory || directory.includes('\0') || !isAbsolute(directory) || normalize(directory).replace(/\/+$/, '') !== directory.replace(/\/+$/, '')) {
    throw new Error('Checkout directory must be a normalized absolute path');
  }
  const document = relativeDocumentPath(path);
  const root = await realpath(directory);
  if (root !== directory) throw new Error('Checkout canonical directory changed');
  const target = await realpath(resolve(root, document));
  const local = relative(root, target);
  if (local === '..' || local.startsWith(`..${sep}`) || isAbsolute(local)) {
    throw new Error(`Document ${document} resolves outside the checkout ${root}`);
  }
  if (!(await stat(target)).isFile()) throw new Error(`Document ${document} must be a regular file`);
  return target;
}

/** Recovery/provenance guards freshness, never the working-tree revision. */
export async function resolveStreamsDocument(snapshot: StreamsCardSnapshot, target: StreamsDocumentTarget, options: { env?: NodeJS.ProcessEnv; signal?: AbortSignal } = {}): Promise<ResolvedStreamsDocument> {
  if (!isDeepStrictEqual(snapshot.scope, target.scope)) throw new Error('Viewing session or catalog source changed; redraw before opening');
  const repository = snapshot.stream.repositories[target.repository];
  if (!repository || repository.url !== target.repositoryUrl) throw new Error('Document repository changed; redraw before opening');
  if (!snapshot.stream.documents.some(document => document.repository === target.repository && document.ref === target.ref && document.commit === target.commit && document.source_session_id === target.source_session_id && document.path === target.path)) {
    throw new Error('Document reference no longer present; redraw before opening');
  }
  const location = snapshot.documentLocations.find(l => l.repository === target.repository && l.path === target.path);
  const descriptorKeys = ['kind', 'revision', 'checkout_id', 'root', 'file'] as const;
  const matchesPrimary = !!location && location.error === target.error && descriptorKeys.every(key => location[key] === target[key]);
  const matchedAlt = !matchesPrimary && target.error === null ? location?.alternates?.find(a => descriptorKeys.every(key => a[key] === target[key])) : undefined;
  if (!location || (!matchesPrimary && !matchedAlt)) throw new Error('Viewing checkout or app location changed; redraw before opening');
  if (matchesPrimary && location.error) throw new Error(location.error);
  const expectedFile = matchedAlt ? matchedAlt.file : location.file;
  relativeDocumentPath(target.path);
  const bindings = snapshot.checkouts.filter(binding => binding.repository === target.repository && binding.checkout_id === target.checkout_id);
  if (bindings.length > 1) throw new Error('Ambiguous viewing-session checkout binding');
  const selected = bindings[0];
  if (selected && selected.root !== target.root) throw new Error('Viewing checkout changed; redraw before opening');
  if (!selected) throw new Error(`No checkout binding for ${target.repository} in this viewing session`);
  const baseRoot = selected.root;
  let root = baseRoot;
  let before;
  try {
    if (await realpath(root) !== root) throw new Error('Checkout canonical directory changed');
    before = await stat(root, { bigint: true });
    if (!before.isDirectory()) throw new Error('Checkout must be a directory');
  } catch (error) {
    throw new Error(`Viewing checkout unavailable: ${(error as Error).message}`, { cause: error });
  }
  let revisionFile: string | undefined;
  if (target.kind === 'revision') {
    if (target.revision !== (target.commit ?? target.ref)) throw new Error('Document revision changed; redraw before opening');
    const env = options.env ?? process.env;
    const output = await runProcess(resolveStreamsBin(env), ['checkout', 'document', selected.checkout_id, target.path, '--revision', target.revision, '--json'],
      { env, timeoutMs: 60000, maxBuffer: 64 * 1024, signal: options.signal });
    let result;
    try { result = JSON.parse(output); } catch { throw new Error('Invalid document revision response'); }
    if (!result || result.checkout_id !== selected.checkout_id || result.revision !== target.revision
      || typeof result.root !== 'string' || typeof result.file !== 'string') throw new Error('Invalid document revision response');
    root = result.root;
    revisionFile = result.file;
  }
  let file;
  try { file = await resolveCheckoutDocument(root, target.path); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') {
      throw new Error(`Document not present in this checkout: ${target.path}`, { cause: error });
    }
    throw error;
  }
  if (file !== (target.kind === 'revision' ? revisionFile : expectedFile)) throw new Error('App-resolved canonical file changed; redraw before opening');
  const after = await stat(baseRoot, { bigint: true });
  if (await realpath(baseRoot) !== baseRoot || before.dev !== after.dev || before.ino !== after.ino) throw new Error('Viewing checkout changed during document resolution');
  const key = createHash('sha256').update(JSON.stringify([snapshot.scope.catalogSource, snapshot.scope.hostId, selected.checkout_id, file])).digest('hex');
  return { root, file, key };
}
