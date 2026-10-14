import test from 'node:test';
import assert from 'node:assert/strict';
import stringWidth from 'string-width';
import { mkdtemp, mkdir, rm, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createCard, readCard } from '../dist/card.js';
import { renderCard, CardView, plain } from '../dist/herdr/view.js';

const card = { title: 'Work cards', status: 'active', state: 'Extension built and installed. Shared workspace remains planned.', cwd: '/repo',
  documents: [{ path: 'docs/design.md', description: 'Design' }],
  issues: [{ url: 'https://example.com/1', description: 'Workspace design' }], prs: [] };
const snapshot = c => ({ slug: 'work-cards', path: '/cards/work-cards.yaml', revision: 'r', card: c });
const unstyled = text => text.replace(/\x1b\[[\d;]*m/g, '').replace(/\x1b\]8;;[^\x1b]*\x1b\\/g, '');
const render = (c, columns, rows) => renderCard(snapshot(c), columns, rows);

test('quiet wide and narrow layouts wrap sentences and retain useful references', () => {
  assert.equal(unstyled(render(card, 72, 8).text), [
    'Work cards                                                      ● active',
    card.state, '[a] docs/design.md', '[1] Issue: Workspace design', 'q close',
  ].join('\n'));
  assert.equal(unstyled(render(card, 24, 8).text), [
    'Work cards      ● active', 'Extension built and', 'installed. Shared', 'workspace remains', 'planned.',
    '[a] docs/design.md', '[1] Issue: Workspace de…', 'q close',
  ].join('\n'));
});

