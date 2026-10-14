import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, mkdir, rm, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createCard } from '../dist/card.js';
import { renderCard, plain, KeyInput, CardView } from '../dist/herdr/view.js';

const card = { title: 'Title', status: 'active', state: 'State', cwd: '/repo', documents: [], issues: [], prs: [] };
const snapshot = c => ({ slug: 'title', path: '/cards/title.yaml', revision: 'r', card: c });
// Strip only renderer-owned SGR and OSC8, not the security sanitizer.
const unstyled = text => text.replace(/\x1b\[[\d;]*m/g, '').replace(/\x1b\]8;;[^\x1b]*\x1b\\/g, '');
async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'work-cards-view-')); t.after(() => rm(root, { recursive: true, force: true }));
  const repo = join(root, 'repo'); await mkdir(repo); const cards = join(root, 'cards');
  const saved = await createCard({ ...card, cwd: repo, documents: [{ path: 'notes.md', description: 'Notes' }] }, cards);
  await writeFile(join(repo, 'notes.md'), 'hello\nworld\n');
  return { root, repo, cards, saved };
}
test('compact rendering bounds rows/columns and exposes shortcuts only for displayed documents', () => {
  const c = { ...card, documents: Array.from({ length: 5 }, (_, i) => ({ path: `${i}.md`, description: `Document ${i}` })),
    issues: [{ url: 'https://example.com/issues/1', description: 'An issue' }] };
  const rendered = renderCard(snapshot(c), 25, 4);
  assert.equal(rendered.text.split('\n').length, 4); assert.deepEqual([...rendered.documents.values()], [{ cwd: '/repo', path: '0.md' }]);
  assert.match(rendered.text, /more/);
  const tiny = renderCard(snapshot(c), 10, 1); assert.equal(tiny.documents.size, 0);
});
test('document labels use alphabetic keys and full relative paths, never descriptions', () => {
  const c = { ...card, documents: [
    { path: 'docs/design.md', description: 'Planned editor-first document actions and deferred research' },
    { path: 'adrs/design.md', description: '' },
  ] };
  const rendered = renderCard(snapshot(c), 100, 10);
  assert.deepEqual(unstyled(rendered.text).split('\n').slice(1), ['State', '[a] docs/design.md', '[b] adrs/design.md', 'q close']);
  assert.deepEqual([...rendered.documents], [
    ['a', { cwd: '/repo', path: 'docs/design.md' }],
    ['b', { cwd: '/repo', path: 'adrs/design.md' }],
  ]);
});
test('issue and PR labels share consecutive numbers independently of document letters', () => {
  const c = { ...card, documents: [{ path: 'docs/design.md', description: 'Hidden description' }],
    issues: [{ url: 'https://example.com/issues/1', description: 'An issue' },
      { url: 'https://example.com/issues/2', description: '' }],
    prs: [{ url: 'https://example.com/pulls/3', description: 'A PR' },
      { url: 'https://example.com/pulls/4', description: '' }] };
  const rendered = renderCard(snapshot(c), 100, 10);
  assert.deepEqual(unstyled(rendered.text).split('\n').slice(1), ['State', '[a] docs/design.md',
    '[1] Issue: An issue', '[2] Issue: https://example.com/issues/2',
    '[3] PR: A PR', '[4] PR: https://example.com/pulls/4', 'q close']);
  for (const link of [...c.issues, ...c.prs]) assert.ok(rendered.text.includes(`\x1b]8;;${link.url}\x1b\\`));
  assert.deepEqual([...rendered.documents], [['a', { cwd: '/repo', path: 'docs/design.md' }]]);
});
test('PR-only labels start at one', () => {
  const c = { ...card, prs: [{ url: 'https://example.com/pulls/1', description: 'A PR' }] };
  assert.deepEqual(unstyled(renderCard(snapshot(c), 80, 3).text).split('\n').slice(1), ['State', '[1] PR: A PR']);
});
test('numeric labels continue beyond nine across issues and PRs without document actions', () => {
  const c = { ...card, issues: Array.from({ length: 9 }, (_, i) => ({ url: `https://example.com/issues/${i + 1}`, description: `Issue ${i + 1}` })),
    prs: [{ url: 'https://example.com/pulls/10', description: 'Ten' }, { url: 'https://example.com/pulls/11', description: 'Eleven' }] };
  const rendered = renderCard(snapshot(c), 80, 20);
  assert.deepEqual(unstyled(rendered.text).split('\n').slice(2, -1),
    [...Array.from({ length: 9 }, (_, i) => `[${i + 1}] Issue: Issue ${i + 1}`), '[10] PR: Ten', '[11] PR: Eleven']);
  assert.equal(rendered.documents.size, 0);
});
test('numbered HTTP labels retain sanitization, clipping, OSC8 targets and overflow', () => {
  const url = 'https://example.com/issues/1';
  const c = { ...card, issues: [{ url, description: 'Ab\x07\u202ecd' }],
    prs: [{ url: 'https://example.com/pulls/2', description: 'Hidden' },
      { url: 'https://example.com/pulls/3', description: 'Also hidden' }] };
  for (const width of [0, 1, 2, 3, 4, 8, 12, 80]) {
    const rendered = renderCard(snapshot(c), width, 4);
    if (width) {
      const lines = unstyled(rendered.text).split('\n');
      assert.ok(lines.every(line => line.length <= width));
      assert.ok(rendered.text.includes(`\x1b]8;;${url}\x1b\\`));
      assert.equal(lines[3], '… 2 more; enlarge pane'.slice(0, width));
    } else assert.equal(rendered.text, '');
    assert.equal(rendered.documents.size, 0);
    assert.doesNotMatch(rendered.text, /\x07|\u202e|Hidden|pulls\/2/);
  }
});
test('document keys skip q and stop safely after z without numeric shortcuts', () => {
  const c = { ...card, documents: Array.from({ length: 28 }, (_, i) => ({ path: 'docs/' + i + '.md', description: '' })) };
  const rendered = renderCard(snapshot(c), 80, 40);
  assert.deepEqual([...rendered.documents.keys()], [...'abcdefghijklmnoprstuvwxyz']);
  assert.match(unstyled(rendered.text), /\[p\] docs\/15.md\n\[r\] docs\/16.md/);
  assert.match(unstyled(rendered.text), /\[z\] docs\/24.md\ndocs\/25.md\ndocs\/26.md\ndocs\/27.md\nq close$/);
  assert.doesNotMatch(rendered.text, /\[q\]|\[\d\]|undefined/);
});
test('long paths remain clipped and sanitized with only fully painted shortcuts active', () => {
  const c = { ...card, documents: [{ path: 'docs/very-long-design\x1b\u202e.md', description: 'Do not show this' }] };
  for (const width of [0, 1, 2, 3, 4, 8]) {
    const rendered = renderCard(snapshot(c), width, 3);
    assert.ok(unstyled(rendered.text).split('\n').every(line => Array.from(line).length <= width));
    assert.doesNotMatch(rendered.text, /\x1b\[2J|\u202e|Do not show this/);
    assert.equal(rendered.documents.has('a'), width >= 3);
  }
  assert.match(unstyled(renderCard(snapshot(c), 80, 3).text), /\[a\] docs\/very-long-design.md/);
});
test('digits are inert and j/k are normal document intents, q exits display only', async t => {
  const f = await fixture(t);
  const documents = Array.from({ length: 11 }, (_, i) => ({ path: 'doc-' + i + '.md', description: '' }));
  const saved = await createCard({ ...card, title: 'Many documents', cwd: f.repo, documents }, f.cards);
  const view = new CardView(saved.path); await view.refresh(); view.render(80, 20);
  for (const digit of '0123456789') assert.deepEqual(await view.key(digit), { kind: 'none' });
  for (const [key, path] of [['j', 'doc-9.md'], ['k', 'doc-10.md']]) {
    const intent = await view.key(key);
    assert.equal(intent.kind, 'open-document'); assert.equal(intent.relativePath, path);
    assert.equal(intent.snapshot.path, saved.path);
  }
  assert.deepEqual(await view.key('q'), { kind: 'exit' });
  assert.deepEqual(await view.key('\x03'), { kind: 'exit' });
});

test('HTTP links use OSC8, sanitize display controls, and never offer scheme actions', () => {
  const c = { ...card, title: 'Title\x1b[2J\u202e', issues: [{ url: 'https://example.com/1', description: 'Issue\x07' },
    { url: 'file:///etc/passwd', description: 'Unsafe' }] };
  const rendered = renderCard(snapshot(c), 80, 8);
  assert.match(rendered.text, /\x1b\]8;;https:\/\/example.com\/1/);
  assert.doesNotMatch(rendered.text, /\x1b\[2J|\u202e|\x07|\x1b\]8;;file:/);
  assert.equal(plain('a\x00\u202eb'), 'ab');
});
test('raw input ignores escape strings, arrows, split paste sequences and pasted shortcuts', () => {
  const input = new KeyInput();
  assert.deepEqual(input.feed(Buffer.from('01234567891011')), []);
  assert.deepEqual(input.feed(Buffer.from('\x1b[A\x1b]8;;q1\x1b\\')), []);
  assert.deepEqual(input.feed(Buffer.from('\x1b[20')), []);
  assert.deepEqual(input.feed(Buffer.from('0~qa123\x1b[201~aq')), ['a', 'q']);
  assert.deepEqual(input.feed(Buffer.from('\x03')), ['\x03']);
});
test('document intent carries the freshly read snapshot and does not read document bytes', async t => {
  const f = await fixture(t); await rm(join(f.repo, 'notes.md'));
  const view = new CardView(f.saved.path); await view.refresh(); view.render(30, 6);
  await writeFile(f.saved.path, (await readFile(f.saved.path, 'utf8')).replace('state: State', 'state: Changed'));
  const intent = await view.key('a');
  assert.equal(intent.kind, 'open-document'); assert.equal(intent.relativePath, 'notes.md');
  assert.equal(intent.snapshot.card.state, 'Changed');
  assert.notEqual(intent.snapshot.revision, f.saved.revision);
  assert.match(view.render(30, 6), /Title/);
});

