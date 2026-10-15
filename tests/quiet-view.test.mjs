import test from 'node:test';
import assert from 'node:assert/strict';
import stringWidth from 'string-width';
import { mkdtemp, mkdir, rm, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createCard, readCard } from '../dist/card.js';
import { renderCard, renderDisplayCard, CardView, plain } from '../dist/herdr/view.js';

const card = { title: 'Work cards', status: 'active', state: 'Extension built and installed. Shared workspace remains planned.', cwd: '/repo',
  documents: [{ path: 'docs/design.md', description: 'Design' }],
  issues: [{ url: 'https://example.com/1', description: 'Workspace design' }], prs: [] };
const snapshot = c => ({ slug: 'work-cards', path: '/cards/work-cards.yaml', revision: 'r', card: c });
const unstyled = text => text.replace(/\x1b\[[\d;]*m/g, '').replace(/\x1b\]8;;[^\x1b]*\x1b\\/g, '');
const render = (c, columns, rows) => renderCard(snapshot(c), columns, rows);

test('close hint occupies bottom-right over every last-row kind without reserving a row', () => {
  for (const [c, rows] of [[{ ...card, state: '', documents: [], issues: [] }, 1],
    [{ ...card, state: 'long state words repeated here', documents: [], issues: [] }, 2],
    [{ ...card, state: '', issues: [] }, 2], [{ ...card, state: '', documents: [] }, 2],
    [{ ...card, state: '', documents: Array(8).fill({ path: 'notes.md' }), issues: [] }, 3]]) {
    for (const width of [1, 2, 3, 5, 8, 9, 11, 24]) {
      const rendered = render(c, width, rows), lines = unstyled(rendered.text).split('\n');
      assert.equal(lines.length, rows);
      assert.equal(stringWidth(lines.at(-1)), width);
      if (width >= 3) assert.ok(lines.at(-1).endsWith(width >= 9 ? '[q] close' : '[q]'));
      if (rows === 2 && c.documents.length && width - (width >= 9 ? 9 : width >= 3 ? 3 : 0) < 3) assert.equal(rendered.documents.size, 0);
      const last = rendered.text.split('\n').at(-1);
      if (width >= 3 && last.includes('\x1b]8;;https:')) assert.ok(last.indexOf('\x1b]8;;\x1b\\') < last.indexOf('\x1b[2m[q]'));
    }
  }
});

test('resize and fresh-key layout reject shortcuts covered by the footer', async t => {
  const root = await mkdtemp(join(tmpdir(), 'work-cards-footer-')); t.after(() => rm(root, { recursive: true, force: true }));
  const saved = await createCard({ ...card, cwd: root, state: '', issues: [] }, join(root, 'cards'));
  const view = new CardView(saved.path); await view.refresh();
  view.render(12, 2); assert.equal((await view.key('a')).kind, 'open-document');
  view.render(11, 2); assert.equal((await view.key('a')).kind, 'none');
  view.render(5, 2); assert.equal((await view.key('a')).kind, 'none');
  view.render(6, 2); assert.equal((await view.key('a')).kind, 'open-document');
  view.render(12, 2);
  await writeFile(saved.path, (await readFile(saved.path, 'utf8')).replace('state: ""', 'state: New state'));
  assert.equal((await view.key('a')).kind, 'none');
});

test('notice and unavailable frames share bottom-right footer geometry', async () => {
  const view = new CardView('/missing/card.yaml');
  view.setNotice('card', 'Unavailable long notice');
  assert.equal(unstyled(view.render(20, 3)), '\n\nUnavailable[q] close');
  assert.equal(unstyled(view.render(3, 1)), '[q]');
  assert.equal(view.render(0, 1), '');
  assert.deepEqual(await view.key('q'), { kind: 'exit' });
});

test('quiet wide and narrow layouts wrap sentences and retain useful references', () => {
  assert.equal(unstyled(render(card, 72, 8).text), [
    'Work cards                                                      ● active',
    card.state, '[a] docs/design.md', '[1] Issue: Workspace design', '', '', '', ' '.repeat(63) + '[q] close',
  ].join('\n'));
  assert.equal(unstyled(render(card, 24, 8).text), [
    'Work cards      ● active', 'Extension built and', 'installed. Shared', 'workspace remains', 'planned.',
    '[a] docs/design.md', '[1] Issue: Workspace de…', ' '.repeat(15) + '[q] close',
  ].join('\n'));
});

