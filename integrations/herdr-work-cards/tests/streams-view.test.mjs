import test from 'node:test';
import assert from 'node:assert/strict';
import stringWidth from 'string-width';
import { StreamsCardSource } from '../dist/herdr/streams-source.js';
import { renderStreamsCard, StreamsCardView } from '../dist/herdr/streams-view.js';
import { renderDisplayCard } from '../dist/herdr/view.js';

const native = '12345678-1234-4234-8234-123456789abc';
const plain = text => text.replace(/\x1b\[[\d;]*m/g, '').replace(/\x1b\]8;;[^\x1b]*\x1b\\/g, '');
function fixture() {
  const session = { id: 'own', host_id: 'host', stream_id: 'shared', harness: 'pi', harness_session_id: native,
    title: 'Own assignment', title_mode: 'manual', state_short: 'Implement', status: 'active', state: 'Implement | verify', remaining: '界😀é | release' };
  const value = { catalog_source: 'catalog', host_id: 'host', session, sessions: [session], authority: null,
    native: { frontend: 'herdr', pane_id: 'w:p1', tab_id: 'w:t1', workspace_id: 'w' },
    stream: { id: 'shared', title: 'Shared', title_mode: 'manual', goal_short: 'Ship library', goal: 'Ship library | client', repositories: { lib: { url: 'https://example.com/lib' } },
      documents: [{ repository: 'lib', ref: 'topic', path: 'docs/design.md', description: 'Design' }],
      issues: [{ url: 'https://example.com/org/repo/issues/42', description: 'Track work' }],
      prs: [{ url: 'https://example.com/org/repo/pull/51', description: 'Review' }] } };
  let roots = { lib: '/code/library' }, fail = false;
  const source = new StreamsCardSource({ nativeSessionId: native, readContext: async () => {
    if (fail) throw Error('Malformed record');
    value.checkouts = Object.entries(roots).map(([repository, root]) => ({ repository, checkout_id: repository, root }));
    value.document_locations = value.stream.documents.map(({ repository, path }) => ({ repository, path, checkout_id: roots[repository] ? repository : null,
      root: roots[repository] ?? null, file: roots[repository] ? `${roots[repository]}/${path}` : null, error: roots[repository] ? null : 'No checkout binding' }));
    return value;
  } });
  const view = new StreamsCardView(source);
  return { value, view, source, roots: next => { roots = next; }, fail: next => { fail = next; } };
}

test('two exact sessions share goal and references, never progress; baseline renderer is reused byte for byte', async () => {
  const { value, source } = fixture(); const snapshot = await source.read();
  const rendered = renderStreamsCard(snapshot, 100, 12);
  const target = rendered.documents.get('a');
  assert.deepEqual(target, { scope: snapshot.scope, repository: 'lib', repositoryUrl: 'https://example.com/lib', ref: 'topic', path: 'docs/design.md', root: '/code/library', checkout_id: 'lib', file: '/code/library/docs/design.md', error: null });
  assert.equal(rendered.text, renderDisplayCard({ title: snapshot.session.title, status: 'active', goal: snapshot.stream.goal,
    now: snapshot.session.state, remaining: snapshot.session.remaining,
    documents: [{ label: 'docs/design.md', target }], issues: snapshot.stream.issues, prs: snapshot.stream.prs }, 100, 12).text);
  const otherNative = '12345678-1234-4234-8234-123456789abd';
  const other = { ...value.session, id: 'other', harness_session_id: otherNative, title: 'Other', state: 'Other stage', remaining: 'Other todo', status: 'blocked' };
  const otherSource = new StreamsCardSource({ nativeSessionId: otherNative, readContext: async () => ({ ...value, session: other, sessions: [value.session, other] }) });
  const second = plain(renderStreamsCard(await otherSource.read(), 100, 12).text);
  for (const text of [plain(rendered.text), second]) {
    assert.match(text, /Ship library \| client/); assert.match(text, /docs\/design.md/);
    assert.match(text, /Issue #42/); assert.match(text, /PR #51/);
  }
  assert.doesNotMatch(second, /Implement|verify|release/);
  assert.doesNotMatch(plain(rendered.text), /Other/);
  assert.match(rendered.text, /\x1b\]8;;https:\/\/example.com\/org\/repo\/issues\/42/);
});

