import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, chmod, rm, realpath } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { StreamsCardSource, createStreamsSnapshotReader, parseStreamsCardContext } from '../dist/herdr/streams-source.js';
import { isStream, isSession, parseLocalContext } from '../dist/streams-context.js';

const native = '12345678-1234-4234-8234-123456789abc';
function wire() {
  const session = { id: 'own', host_id: 'host', stream_id: 'shared', harness: 'pi', harness_session_id: native,
    title: 'Own', title_mode: 'manual', status: 'active', state: 'Own state', state_short: 'Own', remaining: 'Own remaining', checkout: null };
  return { catalog_source: 'catalog', host_id: 'host', stream: { id: 'shared', title: 'Shared', title_mode: 'manual', goal: 'Goal', goal_short: 'Goal', goal_mode: 'manual', status: 'active', archived: false, owner: { host_id: 'host' },
    repositories: { main: { url: 'https://example.com/main' } },
    documents: [{ repository: 'main', ref: 'topic', commit: 'abc123', path: 'docs/design.md', description: 'Design' }], issues: [], prs: [] },
    native: { frontend: 'herdr', pane_id: 'w:p1', tab_id: 'w:t1', workspace_id: 'w' },
    checkouts: [{ repository: 'main', checkout_id: 'checkout-b', root: '/host/b' }],
    document_locations: [{ kind: 'local', repository: 'main', path: 'docs/design.md', checkout_id: 'checkout-b', root: '/host/b', file: '/host/b/docs/design.md', error: null }],
    session, sessions: [session, { ...session, id: 'other', harness_session_id: 'other', state: 'Private other state' }], authority: null };
}
function fixture(options = {}) {
  const value = wire(); let calls = 0;
  const source = new StreamsCardSource({ nativeSessionId: native,
    readSnapshot: async id => { calls++; assert.equal(id, native); return parseStreamsCardContext(value, id); }, ...options });
  return { value, source, calls: () => calls };
}
test('projects exact own fields and app checkout resolution from one read; never retains authority/other progress', async () => {
  const { value, source, calls } = fixture(); value.authority = { host_id: 'host', token: 'not native authorization' };
  const snapshot = await source.read();
  assert.deepEqual(snapshot.scope, { catalogSource: 'catalog', hostId: 'host', streamId: 'shared', sessionId: 'own', nativeSessionId: native });
  assert.deepEqual(snapshot.checkouts, value.checkouts); assert.deepEqual(snapshot.documentLocations, value.document_locations);
  assert.doesNotMatch(JSON.stringify(snapshot), /Private other state|authorization/);
  assert.equal(snapshot.stream.title_mode, 'manual'); assert.equal(snapshot.session.state_short, 'Own');
  snapshot.checkouts[0].root = 'mutated'; snapshot.stream.documents[0].path = 'mutated';
  assert.equal((await source.read()).stream.documents[0].path, 'docs/design.md'); assert.equal(calls(), 2);
});
for (const [name, change] of [
  ['missing stream', v => { v.stream = null; }], ['missing session', v => { delete v.session; }],
  ['foreign host', v => { v.session.host_id = 'remote'; }], ['foreign harness', v => { v.session.harness = 'codex'; }],
  ['foreign native', v => { v.session.harness_session_id = '12345678-1234-4234-8234-123456789abd'; }],
  ['foreign stream', v => { v.session.stream_id = 'elsewhere'; }], ['invalid status', v => { v.session.status = 'unknown'; }],
  ['coerced status', v => { v.session.status = ['active']; }],
  ['missing checkout', v => { delete v.session.checkout; }],
  ['invalid checkout', v => { v.session.checkout = { repository: 'main', ref: 'topic', directory_within_checkout: null }; }],
  ['invalid session mode', v => { v.session.title_mode = 'unknown'; }],
  ['coerced session mode', v => { v.session.title_mode = ['manual']; }],
  ['invalid stream mode', v => { v.stream.title_mode = 'unknown'; }],
  ['invalid goal mode', v => { v.stream.goal_mode = 'unknown'; }],
  ['missing stream status', v => { delete v.stream.status; }],
  ['invalid stream status', v => { v.stream.status = 'unknown'; }],
  ['coerced stream status', v => { v.stream.status = ['active']; }],
  ['missing archived', v => { delete v.stream.archived; }],
  ['invalid archived', v => { v.stream.archived = 'false'; }],
  ['missing owner', v => { delete v.stream.owner; }],
  ['empty owner identity', v => { v.stream.owner.host_id = ' '; }],
  ['empty session identity', v => { v.session.id = ' '; }],
  ['empty catalog identity', v => { v.catalog_source = ' '; }],
  ['empty host identity', v => { v.host_id = ' '; }],
  ['nul identity', v => { v.session.id = 'own\0'; }],
  ['empty repository alias', v => { v.stream.repositories[' '] = { url: 'https://example.com/other' }; }],
  ['empty repository URL', v => { v.stream.repositories.main.url = ''; }],
  ['unknown document alias', v => { v.stream.documents[0].repository = 'unknown'; }],
  ['empty document ref', v => { v.stream.documents[0].ref = ' '; }],
  ['empty document path', v => { v.stream.documents[0].path = ''; }],
  ['empty commit', v => { v.stream.documents[0].commit = ''; }],
  ['blank source session', v => { v.stream.documents[0].source_session_id = ' '; }],
  ['nonstring commit', v => { v.stream.documents[0].commit = 123; }],
  ['nonstring source session', v => { v.stream.documents[0].source_session_id = {}; }],
  ['empty authority token', v => { v.authority = { host_id: 'host', token: '' }; }],
  ['missing own membership', v => { v.sessions.shift(); }],
  ['ambiguous native membership', v => { v.sessions.push({ ...v.session, id: 'duplicate' }); }],
  ['missing goal', v => { delete v.stream.goal; }], ['missing title', v => { delete v.stream.title; }],
  ['missing compact goal', v => { delete v.stream.goal_short; }], ['missing repository', v => { v.stream.repositories = {}; }],
  ['duplicate documents', v => { v.stream.documents.push({ ...v.stream.documents[0], ref: 'other' }); }],
  ['malformed document', v => { delete v.stream.documents[0].ref; }], ['malformed sessions', v => { v.sessions = {}; }],
  ['error-shaped', v => { v.error = { message: 'failure' }; }], ['missing source', v => { delete v.catalog_source; }],
  ['missing attestation', v => { delete v.native; }], ['wrong frontend', v => { v.native.frontend = 'terminal'; }],
  ['relative root', v => { v.checkouts[0].root = 'checkout'; }],
  ['noncanonical root', v => { v.checkouts[0].root = '/a/../checkout'; }],
  ['duplicate checkout', v => { v.checkouts.push(v.checkouts[0]); }],
  ['unknown alias', v => { v.checkouts[0].repository = 'unknown'; }],
  ['missing locations', v => { delete v.document_locations; }],
  ['missing document location', v => { v.document_locations = []; }],
  ['duplicate location', v => { v.document_locations.push(v.document_locations[0]); }],
  ['foreign checkout ID', v => { v.document_locations[0].checkout_id = 'other'; }],
  ['success without file', v => { v.document_locations[0].file = null; }],
  ['error with file', v => { v.document_locations[0].error = 'Missing'; }],
  ['error with half checkout identity', v => { Object.assign(v.document_locations[0], { checkout_id: null, file: null, error: 'Missing' }); }],
  ['error with half checkout root', v => { Object.assign(v.document_locations[0], { root: null, file: null, error: 'Missing' }); }],
  ['error with foreign checkout', v => { Object.assign(v.document_locations[0], { checkout_id: 'foreign', file: null, error: 'Missing' }); }],
  ['error with foreign checkout root', v => { Object.assign(v.document_locations[0], { root: '/foreign', file: null, error: 'Missing' }); }],
]) test(`rejects ${name}`, async () => { const { value, source } = fixture(); change(value); await assert.rejects(source.read()); });
test('native identity must be a UUID before invoking any reader', () => {
  assert.throws(() => new StreamsCardSource({ nativeSessionId: 'pane-name', readSnapshot: async () => { throw Error('must not run'); } }), /UUID/);
});
for (const [name, change] of [
  ['catalog', v => { v.catalog_source = 'new'; }], ['host', v => { v.host_id = v.session.host_id = 'new'; }],
  ['stream', v => { v.stream.id = v.session.stream_id = 'new'; }], ['session', v => { v.session.id = 'new'; }],
]) test(`lifetime fence refuses ${name} switching, including return to original scope`, async () => {
  const { value, source } = fixture(); const original = structuredClone(value);
  await source.read(); change(value); await assert.rejects(source.read(), /scope changed/);
  Object.assign(value, original); await assert.rejects(source.read(), /scope changed/);
});
test('launcher expected scope fences the very first read', async () => {
  const expectedScope = { catalogSource: 'old', hostId: 'host', streamId: 'shared', sessionId: 'own', nativeSessionId: native };
  const { source } = fixture({ expectedScope }); expectedScope.catalogSource = 'catalog';
  await assert.rejects(source.read(), /scope changed/);
});
test('shared context accepts unenrolled contexts and general non-UUID harness identities', () => {
  const value = wire(); value.session.harness_session_id = 'native';
  value.session.checkout = { repository: 'main', ref: 'topic', directory_within_checkout: '' };
  assert.equal(isStream(value.stream), true); assert.equal(isSession(value.session), true);
  assert.equal(parseLocalContext(value), value);
  assert.equal(isStream(null), false); assert.equal(isSession([]), false);
  value.stream = value.session = null; value.sessions = [];
  assert.equal(parseLocalContext(value), value);
  assert.throws(() => parseLocalContext({ ...value, error: null }), /Invalid local catalog context/);
});
test('document provenance accepts omitted, null and nonempty strings without changing the wire', async () => {
  const { value, source } = fixture(), local = structuredClone(value.document_locations[0]);
  for (const provenance of [undefined, null, 'saved-origin']) {
    value.document_locations[0] = structuredClone(local);
    const doc = value.stream.documents[0];
    if (provenance === undefined) { delete doc.commit; delete doc.source_session_id; }
    else { doc.commit = doc.source_session_id = provenance; }
    assert.deepEqual((await source.read()).stream.documents[0], doc);
    assert.equal(parseLocalContext(value), value);
    value.document_locations[0] = { ...value.document_locations[0], kind: 'revision', file: null, revision: doc.commit ?? doc.ref };
    assert.equal((await source.read()).documentLocations[0].revision, doc.commit ?? doc.ref);
  }
});
test('trusted snapshot reader returns its projection directly without a second raw-wire parse', async () => {
  const snapshot = parseStreamsCardContext(wire(), native);
  const source = new StreamsCardSource({ nativeSessionId: native, readSnapshot: async () => snapshot });
  assert.equal(await source.read(), snapshot);
});
test('requested native identity is fenced even on a trusted reader first read', async () => {
  let snapshot = parseStreamsCardContext(wire(), native), calls = 0;
  snapshot.scope.nativeSessionId = '12345678-1234-4234-8234-123456789abd';
  const source = new StreamsCardSource({ nativeSessionId: native, readSnapshot: async () => { calls++; return snapshot; } });
  await assert.rejects(source.read(), /scope changed/);
  snapshot = parseStreamsCardContext(wire(), native);
  await assert.rejects(source.read(), /scope changed/); assert.equal(calls, 1);
});
test('source cancellation is checked before dispatch and after a trusted reader resolves', async () => {
  const controller = new AbortController(); let calls = 0;
  const source = new StreamsCardSource({ nativeSessionId: native, readSnapshot: async (id, signal) => {
    calls++; assert.equal(id, native); assert.equal(signal, controller.signal);
    controller.abort(); return parseStreamsCardContext(wire(), id);
  } });
  await assert.rejects(source.read(controller.signal), /abort/i); assert.equal(calls, 1);
  await assert.rejects(source.read(controller.signal), /abort/i); assert.equal(calls, 1);
});
test('an in-flight original-scope read cannot escape a concurrent permanent scope switch', async () => {
  const snapshot = parseStreamsCardContext(wire(), native); let release;
  const source = new StreamsCardSource({ nativeSessionId: native, expectedScope: snapshot.scope,
    readSnapshot: async () => new Promise(resolve => { release = resolve; }) });
  const original = source.read(), resolveOriginal = release;
  const switched = source.read();
  release({ ...snapshot, scope: { ...snapshot.scope, catalogSource: 'other' } });
  await assert.rejects(switched, /scope changed/);
  resolveOriginal(snapshot); await assert.rejects(original, /scope changed/);
});
test('nullable compact summaries are retained without native-side inference', async () => {
  const { value, source } = fixture(); value.stream.goal_short = value.session.state_short = null;
  const snapshot = await source.read(); assert.equal(snapshot.stream.goal_short, null); assert.equal(snapshot.session.state_short, null);
});
test('app errors remain renderable; external repo root/default is consumed without native discovery', async () => {
  const { value, source } = fixture();
  Object.assign(value.document_locations[0], { kind: null, checkout_id: null, root: null, file: null, error: 'No registered checkout for main' });
  value.checkouts = []; assert.equal((await source.read()).documentLocations[0].error, 'No registered checkout for main');
  value.checkouts = [{ repository: 'main', checkout_id: 'external-default', root: '/external/default' }];
  Object.assign(value.document_locations[0], { kind: 'local', checkout_id: 'external-default', root: '/external/default', file: '/external/default/docs/design.md', error: null });
  assert.equal((await source.read()).checkouts[0].checkout_id, 'external-default');
});
test('revision primary and alternates retain real checkout metadata without extracted files', async () => {
  const { value, source } = fixture();
  const revision = { kind: 'revision', checkout_id: 'checkout-b', root: '/host/b', file: null, revision: 'abc123' };
  value.document_locations[0].alternates = [{ ...revision, label: 'revision abc123' }];
  assert.deepEqual((await source.read()).documentLocations[0].alternates[0], { ...revision, label: 'revision abc123' });
  Object.assign(value.document_locations[0], revision);
  assert.equal((await source.read()).documentLocations[0].kind, 'revision');
  for (const change of [{ revision: 'stale' }, { file: '/host/b/extracted.md' }, { kind: 'local' }, { checkout_id: 'checkout-b@abc123' }]) {
    const original = structuredClone(value.document_locations[0]);
    Object.assign(value.document_locations[0], change); await assert.rejects(source.read());
    value.document_locations[0] = original;
  }
  delete value.stream.documents[0].commit;
  value.document_locations[0].revision = value.document_locations[0].alternates[0].revision = 'topic';
  assert.equal((await source.read()).documentLocations[0].revision, 'topic');
});
for (const [name, change] of [
  ['wrong revision', a => { a.revision = 'stale'; }],
  ['extracted revision file', a => { a.file = '/host/b/extracted.md'; }],
  ['unbound checkout', a => { a.checkout_id = 'unknown'; }],
  ['foreign root', a => { a.root = '/elsewhere'; }],
  ['empty label', a => { a.label = ''; }],
  ['local with revision', a => { a.kind = 'local'; a.file = '/host/b/docs/design.md'; }],
]) test(`rejects alternate with ${name}`, () => {
  const value = wire(), alternate = { kind: 'revision', checkout_id: 'checkout-b', root: '/host/b', file: null, revision: 'abc123', label: 'Saved revision' };
  value.document_locations[0].alternates = [alternate]; change(alternate);
  assert.throws(() => parseStreamsCardContext(value, native), /alternate/);
});
async function cliFixture(t) {
  const dir = await realpath(await mkdtemp(join(tmpdir(), 'streams-read-only-'))); t.after(() => rm(dir, { recursive: true, force: true }));
  const executable = join(dir, 'streams');
  await writeFile(executable, `#!${process.execPath}\nconst args = process.argv.slice(2);\nif (JSON.stringify(args) !== JSON.stringify(['agent','context','--harness','pi','--session-id','${native}','--attest','--json', ...(process.env.EXPECT_INVENTORY ? ['--native-inventory'] : [])])) process.exit(9);\nif (process.cwd() !== process.env.EXPECT_CWD) process.exit(10);\nif (process.env.HERDR_PANE_ID !== 'w:p1' || process.env.HERDR_TAB_ID !== 'w:t1' || process.env.HERDR_WORKSPACE_ID !== 'w') process.exit(11);\nif (process.env.DELAY) setTimeout(() => {}, 5000);\nelse { process.stdout.write(process.env.RESPONSE); process.exit(Number(process.env.EXIT_CODE || 0)); }\n`);
  await chmod(executable, 0o755);
  return extra => createStreamsSnapshotReader({ cwd: dir, owner: wire().native, ...extra,
    env: { PATH: dir, STREAMS_BIN_PATH: executable, EXPECT_CWD: dir, RESPONSE: JSON.stringify(wire()), HERDR_PANE_ID: 'wrong-helper-pane', ...extra?.env } });
}
test('CLI executes only bounded read-only attested context argv and exact owner env, without shell', async t => {
  const factory = await cliFixture(t), snapshot = await factory()(native);
  assert.deepEqual(snapshot, parseStreamsCardContext(wire(), native));
  assert.equal(snapshot.session.id, 'own'); assert.equal(snapshot.scope.nativeSessionId, native);
  assert.equal(Object.hasOwn(snapshot, 'sessions'), false); assert.equal(Object.hasOwn(snapshot, 'authority'), false);
  await assert.rejects(factory()('not-a-uuid'), /UUID/); assert.throws(() => factory({ timeoutMs: 0 }), /positive/);
});
test('handover requests bounded read-only identity inventory without retaining other Session progress', async t => {
  const value = wire();
  value.sessions.push({ ...value.session, id: 'remote', host_id: 'remote', state: 'Remote secret' });
  value.native_inventory = { socket: '/herdr.sock', panes: [{ pane_id: 'w:p1', terminal_id: 'term1', workspace_id: 'w', tab_id: 'w:t1', native_session_id: native }] };
  const factory = await cliFixture(t), reader = factory({ env: { EXPECT_INVENTORY: '1', RESPONSE: JSON.stringify(value) } });
  const source = new StreamsCardSource({ nativeSessionId: native, readSnapshot: reader });
  const saved = await source.read(undefined, true);
  assert.equal(saved.handover.socket, '/herdr.sock');
  assert.deepEqual(saved.handover.inventory, value.native_inventory.panes);
  assert.deepEqual(saved.handover.scopes.map(s => s.sessionId), ['own', 'other']);
  assert.doesNotMatch(JSON.stringify(saved.handover), /Private other state|Remote secret|state|title|token/);
  for (const invalid of [null, [], {}, { socket: 'relative', panes: [] }, { socket: '/herdr.sock', panes: [{}] },
    { socket: '/herdr.sock', panes: [{ ...value.native_inventory.panes[0], native_session_id: 1 }] }]) {
    assert.throws(() => parseStreamsCardContext({ ...value, native_inventory: invalid }, native), /inventory/);
  }
});
test('CLI reader honours cancellation before dispatch and during a bounded subprocess', async t => {
  const factory = await cliFixture(t), controller = new AbortController();
  controller.abort(); await assert.rejects(factory()(native, controller.signal), /abort/i);
  const running = new AbortController(), read = factory({ env: { DELAY: 'yes' } })(native, running.signal);
  running.abort(); await assert.rejects(read, /abort/i);
});
for (const [name, options, pattern] of [
  ['malformed', { env: { RESPONSE: '{' } }, /malformed JSON/],
  ['error JSON', { env: { RESPONSE: '{"error":{"message":"no"}}' } }, /Invalid local catalog context/],
  ['unbound', { env: { RESPONSE: JSON.stringify({ ...wire(), session: null }) } }, /unbound/],
  ['mismatched owner attestation', { env: { RESPONSE: JSON.stringify({ ...wire(), native: { ...wire().native, pane_id: 'helper' } }) } }, /exact requested owner/],
  ['nonzero exit', { env: { EXIT_CODE: '2' } }, /read failed/], ['output limit', { maxBuffer: 20 }, /read failed/],
  ['timeout', { timeoutMs: 50, env: { DELAY: 'yes' } }, /read failed/],
]) test(`CLI fails visibly for ${name}`, async t => { const factory = await cliFixture(t); await assert.rejects(factory(options)(native), pattern); });