test('structured legacy layout combines italic title with plain goal and retains Now/status', () => {
  const c = { ...card, state: 'Goal: Reliable recovery | Now: Failure reproduced | Remaining: Choose retry' };
  const rendered = render(c, 100, 8).text;
  const lines = unstyled(rendered).split('\n');
  assert.equal(lines[0], 'Work cards: Reliable recovery');
  assert.match(lines[1], /^Now: Failure reproduced +● active$/);
  assert.equal(lines[2], 'Choose retry');
  assert.ok(rendered.includes('\x1b[3mWork cards\x1b[0m: Reliable recovery'));
  assert.ok(rendered.includes('\x1b[3mChoose retry\x1b[0m'));
  assert.doesNotMatch(rendered, /Goal:|Remaining:|\*/);
});

test('empty structured fields omit empty sections and keep title-only rows without colons', () => {
  const display = { title: 'Agent', workspaceTitle: 'Workspace', status: 'active',
    state: { sections: { goal: '', now: '', remaining: '' } }, documents: [], issues: [], prs: [] };
  const lines = unstyled(renderDisplayCard(display, 40, 5).text).split('\n');
  assert.equal(lines[0], 'Workspace');
  assert.equal(lines[1], 'Agent'.padEnd(32) + '● active');
  assert.equal(lines[2], '');
  const untitled = { ...display, title: '', workspaceTitle: '', state: { sections: { goal: 'Purpose', now: 'Stage', remaining: '' } } };
  const untitledText = renderDisplayCard(untitled, 40, 5).text;
  assert.ok(untitledText.startsWith('Purpose\nStage'));
  const body = unstyled(untitledText).split('\n');
  assert.equal(body[0], 'Purpose');
  assert.equal(body[1], 'Stage'.padEnd(32) + '● active');
  assert.equal(body[2], '');
  const punctuation = renderDisplayCard({ ...display, workspaceTitle: 'Workspace:' }, 40, 5).text;
  assert.ok(punctuation.startsWith('\x1b[3mWorkspace:\x1b[0m\n'));
});

test('wrapped structured titles and remaining are italic but bodies and legacy Now stay plain', () => {
  const c = { ...card, title: 'Long workspace title', documents: [], issues: [],
    state: 'Goal: Plain goal body | Now: Plain stage | Remaining: Next work wraps across lines' };
  const styled = render(c, 12, 20).text;
  assert.ok(styled.includes('\x1b[3mLong\x1b[0m\n\x1b[3mworkspace\x1b[0m\n\x1b[3mtitle\x1b[0m: Plain'));
  assert.ok(render(c, 40, 20).text.includes('\nNow: Plain stage'));
  assert.ok(styled.includes('\x1b[3mNext work\x1b[0m'));
  assert.ok(styled.includes('\x1b[3mwraps across\x1b[0m'));
  const one = unstyled(render(c, 20, 1).text);
  assert.ok(one.startsWith('Long worksp'));
  assert.ok(one.endsWith('[q] close'));
});