test('commit provenance is labelled as added, never as a revision pin or recovery branch', async () => {
  const f = fixture(); f.value.stream.documents[0].added_at = 'abcdef0123456789';
  const rendered = renderStreamsCard(await f.source.read(), 100, 10);
  assert.match(plain(rendered.text), /docs\/design.md \(added abcdef01\)/);
  assert.doesNotMatch(plain(rendered.text), /@ topic|@ abcdef/);
  assert.equal(rendered.documents.get('a').added_at, 'abcdef0123456789');
});
test('missing bindings keep actionable references and their captured recovery ref and URL', async () => {
  const f = fixture(); f.roots({}); await f.view.refresh();
  assert.match(plain(f.view.render(100, 10)), /\[a\] docs\/design.md/);
  const action = await f.view.key('a'); assert.equal(action.kind, 'open-document');
  assert.equal(action.target.root, null); assert.equal(action.target.ref, 'topic');
  assert.equal(action.target.repositoryUrl, 'https://example.com/lib');
  assert.equal(action.snapshot, f.view.current);
});

test('multiroot labels reuse distinguishing ancestors; same-root collisions and unbound aliases preserve identity', async () => {
  const f = fixture();
  f.value.stream.repositories = Object.fromEntries(['lib', 'client', 'same', 'unbound'].map(alias => [alias, { url: `https://example.com/${alias}` }]));
  f.value.stream.documents = Object.keys(f.value.stream.repositories).map(repository => ({ repository, ref: 'topic', path: 'docs/design.md', description: '' }));
  f.roots({ lib: '/backend/common', client: '/frontend/common', same: '/backend/common' });
  const snapshot = await f.source.read(); const rendered = renderStreamsCard(snapshot, 180, 12); const text = plain(rendered.text);
  for (const label of ['lib:backend/common/docs/design.md', 'frontend/common/docs/design.md', 'same:backend/common/docs/design.md', 'unbound/docs/design.md']) assert.ok(text.includes(label), label);
  assert.deepEqual([...rendered.documents.values()].map(t => [t.repository, t.root]), [['lib', '/backend/common'], ['client', '/frontend/common'], ['same', '/backend/common'], ['unbound', null]]);
});

for (const [name, change] of [
  ['path', f => { f.value.stream.documents[0].path = 'replaced.md'; }],
  ['ref', f => { f.value.stream.documents[0].ref = 'different-branch'; }],
  ['provenance', f => { f.value.stream.documents[0].added_at = 'new-commit'; }],
  ['repository URL', f => { f.value.stream.repositories.lib.url = 'https://example.com/new'; }],
  ['root', f => { f.roots({ lib: '/relocated' }); }],
  ['binding removed', f => { f.roots({}); }],
  ['reorder', f => { f.value.stream.documents.unshift({ repository: 'lib', ref: 'topic', path: 'new.md', description: '' }); }],
  ['removed reference', f => { f.value.stream.documents = []; }],
]) test(`painted shortcut rejects changed ${name} and stays disabled until redraw`, async () => {
  const f = fixture(); await f.view.refresh(); f.view.render(100, 12); change(f);
  assert.deepEqual(await f.view.key('a'), { kind: 'none' }); assert.match(f.view.notice.text, /changed.*redraw/);
  assert.deepEqual(await f.view.key('a'), { kind: 'none' });
  if (f.value.stream.documents.length) {
    f.view.render(100, 12); assert.equal((await f.view.key('a')).kind, 'open-document');
  }
});

test('source identity switching fails visibly and clears all painted actions', async () => {
  const f = fixture(); await f.view.refresh(); f.view.render(100, 12);
  f.value.catalog_source = 'new-source';
  assert.deepEqual(await f.view.key('a'), { kind: 'none' }); assert.equal(f.view.current, undefined);
  assert.equal(f.view.notice.source, 'card'); assert.match(f.view.notice.text, /scope changed/);
});

test('background refresh never replaces captured painted root identity', async () => {
  const f = fixture(); await f.view.refresh(); f.view.render(100, 12);
  f.roots({ lib: '/relocated' }); await f.view.refresh();
  assert.deepEqual(await f.view.key('a'), { kind: 'none' });
});

