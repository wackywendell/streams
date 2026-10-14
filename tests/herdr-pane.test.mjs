import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, mkdir, symlink, rm, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createCard, readCard } from '../dist/card.js';
import { DEFAULT_CONFIG } from '../dist/config.js';
import { renderCard, plain, KeyInput, CardView } from '../dist/herdr/view.js';

const card = { title: 'Title', status: 'active', state: 'State', cwd: '/repo', documents: [], issues: [], prs: [] };
const snapshot = c => ({ slug: 'title', path: '/cards/title.yaml', revision: 'r', card: c });
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
  assert.deepEqual(input.feed(Buffer.from('\x1b[A\x1b]8;;q1\x1b\\')), []);
  assert.deepEqual(input.feed(Buffer.from('\x1b[20')), []);
  assert.deepEqual(input.feed(Buffer.from('0~q123\x1b[201~1q')), ['1', 'q']);
  assert.deepEqual(input.feed(Buffer.from('\x03')), ['\x03']);
});
test('preview is read-only, byte-bounded and returns to the card with q', async t => {
  const f = await fixture(t); await writeFile(join(f.repo, 'notes.md'), 'x'.repeat(1000));
  const view = new CardView(f.saved.path, { ...DEFAULT_CONFIG, max_document_bytes: 32 });
  await view.refresh(); view.render(30, 6); await view.key('1');
  assert.equal(view.preview.bytesRead, 32); assert.equal(view.preview.truncated, true);
  assert.match(view.render(30, 6), /q.*back/);
  await view.key('q'); assert.equal(view.preview, undefined); assert.match(view.render(30, 6), /Title/);
});
test('read failures disable stale document actions, and changed shortcut targets require a second press', async t => {
  const f = await fixture(t); const view = new CardView(f.saved.path, DEFAULT_CONFIG);
  await view.refresh(); view.render(80, 10);
  await writeFile(f.saved.path, 'bad: schema'); await view.key('1');
  assert.equal(view.preview, undefined); assert.equal(view.current, undefined); assert.match(view.render(80, 10), /unavailable/i);
  const valid = await createCard({ ...card, title: 'Second', cwd: f.repo, documents: [{ path: 'notes.md', description: '' }] }, f.cards);
  const v = new CardView(valid.path, DEFAULT_CONFIG); await v.refresh(); v.render(80, 10);
  const bytes = await import('node:fs/promises');
  await writeFile(join(f.repo, 'new.md'), 'new');
  const yaml = (await bytes.readFile(valid.path, 'utf8')).replace('notes.md', 'new.md'); await writeFile(valid.path, yaml);
  await v.key('1'); assert.equal(v.preview, undefined); assert.equal(v.notice.source, 'document'); assert.match(v.notice.text, /changed/i);
  v.render(80, 10); await v.key('1'); assert.match(v.preview.text, /new/);
});
test('a changed checkout cannot reuse a painted document shortcut, even after background refresh', async t => {
  const f = await fixture(t); const view = new CardView(f.saved.path, DEFAULT_CONFIG);
  await view.refresh(); view.render(80, 10);
  const other = join(f.root, 'other-checkout'); await mkdir(other); await writeFile(join(other, 'notes.md'), 'different checkout');
  await writeFile(f.saved.path, (await readFile(f.saved.path, 'utf8')).replace(f.repo, other));
  await view.refresh(); // The pane poll runs before key processing, but has not painted the new target yet.
  await view.key('1'); assert.equal(view.preview, undefined); assert.equal(view.notice.source, 'document'); assert.match(view.notice.text, /changed/i);
  view.render(80, 10); await view.key('1'); assert.equal(view.preview.text, 'different checkout');
});
test('preview scrolls to down/up boundaries and clamps when resized', async t => {
  const f = await fixture(t); await writeFile(join(f.repo, 'notes.md'), Array.from({ length: 20 }, (_, i) => `line ${i + 1}`).join('\n'));
  const view = new CardView(f.saved.path, DEFAULT_CONFIG); await view.refresh(); view.render(80, 4); await view.key('1');
  assert.match(view.render(80, 4), /1-3\/20/); assert.match(view.render(80, 4), /line 3/); assert.doesNotMatch(view.render(80, 4), /line 4/);
  await view.key('j'); assert.equal(view.preview.offset, 1); assert.match(view.render(80, 4), /2-4\/20/);
  for (let i = 0; i < 30; i++) await view.key('j');
  assert.equal(view.preview.offset, 17); assert.match(view.render(80, 4), /18-20\/20/); assert.match(view.render(80, 4), /line 20/);
  assert.match(view.render(80, 10), /12-20\/20/); assert.equal(view.preview.offset, 11);
  for (let i = 0; i < 30; i++) await view.key('k');
  assert.equal(view.preview.offset, 0); assert.match(view.render(80, 10), /1-9\/20/);
  assert.equal(view.render(80, 1).split('\n').length, 1); assert.equal(view.render(80, 0), '');
  await view.key('q'); assert.equal(view.preview, undefined);
});
test('card changes reset preview scrolling, and reopening starts at the top', async t => {
  const f = await fixture(t); await writeFile(join(f.repo, 'notes.md'), 'one\ntwo\nthree\nfour\nfive');
  const view = new CardView(f.saved.path, DEFAULT_CONFIG); await view.refresh(); view.render(80, 3); await view.key('1');
  await view.key('j'); assert.equal(view.preview.offset, 1);
  await writeFile(f.saved.path, (await readFile(f.saved.path, 'utf8')).replace('state: State', 'state: Changed'));
  await view.refresh(); assert.equal(view.preview, undefined);
  view.render(80, 3); await view.key('1'); assert.equal(view.preview.offset, 0);
});
test('preview scrolling ignores paste, escape controls and unsupported navigation', async t => {
  const f = await fixture(t); await writeFile(join(f.repo, 'notes.md'), 'one\ntwo\nthree\nfour\nfive');
  const view = new CardView(f.saved.path, DEFAULT_CONFIG); await view.refresh(); view.render(80, 3); await view.key('1');
  const input = new KeyInput();
  for (const key of input.feed(Buffer.from('\x1b[200~jjkkq\x1b[201~\x1b[B\x1b]j\x07'))) await view.key(key);
  await view.key('x'); assert.equal(view.preview.offset, 0);
  for (const key of input.feed(Buffer.from('j'))) await view.key(key);
  assert.equal(view.preview.offset, 1); assert.match(view.render(80, 3), /2-3\/5/);
});
test('byte-capped previews explicitly label the loaded prefix while scrolling', async t => {
  const f = await fixture(t); await writeFile(join(f.repo, 'notes.md'), 'one\ntwo\nthree\nfour\nfive');
  const view = new CardView(f.saved.path, { ...DEFAULT_CONFIG, max_document_bytes: 14 });
  await view.refresh(); view.render(80, 3); await view.key('1'); await view.key('j');
  assert.equal(view.preview.bytesRead, 14); assert.match(view.render(80, 3), /byte-capped/i);
});
test('preview rejects checkout-escaping symlinks through core resolver', async t => {
  const f = await fixture(t); await rm(join(f.repo, 'notes.md')); await writeFile(join(f.root, 'private.md'), 'secret');
  await symlink(join(f.root, 'private.md'), join(f.repo, 'notes.md'));
  const view = new CardView(f.saved.path, DEFAULT_CONFIG); await view.refresh(); view.render(80, 10); await view.key('1');
  assert.equal(view.preview, undefined); assert.equal(view.notice.source, 'document'); assert.match(view.notice.text, /outside/);
});
test('a repaired card clears its read-error notice on the next normal poll', async t => {
  const f = await fixture(t); const view = new CardView(f.saved.path, DEFAULT_CONFIG);
  const { readFile } = await import('node:fs/promises'); const original = await readFile(f.saved.path);
  await writeFile(f.saved.path, 'bad: schema'); await view.refresh(); assert.equal(view.notice.source, 'card'); assert.match(view.notice.text, /unavailable/i);
  view.setNotice('card', 'Arbitrary read failure wording');
  await writeFile(f.saved.path, original); await view.refresh(); assert.equal(view.notice, undefined);
  view.render(80, 10); await view.key('1'); assert.ok(view.preview);
});
test('notice clearing uses sources rather than wording and latest notice wins', async t => {
  const f = await fixture(t); const view = new CardView(f.saved.path, DEFAULT_CONFIG); await view.refresh();
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
test('only attempted visible document navigation and preview back clear all notices', async t => {
  const f = await fixture(t); const view = new CardView(f.saved.path, DEFAULT_CONFIG); await view.refresh(); view.render(80, 10);
  view.setNotice('document', 'Card unavailable (document error)');
  await view.refresh(); assert.equal(view.notice.source, 'document');
  await view.key('2'); assert.equal(view.notice.source, 'document');
  await view.key('1'); assert.ok(view.preview); assert.equal(view.notice, undefined);
  view.setNotice('projection', 'Publishing failure');
  assert.equal(await view.key('q'), false); assert.equal(view.preview, undefined); assert.equal(view.notice, undefined);
  view.setNotice('document', 'Document failure');
  assert.equal(await view.key('q'), true); assert.equal(view.notice.source, 'document');
});
test('structured notices retain card and preview row bounds and hide offscreen shortcuts', async t => {
  const f = await fixture(t); await writeFile(join(f.repo, 'notes.md'), 'one\ntwo\nthree\nfour\nfive');
  const view = new CardView(f.saved.path, DEFAULT_CONFIG); await view.refresh();
  view.setNotice('projection', 'Arbitrary notice\x1b[2J');
  assert.equal(view.render(20, 3).split('\n').length, 3);
  assert.doesNotMatch(view.render(20, 3), /\x1b/);
  await view.key('1'); assert.equal(view.preview, undefined);
  view.render(80, 10); await view.key('1'); assert.ok(view.preview);
  view.setNotice('projection', 'Publishing failure');
  assert.equal(view.render(80, 4).split('\n').length, 4); assert.match(view.render(80, 4), /1-2\/5/);
  for (let i = 0; i < 10; i++) await view.key('j');
  assert.equal(view.preview.offset, 3); assert.match(view.render(80, 4), /4-5\/5/);
  assert.equal(view.render(80, 1), 'Publishing failure'); assert.equal(view.render(80, 0), '');
});
test('tiny panes never retain an offscreen document shortcut', async t => {
  const f = await fixture(t); const view = new CardView(f.saved.path, DEFAULT_CONFIG); await view.refresh(); view.render(80, 10);
  view.render(5, 1); await view.key('1'); assert.equal(view.preview, undefined);
});
