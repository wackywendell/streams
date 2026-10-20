import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, chmod, rm, realpath } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { StreamsCardSource, createStreamsContextReader } from '../dist/herdr/streams-source.js';

const native = '12345678-1234-4234-8234-123456789abc';
function wire() {
  const session = { id: 'own', host_id: 'host', stream_id: 'shared', harness: 'pi', harness_session_id: native,
    title: 'Own', title_mode: 'manual', status: 'active', state: 'Own state', state_short: 'Own', remaining: 'Own remaining' };
  return { catalog_source: 'catalog', host_id: 'host', stream: { id: 'shared', title: 'Shared', title_mode: 'manual', goal: 'Goal', goal_short: 'Goal',
    repositories: { main: { url: 'https://example.com/main' } },
    documents: [{ repository: 'main', ref: 'topic', commit: 'abc123', path: 'docs/design.md', description: 'Design' }], issues: [], prs: [] },
    native: { frontend: 'herdr', pane_id: 'w:p1', tab_id: 'w:t1', workspace_id: 'w' },
    checkouts: [{ repository: 'main', checkout_id: 'checkout-b', root: '/host/b' }],
    document_locations: [{ repository: 'main', path: 'docs/design.md', checkout_id: 'checkout-b', root: '/host/b', file: '/host/b/docs/design.md', error: null }],
    session, sessions: [session, { ...session, id: 'other', harness_session_id: 'other', state: 'Private other state' }], authority: null };
}
function fixture(options = {}) {
  const value = wire(); let calls = 0;
  const source = new StreamsCardSource({ nativeSessionId: native,
    readContext: async id => { calls++; assert.equal(id, native); return value; }, ...options });
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
  ['coerced status', v => { v.session.status = ['active']; }], ['missing own membership', v => { v.sessions.shift(); }],
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
  assert.throws(() => new StreamsCardSource({ nativeSessionId: 'pane-name', readContext: async () => { throw Error('must not run'); } }), /UUID/);
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
test('nullable compact summaries are retained without native-side inference', async () => {
  const { value, source } = fixture(); value.stream.goal_short = value.session.state_short = null;
  const snapshot = await source.read(); assert.equal(snapshot.stream.goal_short, null); assert.equal(snapshot.session.state_short, null);
});
test('app errors remain renderable; external repo root/default is consumed without native discovery', async () => {
  const { value, source } = fixture();
  Object.assign(value.document_locations[0], { checkout_id: null, root: null, file: null, error: 'No registered checkout for main' });
  value.checkouts = []; assert.equal((await source.read()).documentLocations[0].error, 'No registered checkout for main');
  value.checkouts = [{ repository: 'main', checkout_id: 'external-default', root: '/external/default' }];
  Object.assign(value.document_locations[0], { checkout_id: 'external-default', root: '/external/default', file: '/external/default/docs/design.md', error: null });
  assert.equal((await source.read()).checkouts[0].checkout_id, 'external-default');
});
async function cliFixture(t) {
  const dir = await realpath(await mkdtemp(join(tmpdir(), 'streams-read-only-'))); t.after(() => rm(dir, { recursive: true, force: true }));
  const executable = join(dir, 'streams');
  await writeFile(executable, `#!${process.execPath}\nconst args = process.argv.slice(2);\nif (JSON.stringify(args) !== JSON.stringify(['agent','context','--harness','pi','--session-id','${native}','--attest','--json'])) process.exit(9);\nif (process.cwd() !== process.env.EXPECT_CWD) process.exit(10);\nif (process.env.HERDR_PANE_ID !== 'w:p1' || process.env.HERDR_TAB_ID !== 'w:t1' || process.env.HERDR_WORKSPACE_ID !== 'w') process.exit(11);\nif (process.env.DELAY) setTimeout(() => {}, 5000);\nelse { process.stdout.write(process.env.RESPONSE); process.exit(Number(process.env.EXIT_CODE || 0)); }\n`);
  await chmod(executable, 0o755);
  return extra => createStreamsContextReader({ cwd: dir, owner: wire().native, ...extra,
    env: { PATH: dir, EXPECT_CWD: dir, RESPONSE: JSON.stringify(wire()), HERDR_PANE_ID: 'wrong-helper-pane', ...extra?.env } });
}
test('CLI executes only bounded read-only attested context argv and exact owner env, without shell', async t => {
  const factory = await cliFixture(t); assert.equal((await factory()(native)).session.id, 'own');
  await assert.rejects(factory()('not-a-uuid'), /UUID/); assert.throws(() => factory({ timeoutMs: 0 }), /positive/);
});
for (const [name, options, pattern] of [
  ['malformed', { env: { RESPONSE: '{' } }, /malformed JSON/],
  ['error JSON', { env: { RESPONSE: '{"error":{"message":"no"}}' } }, /Invalid or unbound/],
  ['unbound', { env: { RESPONSE: JSON.stringify({ ...wire(), session: null }) } }, /unbound/],
  ['mismatched owner attestation', { env: { RESPONSE: JSON.stringify({ ...wire(), native: { ...wire().native, pane_id: 'helper' } }) } }, /exact requested owner/],
  ['nonzero exit', { env: { EXIT_CODE: '2' } }, /read failed/], ['output limit', { maxBuffer: 20 }, /read failed/],
  ['timeout', { timeoutMs: 50, env: { DELAY: 'yes' } }, /read failed/],
]) test(`CLI fails visibly for ${name}`, async t => { const factory = await cliFixture(t); await assert.rejects(factory(options)(native), pattern); });