test('trusted palette styling is closed and state stays normal contrast', () => {
  const text = render(card, 72, 8).text;
  assert.match(text, /\x1b\[1mWork cards\x1b\[0m/);
  assert.match(text, /\x1b\[36m● active\x1b\[0m/);
  assert.ok(text.includes('\n' + card.state + '\n'));
  assert.match(text, /\x1b\[36m\[a\]\x1b\[0m/);
  assert.match(text, /\x1b\[2mdocs\/\x1b\[0m\x1b\[1mdesign.md\x1b\[0m/);
  assert.match(text, /\x1b\]8;;https:\/\/example.com\/1\x1b\\\x1b\[36m\[1\] Issue: Workspace design\x1b\[0m\x1b\]8;;\x1b\\/);
  assert.ok(text.endsWith('\x1b[2mq close\x1b[0m'));
  for (const line of text.split('\n')) {
    const opens = [...line.matchAll(/\x1b\[(?:1|2|31|32|33|36)m/g)].length;
    assert.equal(opens, [...line.matchAll(/\x1b\[0m/g)].length);
  }
});

test('all statuses use symbol and word, independent of colour and model activity', () => {
  for (const [status, symbol, colour] of [['active', '●', 36], ['waiting', '○', 33], ['blocked', '!', 31], ['parked', '○', 2], ['done', '✓', 32]]) {
    const text = render({ ...card, status }, 40, 8).text;
    assert.ok(text.includes(`\x1b[${colour}m${symbol} ${status}\x1b[0m`));
    assert.ok(unstyled(text).split('\n')[0].endsWith(`${symbol} ${status}`));
  }
});

test('blank state and empty references produce no empty sections or border', () => {
  const text = render({ ...card, state: '', documents: [], issues: [] }, 24, 8).text;
  assert.equal(unstyled(text), 'Work cards      ● active\nq close');
  assert.doesNotMatch(text, /[│┌┐└┘]|Documents:|Issues:|State:/);
});

test('legacy prose is visibly bounded while references and hidden counts remain useful', () => {
  const c = { ...card, state: 'Very verbose historical activity. '.repeat(80), documents: Array.from({ length: 8 }, (_, i) => ({ path: `docs/${i}.md`, description: '' })) };
  const rendered = render(c, 24, 6);
  const lines = unstyled(rendered.text).split('\n');
  assert.equal(lines.length, 6);
  assert.ok(lines[3].endsWith('…'));
  assert.equal(lines[4], '[a] docs/0.md');
  assert.equal(lines[5], '… 8 more; enlarge pane');
  assert.deepEqual([...rendered.documents.keys()], ['a']);
});

test('long words hard-wrap only at grapheme boundaries and never exceed cell width', () => {
  const words = ['界'.repeat(6), 'e\u0301'.repeat(8), '👍🏽'.repeat(6), '🇫🇷'.repeat(6), 'abcdefghi'];
  for (const state of words) {
    const text = unstyled(render({ ...card, title: 'T', state, documents: [], issues: [] }, 5, 20).text);
    const lines = text.split('\n').slice(1);
    assert.equal(lines.join(''), state);
    assert.ok(lines.every(line => stringWidth(line) <= 5));
    assert.ok(lines.every(line => !/^\p{M}/u.test(line)));
  }
});

test('tiny dimensions, wide text and styled links remain bounded and fail closed', () => {
  const c = { ...card, title: '界👍🏽e\u0301', state: '界'.repeat(12), documents: [{ path: '界/e\u0301👍🏽.md', description: '' }] };
  for (let rows = 0; rows <= 9; rows++) for (let columns = 0; columns <= 25; columns++) {
    const rendered = render(c, columns, rows);
    const text = unstyled(rendered.text);
    assert.ok(!text || text.split('\n').length <= rows);
    assert.ok(text.split('\n').every(line => stringWidth(line) <= columns));
    for (const key of rendered.documents.keys()) assert.ok(text.split('\n').some(line => line.startsWith(`[${key}]`)));
    if (!rows || !columns) assert.equal(rendered.text, '');
    assert.equal([...rendered.text.matchAll(/\x1b\]8;;https:/g)].length, [...rendered.text.matchAll(/\x1b\]8;;\x1b\\/g)].length);
  }
});

test('untrusted SGR/OSC/control text is sanitized before trusted styling is applied', () => {
  const evil = '\x1b[31m\x1b]8;;https://evil.invalid\x1b\\\x07\u202e';
  const c = { ...card, title: 'T' + evil, state: 'S' + evil, documents: [{ path: 'docs/' + evil + 'x.md', description: '' }],
    issues: [{ url: 'https://example.com/%1b', description: evil }] };
  const text = render(c, 200, 8).text;
  assert.doesNotMatch(text, /\x1b\[31m|\x1b\]8;;https:\/\/evil|\x07|\u202e|\x1b\]8;;https:\/\/example/);
  assert.equal(plain('a\x1b[31mb'), 'a[31mb');
});

test('resizing and Unicode notices keep only fully painted document keys active', async t => {
  const root = await mkdtemp(join(tmpdir(), 'work-cards-quiet-')); t.after(() => rm(root, { recursive: true, force: true }));
  const repo = join(root, 'repo'); await mkdir(repo);
  const saved = await createCard({ ...card, cwd: repo, state: '', documents: [{ path: 'notes.md', description: '' }], issues: [] }, join(root, 'cards'));
  const view = new CardView(saved.path); await view.refresh();
  view.setNotice('projection', '界👍🏽e\u0301\x1b[2J\u202e');
  for (const columns of [1, 2, 3, 4, 6, 12]) {
    const text = view.render(columns, 3);
    assert.ok(unstyled(text).split('\n').every(line => stringWidth(line) <= columns));
    assert.doesNotMatch(text, /\x1b\[2J|\u202e/);
    assert.equal((await view.key('a')).kind, columns >= 3 ? 'open-document' : 'none');
    view.setNotice('projection', '界👍🏽e\u0301\x1b[2J\u202e');
  }
  view.render(80, 1); assert.deepEqual(await view.key('a'), { kind: 'none' });
});

test('display sanitization normalizes lone surrogates but preserves paired emoji', () => {
  assert.equal(plain('a\ud800b\udc00c'), 'a�b�c');
  assert.equal(plain('a\ud83d\ude00b'), 'a😀b');
  assert.equal(plain('\ud800\x00\udc00'), '��');
  assert.equal(plain('\ud800\x1b[31m\u202e\udc00'), '�[31m�');
});

test('escaped YAML surrogate titles and state remain bounded after terminal UTF-8 encoding', async t => {
  const root = await mkdtemp(join(tmpdir(), 'work-cards-surrogates-')); t.after(() => rm(root, { recursive: true, force: true }));
  for (const unit of ['\ud800', '\udc00', '\ud83d\ude00']) {
    const title = 'Title' + unit.repeat(4), state = unit.repeat(8);
    await writeFile(join(root, 'example.yaml'), `title: ${JSON.stringify(title)}\nstatus: active\nstate: ${JSON.stringify(state)}\ncwd: /repo\ndocuments: []\nissues: []\nprs: []\n`);
    const saved = await readCard('example', root);
    assert.equal(saved.card.title, title); assert.equal(saved.card.state, state);
    for (const columns of [1, 5, 20]) {
      const rendered = renderCard(saved, columns, 20).text;
      const terminal = Buffer.from(rendered).toString('utf8');
      assert.ok(terminal.split('\n').every(line => stringWidth(line) <= columns));
      assert.equal(rendered, terminal);
      assert.equal(unstyled(terminal).split('\n').slice(1).filter(line => line !== 'q close').join(''), columns === 1 && unit === '😀' ? '…'.repeat(8) : state.toWellFormed());
    }
  }
});

test('escaped YAML document paths and notices are display-normalized without changing targets', async t => {
  const root = await mkdtemp(join(tmpdir(), 'work-cards-surrogates-')); t.after(() => rm(root, { recursive: true, force: true }));
  for (const unit of ['\ud800', '\udc00', '\ud83d\ude00']) {
    const path = `docs/${unit.repeat(8)}.md`;
    const source = join(root, 'example.yaml');
    await writeFile(source, `title: Title\nstatus: active\nstate: ""\ncwd: /repo\ndocuments:\n  - path: ${JSON.stringify(path)}\n    description: Notes\nissues: []\nprs: []\n`);
    const saved = await readCard('example', root);
    assert.equal(saved.card.documents[0].path, path);
    const wide = renderCard(saved, 80, 5);
    assert.ok(unstyled(wide.text).includes(path.toWellFormed()));
    assert.equal(wide.documents.get('a').path, path);
    const view = new CardView(source); await view.refresh();
    for (const columns of [1, 2, 3, 5, 12, 20, 80]) {
      view.setNotice('projection', 'Notice ' + unit.repeat(8));
      const rendered = view.render(columns, 4);
      const terminal = Buffer.from(rendered).toString('utf8');
      assert.ok(terminal.split('\n').every(line => stringWidth(line) <= columns));
      assert.equal(rendered, terminal);
      const intent = await view.key('a');
      if (columns >= 3) {
        assert.equal(intent.kind, 'open-document');
        assert.equal(intent.relativePath, path);
        assert.equal(intent.snapshot.card.documents[0].path, path);
      } else assert.deepEqual(intent, { kind: 'none' });
    }
  }
});

test('fresh-key validation uses the painted notice-reduced layout after state changes', async t => {
  const root = await mkdtemp(join(tmpdir(), 'work-cards-quiet-')); t.after(() => rm(root, { recursive: true, force: true }));
  const repo = join(root, 'repo'); await mkdir(repo);
  const saved = await createCard({ ...card, cwd: repo, state: '', documents: [{ path: 'notes.md', description: '' }, { path: 'other.md', description: '' }] }, join(root, 'cards'));
  const view = new CardView(saved.path); await view.refresh(); view.setNotice('projection', 'Notice');
  const painted = unstyled(view.render(30, 4)); assert.match(painted, /\[a\]/);
  await writeFile(saved.path, (await readFile(saved.path, 'utf8')).replace('state: ""', 'state: Long state now spans several lines and hides references.'));
  assert.deepEqual(await view.key('a'), { kind: 'none' });
  view.render(30, 4); assert.deepEqual(await view.key('a'), { kind: 'none' });
});