test('read failures disable stale document actions, and changed shortcut targets require a second press', async t => {
  const f = await fixture(t); const view = new CardView(f.saved.path);
  await view.refresh(); view.render(80, 10);
  await writeFile(f.saved.path, 'bad: schema');
  assert.deepEqual(await view.key('a'), { kind: 'none' });
  assert.equal(view.current, undefined); assert.match(view.render(80, 10), /unavailable/i);
  const valid = await createCard({ ...card, title: 'Second', cwd: f.repo, documents: [{ path: 'notes.md', description: '' }] }, f.cards);
  const v = new CardView(valid.path); await v.refresh(); v.render(80, 10);
  await writeFile(valid.path, (await readFile(valid.path, 'utf8')).replace('notes.md', 'new.md'));
  assert.deepEqual(await v.key('a'), { kind: 'none' });
  assert.equal(v.notice.source, 'document'); assert.match(v.notice.text, /changed/i);
  v.render(80, 10); assert.equal((await v.key('a')).relativePath, 'new.md');
});

test('a changed checkout cannot reuse a painted document shortcut, even after background refresh', async t => {
  const f = await fixture(t); const view = new CardView(f.saved.path);
  await view.refresh(); view.render(80, 10);
  const other = join(f.root, 'other-checkout'); await mkdir(other);
  await writeFile(f.saved.path, (await readFile(f.saved.path, 'utf8')).replace(f.repo, other));
  await view.refresh(); // Refreshed, but the new checkout has not been painted.
  assert.deepEqual(await view.key('a'), { kind: 'none' });
  assert.equal(view.notice.source, 'document'); assert.match(view.notice.text, /changed/i);
  view.render(80, 10); assert.equal((await view.key('a')).snapshot.card.cwd, other);
});