for (const field of ['workspaceTitle', 'title']) test(`${field} style splits preserve title/colon graphemes with tiny panes and footer`, () => {
  const segmenter = new Intl.Segmenter(undefined, { granularity: 'grapheme' });
  const display = { workspaceTitle: 'Workspace', title: 'Agent', status: 'active',
    state: { sections: { goal: 'Goal', now: 'Now', remaining: 'Remaining' } }, documents: [], issues: [], prs: [] };
  display[field] = 'a\u0d4e';
  if (field === 'workspaceTitle') {
    assert.equal(unstyled(renderDisplayCard(display, 5, 1).text), 'a\u0d4e:[q]');
  }
  const wide = renderDisplayCard(display, 80, 8).text;
  assert.ok(wide.includes('\x1b[3ma\u0d4e:\x1b[0m ' + (field === 'workspaceTitle' ? 'Goal' : 'Now')));
  for (const title of ['a\u0d4e', '界\u0d4e', 'é\u0d4e', '😀\u0d4e']) {
    display[field] = title;
    for (let rows = 1; rows <= 8; rows++) for (let columns = 1; columns <= 32; columns++) {
      for (const notice of [undefined, 'Notice 界😀é']) {
        const text = renderDisplayCard(display, columns, rows, notice).text;
        const lines = text.split('\n');
        assert.equal(lines.length, rows);
        for (const line of lines) {
          const painted = unstyled(line);
          assert.ok(stringWidth(painted) <= columns);
          const boundaries = new Set([0, painted.length, ...Array.from(segmenter.segment(painted), part => part.index)]);
          for (const match of line.matchAll(/\x1b\[[\d;]*m/g)) {
            const boundary = unstyled(line.slice(0, match.index)).length;
            assert.ok(boundaries.has(boundary), `style splits grapheme at ${boundary}: ${JSON.stringify(line)}`);
          }
        }
        if (columns >= 3) assert.ok(unstyled(lines.at(-1)).endsWith(columns >= 9 ? '[q] close' : '[q]'));
      }
    }
  }
});

test('canonical short state paints separate ordered sections without carrier pipes', () => {
  const c = { ...card, state: 'Goal: Reliable recovery | Now: Failure reproduced | Remaining: Choose retry' };
  assert.deepEqual(unstyled(render(c, 72, 8).text).split('\n').slice(0, 5), [
    'Work cards: Reliable recovery', 'Now: Failure reproduced'.padEnd(64) + '● active', 'Choose retry',
    '[a] docs/design.md', '[1] Issue: Workspace design',
  ]);
  assert.deepEqual(unstyled(render(c, 24, 8).text).split('\n').slice(0, 6), [
    'Work cards: Reliable', 'recovery', 'Now: Failure…   ● active', 'Choose retry',
    '[a] docs/design.md', '[1] Issue: Workspace de…',
  ]);
});

test('section wrapping reserves later headings and ellipsizes within the state budget', () => {
  const c = { ...card, state: 'Goal: Very long purpose that would otherwise consume all the available rows | Now: Usable stage | Remaining: Major unfinished scope' };
  assert.deepEqual(unstyled(render(c, 24, 8).text).split('\n').slice(0, 6), [
    'Work cards: Very long', 'purpose that would…', 'Now: Usable…    ● active', 'Major unfinished scope',
    '[a] docs/design.md', '[1] Issue: Workspace de…',
  ]);
  assert.deepEqual(unstyled(render(c, 24, 6).text).split('\n').slice(0, 5), [
    'Work cards: Very long…', 'Now: Usable…    ● active', 'Major unfinished scope', '[a] docs/design.md', '[1] Issue: Workspace de…',
  ]);
  const noRefs = { ...c, documents: [], issues: [] };
  assert.deepEqual(unstyled(render(noRefs, 40, 3).text).split('\n'), [
    'Work cards: Very long purpose that woul…', 'Now: Usable stage'.padEnd(32) + '● active',
    'Major unfinished scope'.padEnd(31) + '[q] close',
  ]);
});

test('each section wraps independently when there is enough height', () => {
  const c = { ...card, documents: [], issues: [], state: 'Goal: Reliable queue recovery | Now: Failure reproduced | Remaining: Choose retry policy' };
  assert.deepEqual(unstyled(render(c, 20, 12).text).split('\n').slice(0, 6), [
    'Work cards: Reliable', 'queue recovery', 'Now:        ● active', 'Failure', 'reproduced', 'Choose retry policy',
  ]);
  assert.deepEqual(unstyled(render({ ...c, state: 'Goal: Purpose | Now: Stage | Remaining: Work' }, 30, 2).text).split('\n'), [
    'Work cards: Purpose', 'Now: Stage…  ● active[q] close',
  ]);
});

test('malformed section carriers stay whole in the legacy prose layout', () => {
  for (const state of ['Goal: Purpose | Now: Stage', 'Goal: Purpose | Now: Stage | Remaining: Work | Other: Extra',
    'Goal: Purpose | Now: Stage Remaining: Ambiguous | Remaining: Work']) {
    const text = unstyled(render({ ...card, state, documents: [], issues: [] }, 200, 8).text);
    assert.equal(text.split('\n')[1], state);
  }
});

test('section content is sanitized and UTF-8/cell bounded with tiny panes, references, notices and footer', () => {
  const c = { ...card, state: 'Goal: 界😀e\u0301\ud800\x1b[31m\u202e | Now: ' + '界'.repeat(30) + ' | Remaining: Scope\udc00' };
  const wide = render(c, 200, 8).text;
  assert.ok(unstyled(wide).includes('Work cards: 界😀e\u0301�[31m'));
  assert.ok(unstyled(wide).split('\n').includes('Scope�'));
  assert.doesNotMatch(wide, /\x1b\[31m|\u202e/);
  for (let rows = 0; rows <= 10; rows++) for (let columns = 0; columns <= 40; columns++) {
    for (const notice of [undefined, 'Notice\ud800\x1b[2J']) {
      const rendered = renderCard(snapshot(c), columns, rows, notice);
      const text = unstyled(rendered.text), lines = text.split('\n');
      assert.equal(rendered.text, Buffer.from(rendered.text).toString('utf8'));
      assert.ok(!text || lines.length <= rows);
      assert.ok(lines.every(line => stringWidth(line) <= columns));
      assert.doesNotMatch(rendered.text, /\x1b\[2J|\x1b\[31m|\u202e/);
      if (rows && columns >= 3) assert.ok(lines.at(-1).endsWith(columns >= 9 ? '[q] close' : '[q]'));
      for (const key of rendered.documents.keys()) assert.ok(lines.some(line => line.startsWith(`[${key}]`)));
    }
  }
});

test('trusted palette styling is closed and state stays normal contrast', () => {
  const text = render(card, 72, 8).text;
  assert.match(text, /\x1b\[3mWork cards\x1b\[0m/);
  assert.match(text, /\x1b\[36m● active\x1b\[0m/);
  assert.ok(text.includes('\n' + card.state + '\n'));
  assert.match(text, /\x1b\[36m\[a\]\x1b\[0m/);
  assert.match(text, /\x1b\[2mdocs\/\x1b\[0m\x1b\[1mdesign.md\x1b\[0m/);
  assert.match(text, /\x1b\]8;;https:\/\/example.com\/1\x1b\\\x1b\[36m\[1\] Issue: Workspace design\x1b\[0m\x1b\]8;;\x1b\\/);
  assert.ok(text.endsWith('\x1b[2m[q] close\x1b[0m'));
  for (const line of text.split('\n')) {
    const opens = [...line.matchAll(/\x1b\[(?:1|2|3|31|32|33|36)m/g)].length;
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
  assert.equal(unstyled(text), 'Work cards      ● active' + '\n'.repeat(7) + ' '.repeat(15) + '[q] close');
  assert.doesNotMatch(text, /[│┌┐└┘]|Documents:|Issues:|State:/);
});

test('legacy prose is visibly bounded while references and hidden counts remain useful', () => {
  const c = { ...card, state: 'Very verbose historical activity. '.repeat(80), documents: Array.from({ length: 8 }, (_, i) => ({ path: `docs/${i}.md`, description: '' })) };
  const rendered = render(c, 24, 6);
  const lines = unstyled(rendered.text).split('\n');
  assert.equal(lines.length, 6);
  assert.ok(lines[3].endsWith('…'));
  assert.equal(lines[4], '[a] docs/0.md');
  assert.equal(lines[5], '… 8 more; enlar[q] close');
  assert.deepEqual([...rendered.documents.keys()], ['a']);
});

test('long words hard-wrap only at grapheme boundaries and never exceed cell width', () => {
  const words = ['界'.repeat(6), 'e\u0301'.repeat(8), '👍🏽'.repeat(6), '🇫🇷'.repeat(6), 'abcdefghi'];
  for (const state of words) {
    const text = unstyled(render({ ...card, title: 'T', state, documents: [], issues: [] }, 5, 20).text);
    const lines = text.split('\n').slice(1, -1);
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
      assert.equal(unstyled(terminal).split('\n').slice(1, -1).join(''), columns === 1 && unit === '😀' ? '…'.repeat(8) : state.toWellFormed());
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
