import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readFile, realpath, rename, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { resolveStreamsDocument } from '../dist/herdr/streams-documents.js';
const scope = { catalogSource: 'catalog', hostId: 'host', streamId: 'effort', sessionId: 'session-b', nativeSessionId: 'pi-b' };
async function fixture(t) {
  const temporary = await realpath(await mkdtemp(join(tmpdir(), 'streams-document-'))); t.after(() => rm(temporary, { recursive: true, force: true }));
  const a = join(temporary, 'workspace-a'), b = join(temporary, 'workspace-b');
  for (const root of [a, b]) await mkdir(join(root, 'docs'), { recursive: true });
  const reference = { repository: 'project', ref: 'recovery-branch', added_at: 'a1', path: 'docs/design.md', description: 'Shared design' };
  const location = { repository: 'project', path: reference.path, checkout_id: 'b', root: b, file: join(b, reference.path), error: null };
  const snapshot = { scope: { ...scope }, stream: { repositories: { project: { url: 'https://example.com/project' } }, documents: [reference] },
    checkouts: [{ repository: 'project', checkout_id: 'b', root: b }], documentLocations: [location] };
  const target = { scope: { ...scope }, repositoryUrl: 'https://example.com/project', ref: reference.ref, added_at: 'a1', ...location };
  return { a, b, snapshot, target };
}
test('a1 provenance opens viewing B current edits, never A or recorded revision', async t => {
  const { a, b, snapshot, target } = await fixture(t); await writeFile(join(a, target.path), 'A at a5'); await writeFile(join(b, target.path), 'B at b5 plus uncommitted edits');
  const resolved = await resolveStreamsDocument(snapshot, target);
  assert.equal(resolved.root, b); assert.equal(resolved.file, join(b, target.path)); assert.equal(await readFile(resolved.file, 'utf8'), 'B at b5 plus uncommitted edits');
});
test('missing B file stays visible when A has it; no cleanup, fallback or catalog mutation', async t => {
  const { a, b, snapshot, target } = await fixture(t); await writeFile(join(a, target.path), 'Only A'); const before = JSON.stringify(snapshot);
  await assert.rejects(resolveStreamsDocument(snapshot, target), /Document not present in this checkout/); assert.equal(JSON.stringify(snapshot), before);
  assert.equal(await readFile(join(a, target.path), 'utf8'), 'Only A'); await assert.rejects(readFile(join(b, target.path)), { code: 'ENOENT' });
});
test('app binding errors stay visible; never search another root or select defaults', async t => {
  const { snapshot, target } = await fixture(t); snapshot.checkouts = [];
  Object.assign(snapshot.documentLocations[0], { checkout_id: null, root: null, file: null, error: 'No registered checkout for project' });
  Object.assign(target, snapshot.documentLocations[0]); await assert.rejects(resolveStreamsDocument(snapshot, target), /No registered checkout/);
});
test('viewing scope is exact source, host, Stream, Session and native UUID', async t => {
  const { snapshot, target } = await fixture(t);
  for (const field of Object.keys(scope)) { const changed = structuredClone(snapshot); changed.scope[field] = 'different'; await assert.rejects(resolveStreamsDocument(changed, target), /Viewing session or catalog source changed/); }
});
test('changed app binding, checkout ID, canonical file, root and errors invalidate painted target', async t => {
  const { snapshot, target } = await fixture(t);
  for (const [field, value] of [['checkout_id', 'other'], ['root', '/other'], ['file', '/other/file'], ['error', 'Gone']]) {
    const changed = structuredClone(snapshot); changed.documentLocations[0][field] = value;
    await assert.rejects(resolveStreamsDocument(changed, target), /Viewing checkout or app location changed/);
  }
  snapshot.checkouts = []; await assert.rejects(resolveStreamsDocument(snapshot, target), /No checkout binding/);
});
test('reference recovery/provenance freshness and repository URL fail before filesystem resolution', async t => {
  const { snapshot, target } = await fixture(t); const changed = structuredClone(snapshot); changed.stream.repositories.project.url = 'https://example.com/other';
  await assert.rejects(resolveStreamsDocument(changed, target), /Document repository changed/);
  for (const field of ['repository', 'ref', 'path', 'added_at']) { const removed = structuredClone(snapshot); removed.stream.documents[0][field] = 'different'; await assert.rejects(resolveStreamsDocument(removed, target), /Document reference no longer present/); }
});
test('editor key includes source, host, checkout ID and canonical file but not ref/provenance or progress', async t => {
  const { b, snapshot, target } = await fixture(t); await writeFile(join(b, target.path), 'B'); const first = await resolveStreamsDocument(snapshot, target);
  for (const field of ['catalogSource', 'hostId']) {
    const other = structuredClone(snapshot); other.scope[field] = 'other'; const next = await resolveStreamsDocument(other, { ...target, scope: other.scope }); assert.notEqual(first.key, next.key);
  }
  const other = structuredClone(snapshot); other.checkouts[0].checkout_id = other.documentLocations[0].checkout_id = 'replacement-lifetime';
  assert.notEqual(first.key, (await resolveStreamsDocument(other, { ...target, checkout_id: 'replacement-lifetime' })).key);
  snapshot.stream.documents[0].ref = 'new-ref'; snapshot.stream.documents[0].added_at = 'new-commit';
  assert.equal(first.key, (await resolveStreamsDocument(snapshot, { ...target, ref: 'new-ref', added_at: 'new-commit' })).key);
});
test('different checkouts inside the same native workspace resolve independently', async t => {
  const { a, b, snapshot, target } = await fixture(t); for (const root of [a, b]) await writeFile(join(root, target.path), root);
  const first = await resolveStreamsDocument(snapshot, target); const other = structuredClone(snapshot);
  other.scope.sessionId = 'session-a'; other.checkouts[0] = { repository: 'project', checkout_id: 'a', root: a };
  Object.assign(other.documentLocations[0], { checkout_id: 'a', root: a, file: join(a, target.path) });
  const next = await resolveStreamsDocument(other, { ...target, scope: other.scope, ...other.documentLocations[0] }); assert.equal(next.file, join(a, target.path)); assert.notEqual(first.key, next.key);

  const withAlternates = structuredClone(snapshot);
  withAlternates.checkouts.push({ repository: 'project', checkout_id: 'origin-a', root: a });
  withAlternates.documentLocations[0].alternates = [
    { kind: 'local', label: 'local (b)', checkout_id: 'b', root: b, file: join(b, target.path) },
    { kind: 'origin', label: 'origin (a)', checkout_id: 'origin-a', root: a, file: join(a, target.path) },
  ];
  const resolvedAlt = await resolveStreamsDocument(withAlternates, { ...target, checkout_id: 'origin-a', root: a, file: join(a, target.path), error: null });
  assert.equal(resolvedAlt.file, join(a, target.path));
  assert.notEqual(resolvedAlt.key, first.key);
});
test('contained symlinks must match app canonical file; escaping symlinks rejected', async t => {
  const { a, b, snapshot, target } = await fixture(t); await writeFile(join(b, 'docs/actual.md'), 'B'); await symlink('actual.md', join(b, target.path));
  await assert.rejects(resolveStreamsDocument(snapshot, target), /App-resolved canonical file changed/);
  snapshot.documentLocations[0].file = target.file = join(b, 'docs/actual.md'); assert.equal((await resolveStreamsDocument(snapshot, target)).file, target.file);
  await rm(join(b, target.path)); await writeFile(join(a, target.path), 'A'); await symlink(join(a, target.path), join(b, target.path)); await assert.rejects(resolveStreamsDocument(snapshot, target), /resolves outside the checkout/);
});
test('root replaced by symlink is not silently relocated', async t => {
  const { a, b, snapshot, target } = await fixture(t); await rename(b, `${b}-old`); await symlink(a, b);
  await assert.rejects(resolveStreamsDocument(snapshot, target), /Viewing checkout unavailable: Checkout canonical directory changed/);
});
for (const path of ['../design.md', '/design.md', 'docs/../design.md', 'docs\\design.md']) test(`reject invalid document path ${path}`, async t => {
  const { snapshot, target } = await fixture(t); snapshot.stream.documents[0].path = target.path = snapshot.documentLocations[0].path = path;
  await assert.rejects(resolveStreamsDocument(snapshot, target), /Invalid relative document path/);
});
