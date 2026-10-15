import test from 'node:test';
import assert from 'node:assert/strict';
import stringWidth from 'string-width';
import { renderDisplayCard, plain } from '../dist/herdr/view.js';

const card = { workspaceTitle: 'Workspace', title: 'Assignment', status: 'active', goal: 'Purpose', now: 'Stage', remaining: 'Next work',
  documents: [{ label: 'docs/design.md', target: { checkout: 'repo', path: 'docs/design.md' } }],
  issues: [{ url: 'https://example.com/1', description: 'Design' }], prs: [] };
const unstyled = text => text.replace(/\x1b\[[\d;]*m/g, '').replace(/\x1b\]8;;[^\x1b]*\x1b\\/g, '');

test('empty structured fields omit empty sections and keep title-only rows without colons', () => {
  const display = { title: 'Agent', workspaceTitle: 'Workspace', status: 'active',
    goal: '', now: '', remaining: '', documents: [], issues: [], prs: [] };
  const lines = unstyled(renderDisplayCard(display, 40, 5).text).split('\n');
  assert.equal(lines[0], 'Workspace');
  assert.equal(lines[1], 'Agent'.padEnd(32) + '● active');
  assert.equal(lines[2], '');
  const untitled = { ...display, title: '', workspaceTitle: '', goal: 'Purpose', now: 'Stage', remaining: '' };
  const untitledText = renderDisplayCard(untitled, 40, 5).text;
  assert.ok(untitledText.startsWith('Purpose\nStage'));
  const body = unstyled(untitledText).split('\n');
  assert.equal(body[0], 'Purpose');
  assert.equal(body[1], 'Stage'.padEnd(32) + '● active');
  assert.equal(body[2], '');
  const punctuation = renderDisplayCard({ ...display, workspaceTitle: 'Workspace:' }, 40, 5).text;
  assert.ok(punctuation.startsWith('\x1b[3mWorkspace:\x1b[0m\n'));
});


for (const field of ['workspaceTitle', 'title']) test(`${field} style splits preserve title/colon graphemes with tiny panes and footer`, () => {
  const segmenter = new Intl.Segmenter(undefined, { granularity: 'grapheme' });
  const display = { workspaceTitle: 'Workspace', title: 'Agent', status: 'active',
    goal: 'Goal', now: 'Now', remaining: 'Remaining', documents: [], issues: [], prs: [] };
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


test('display sanitization normalizes lone surrogates but preserves paired emoji', () => {
  assert.equal(plain('a\ud800b\udc00c'), 'a�b�c');
  assert.equal(plain('a\ud83d\ude00b'), 'a😀b');
  assert.equal(plain('\ud800\x00\udc00'), '��');
  assert.equal(plain('\ud800\x1b[31m\u202e\udc00'), '�[31m�');
});



test('sections, references, notices and footer remain sanitized, UTF-8 and cell bounded', () => {
  const evil = '\x1b[31m\x1b]8;;https://evil.invalid\x1b\\\x07\u202e';
  const c = { ...card, workspaceTitle: '界😀é\ud800' + evil, title: 'Own' + evil,
    goal: 'Purpose | literal pipe', now: '界'.repeat(30) + evil, remaining: 'Scope\udc00',
    documents: [{ label: 'docs/界😀é\ud800.md', target: { checkout: 'repo', path: 'docs/\ud800.md' } }] };
  for (let rows = 0; rows <= 10; rows++) for (let columns = 0; columns <= 40; columns++) {
    for (const notice of [undefined, 'Notice\ud800\x1b[2J']) {
      const rendered = renderDisplayCard(c, columns, rows, notice);
      const text = unstyled(rendered.text), lines = text.split('\n');
      assert.equal(rendered.text, Buffer.from(rendered.text).toString('utf8'));
      assert.ok(!text || lines.length <= rows);
      assert.ok(lines.every(line => stringWidth(line) <= columns));
      assert.doesNotMatch(rendered.text, /\x1b\[2J|\x1b\[31m|\u202e|\x07|\x1b\]8;;https:\/\/evil/);
      if (rows && columns >= 3) assert.ok(lines.at(-1).endsWith(columns >= 9 ? '[q] close' : '[q]'));
      for (const key of rendered.documents.keys()) {
        assert.ok(lines.some(line => line.startsWith(`[${key}]`)));
        assert.equal(rendered.documents.get(key).path, 'docs/\ud800.md');
      }
      assert.equal([...rendered.text.matchAll(/\x1b\]8;;https:/g)].length, [...rendered.text.matchAll(/\x1b\]8;;\x1b\\/g)].length);
    }
  }
});

test('all statuses use symbol and word independent of colour', () => {
  for (const [status, symbol, colour] of [['active', '●', 36], ['waiting', '○', 33], ['blocked', '!', 31], ['parked', '○', 2], ['done', '✓', 32]]) {
    const text = renderDisplayCard({ ...card, status }, 80, 8).text;
    assert.ok(text.includes(`\x1b[${colour}m${symbol} ${status}\x1b[0m`));
    assert.ok(unstyled(text).split('\n')[1].endsWith(`${symbol} ${status}`));
  }
});

test('long words wrap only at grapheme boundaries, including one-cell wide glyph replacement', () => {
  for (const word of ['界'.repeat(6), 'é'.repeat(8), '👍🏽'.repeat(6), '🇫🇷'.repeat(6), 'abcdefghi']) {
    for (const columns of [1, 5, 20]) {
      const c = { ...card, workspaceTitle: '', title: '', goal: word, now: '', remaining: '', documents: [], issues: [] };
      const text = unstyled(renderDisplayCard(c, columns, 30).text);
      const lines = text.split('\n');
      const content = lines.map(line => line.replace(/ *● active$/, ''));
      const body = content.slice(0, content.findIndex(line => !line));
      const segments = [...new Intl.Segmenter(undefined, { granularity: 'grapheme' }).segment(word)];
      assert.equal(body.join(''), segments.map(({ segment }) => stringWidth(segment) > columns ? '…' : segment).join(''));
      assert.ok(lines.every(line => stringWidth(line) <= columns));
      assert.ok(lines.every(line => !/^\p{M}/u.test(line)));
    }
  }
});

test('palette styling and OSC8 close before the footer and every line has balanced trusted styles', () => {
  const text = renderDisplayCard(card, 80, 8).text;
  assert.ok(text.includes('\x1b[3mWorkspace\x1b[0m: Purpose'));
  assert.ok(text.includes('\x1b[3mAssignment\x1b[0m: Stage'));
  assert.ok(text.includes('\x1b[3mNext work\x1b[0m'));
  assert.ok(text.includes('\x1b[2mdocs/\x1b[0m\x1b[1mdesign.md\x1b[0m'));
  assert.ok(text.endsWith('\x1b[2m[q] close\x1b[0m'));
  for (const line of text.split('\n')) {
    assert.equal([...line.matchAll(/\x1b\[(?:1|2|3|31|32|33|36)m/g)].length, [...line.matchAll(/\x1b\[0m/g)].length);
  }
});