test('a repaired card clears its read-error notice on the next normal poll', async t => {
  const f = await fixture(t); const view = new CardView(f.saved.path);
  const original = await readFile(f.saved.path);
  await writeFile(f.saved.path, 'bad: schema'); await view.refresh(); assert.equal(view.notice.source, 'card');
  view.setNotice('card', 'Arbitrary read failure wording');
  await writeFile(f.saved.path, original); await view.refresh(); assert.equal(view.notice, undefined);
  view.render(80, 10); assert.equal((await view.key('a')).kind, 'open-document');
});

test('notice clearing uses sources rather than wording and latest notice wins', async t => {
  const f = await fixture(t); const view = new CardView(f.saved.path); await view.refresh();
  view.setNotice('projection', 'Arbitrary publishing failure wording');
  view.clearNotice('document'); assert.equal(view.notice.source, 'projection');
  view.clearNotice('projection'); assert.equal(view.notice, undefined);
  for (const source of ['document', 'projection']) {
    view.setNotice(source, 'Card unavailable (a document may contain this wording)');
    await view.refresh(); assert.equal(view.notice.source, source);
  }
  view.setNotice('document', 'Herdr projection unavailable: document error');
  view.clearNotice('projection'); assert.equal(view.notice.source, 'document');
  view.setNotice('card', 'Herdr projection unavailable: card error');
  view.clearNotice('projection'); assert.equal(view.notice.source, 'card');
  await view.refresh(); assert.equal(view.notice, undefined);
});
test('only attempted visible document navigation clears notices; exit does not', async t => {
  const f = await fixture(t); const view = new CardView(f.saved.path); await view.refresh(); view.render(80, 10);
  view.setNotice('document', 'Card unavailable (document error)');
  await view.refresh(); assert.equal(view.notice.source, 'document');
  await view.key('b'); assert.equal(view.notice.source, 'document');
  assert.equal((await view.key('a')).kind, 'open-document'); assert.equal(view.notice, undefined);
  view.setNotice('document', 'Document failure');
  assert.deepEqual(await view.key('q'), { kind: 'exit' }); assert.equal(view.notice.source, 'document');
});

test('structured notices retain row bounds and hide offscreen shortcuts', async t => {
  const f = await fixture(t); const view = new CardView(f.saved.path); await view.refresh();
  view.setNotice('projection', 'Arbitrary notice\x1b[2J');
  assert.equal(view.render(20, 3).split('\n').length, 3);
  assert.doesNotMatch(view.render(20, 3), /\x1b\[2J/);
  assert.deepEqual(await view.key('a'), { kind: 'none' });
  view.render(80, 10); assert.equal((await view.key('a')).kind, 'open-document');
  view.setNotice('projection', 'Publishing failure');
  assert.ok(view.render(80, 4).split('\n').length <= 4);
  assert.equal(view.render(80, 1), 'Publishing failure'); assert.equal(view.render(80, 0), '');
});

test('tiny panes never retain an offscreen document shortcut', async t => {
  const f = await fixture(t); const view = new CardView(f.saved.path); await view.refresh(); view.render(80, 10);
  view.render(5, 1); assert.deepEqual(await view.key('a'), { kind: 'none' });
});