function shortGrid(f) {
  f.value.stream.goal = null; f.value.stream.issues = []; f.value.stream.prs = [];
  Object.assign(f.value.session, { title: 'Own', state: '', remaining: '' });
  f.value.stream.documents = ['a.md', 'b.md', 'c.md'].map(path => ({ repository: 'lib', ref: 'x', path, description: '' }));
}
test('painted viewport and notice reserve exact visible keys; reflow rejects hidden later-column target', async () => {
  const f = fixture(); shortGrid(f); await f.view.refresh();
  assert.match(plain(f.view.render(100, 3)), /\[c\]/);
  assert.equal((await f.view.key('c')).kind, 'open-document');
  f.view.setNotice('projection', 'Notice'); f.view.render(20, 2);
  assert.deepEqual(await f.view.key('c'), { kind: 'none' }); assert.equal(f.view.notice.source, 'projection');
  f.view.clearNotice(); f.view.render(100, 3);
  f.value.stream.documents[0].path = 'a-very-long-reference-'.repeat(10) + '.md';
  assert.deepEqual(await f.view.key('c'), { kind: 'none' });
});

test('key revalidation uses the painted notice, not an unpainted reservation change', async () => {
  const f = fixture(); shortGrid(f); await f.view.refresh();
  f.view.setNotice('projection', 'Painted');
  f.view.render(100, 3);
  f.view.clearNotice();
  assert.equal((await f.view.key('c')).kind, 'open-document');
});

test('resize during a pending key read fails closed', async () => {
  const f = fixture(); const initial = await f.source.read();
  let finish;
  const view = new StreamsCardView({ read: async () => initial });
  await view.refresh(); view.render(100, 12);
  view.source.read = () => new Promise(resolve => { finish = resolve; });
  const pending = view.key('a'); view.render(5, 1); finish(initial);
  assert.deepEqual(await pending, { kind: 'none' }); assert.match(view.notice.text, /redraw/);
});

test('fresh progress-only changes are accepted; q/Ctrl-C exit display only, numeric links are inert', async () => {
  const f = fixture(); await f.view.refresh(); f.view.render(100, 12);
  f.value.session.state = 'Fresh progress'; f.value.session.title = 'Fresh title';
  const action = await f.view.key('a'); assert.equal(action.kind, 'open-document'); assert.equal(action.snapshot.session.state, 'Fresh progress');
  assert.deepEqual(await f.view.key('1'), { kind: 'none' });
  f.fail(true);
  assert.deepEqual(await f.view.key('q'), { kind: 'exit' }); assert.deepEqual(await f.view.key('\x03'), { kind: 'exit' });
  assert.equal(f.view.notice, undefined);
});

test('read failures disable stale actions; repaired reads clear card notices only', async () => {
  const f = fixture(); await f.view.refresh(); f.view.render(100, 12); f.fail(true);
  assert.deepEqual(await f.view.key('a'), { kind: 'none' }); assert.equal(f.view.current, undefined);
  assert.equal(f.view.notice.source, 'card'); assert.match(plain(f.view.render(100, 5)), /unavailable.*Malformed/);
  f.fail(false); await f.view.refresh(); assert.equal(f.view.notice, undefined);
  for (const source of ['document', 'projection']) {
    f.view.setNotice(source, 'Keep this'); await f.view.refresh(); assert.equal(f.view.notice.source, source);
    f.view.clearNotice('card'); assert.equal(f.view.notice.source, source);
    f.view.clearNotice(source); assert.equal(f.view.notice, undefined);
  }
});

test('Unicode, visible letters, footer and terminal links remain bounded by the imported renderer', async () => {
  const f = fixture(); f.value.stream.documents = Array.from({ length: 28 }, (_, i) => ({ repository: 'lib', ref: '界😀', path: `docs/é-${i}.md`, description: '' }));
  const snapshot = await f.source.read();
  for (let rows = 0; rows < 9; rows++) for (let columns = 0; columns < 32; columns++) for (const notice of [undefined, 'Notice 界😀é']) {
    const rendered = renderStreamsCard(snapshot, columns, rows, notice); const text = plain(rendered.text);
    assert.ok(!text || text.split('\n').length <= rows); assert.ok(text.split('\n').every(line => stringWidth(line) <= columns));
    for (const [key, target] of rendered.documents) {
      assert.ok(text.includes(`[${key}]`)); assert.equal(target.ref, '界😀'); assert.equal(target.root, '/code/library');
    }
  }
  const keys = renderStreamsCard(snapshot, 200, 20).documents;
  assert.ok(keys.has('j')); assert.ok(keys.has('k')); assert.ok(!keys.has('q'));
});
