import { createHash } from 'node:crypto';
import { realpath, stat } from 'node:fs/promises';
import { isDeepStrictEqual } from 'node:util';
import { resolveCheckoutDocument } from '../records.js';
import type { StreamsCardSnapshot } from './streams-source.js';
import type { StreamsDocumentTarget } from './streams-view.js';

export interface ResolvedStreamsDocument {
  root: string;
  file: string;
  key: string;
}

/** Recovery/provenance guards freshness, never the working-tree revision. */
export async function resolveStreamsDocument(snapshot: StreamsCardSnapshot, target: StreamsDocumentTarget): Promise<ResolvedStreamsDocument> {
  if (!isDeepStrictEqual(snapshot.scope, target.scope)) throw new Error('Viewing session or catalog source changed; redraw before opening');
  const repository = snapshot.stream.repositories[target.repository];
  if (!repository || repository.url !== target.repositoryUrl) throw new Error('Document repository changed; redraw before opening');
  if (!snapshot.stream.documents.some(document => document.repository === target.repository && document.ref === target.ref && document.added_at === target.added_at && document.path === target.path)) {
    throw new Error('Document reference no longer present; redraw before opening');
  }
  const location = snapshot.documentLocations.find(l => l.repository === target.repository && l.path === target.path);
  if (!location || ['checkout_id', 'root', 'file', 'error'].some(key => location[key as keyof typeof location] !== target[key as keyof StreamsDocumentTarget])) throw new Error('Viewing checkout or app location changed; redraw before opening');
  if (location.error) throw new Error(location.error);
  const bindings = snapshot.checkouts.filter(binding => binding.repository === target.repository && binding.checkout_id === target.checkout_id);
  if (bindings.length > 1) throw new Error('Ambiguous viewing-session checkout binding');
  const selected = bindings[0];
  if (selected && selected.root !== target.root) throw new Error('Viewing checkout changed; redraw before opening');
  if (!selected) throw new Error(`No checkout binding for ${target.repository} in this viewing session`);
  const root = selected.root;
  let before;
  try {
    if (await realpath(root) !== root) throw new Error('Checkout canonical directory changed');
    before = await stat(root, { bigint: true });
    if (!before.isDirectory()) throw new Error('Checkout must be a directory');
  } catch (error) {
    throw new Error(`Viewing checkout unavailable: ${(error as Error).message}`, { cause: error });
  }
  let file;
  try { file = await resolveCheckoutDocument(root, target.path); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') {
      throw new Error(`Document not present in this checkout: ${target.path}`, { cause: error });
    }
    throw error;
  }
  if (file !== location.file) throw new Error('App-resolved canonical file changed; redraw before opening');
  const after = await stat(root, { bigint: true });
  if (await realpath(root) !== root || before.dev !== after.dev || before.ino !== after.ino) throw new Error('Viewing checkout changed during document resolution');
  // Native reuse also verifies the Herdr workspace and its exact owned editor tuple.
  // Checkout IDs distinguish registry lifetimes, even at a reused canonical path.
  const key = createHash('sha256').update(JSON.stringify([snapshot.scope.catalogSource, snapshot.scope.hostId, selected.checkout_id, file])).digest('hex');
  return { root, file, key };
}
