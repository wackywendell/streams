import test from 'node:test';
import assert from 'node:assert/strict';
import stringWidth from 'string-width';
import { renderDisplayCard, plain } from '../dist/herdr/view.js';

const card = { title: 'Assignment', status: 'active', goal: 'Purpose', now: 'Stage', remaining: 'Next work',
  documents: [{ label: 'docs/design.md', target: { checkout: 'repo', path: 'docs/design.md' } }],
  issues: [{ url: 'https://example.com/1', description: 'Design' }], prs: [] };
const unstyled = text => text.replace(/\x1b\[[\d;]*m/g, '').replace(/\x1b\]8;;[^\x1b]*\x1b\\/g, '');

test('groups spend only surplus rows on separators and preserve the otherwise-empty footer row', () => {
  const display = { ...card, issues: [] };
  const render = rows => unstyled(renderDisplayCard(display, 80, rows).text).split('\n');
  const tight = render(5);
  assert.equal(tight[0], 'Purpose');
  assert.match(tight[1], /^Assignment: Stage +● active$/);
  assert.equal(tight[2], 'Remaining: Next work');
  assert.equal(tight[3], '[a] docs/design.md');
  assert.equal(tight[4].trim(), '[q] close');
  const oneGap = render(6);
  assert.equal(oneGap[1], '');
  assert.equal(oneGap[3], 'Remaining: Next work');
  assert.equal(oneGap[4], '[a] docs/design.md');
  assert.equal(oneGap[5].trim(), '[q] close');
  const bothGaps = render(7);
  assert.equal(bothGaps[1], '');
  assert.equal(bothGaps[4], '');
  assert.equal(bothGaps[5], '[a] docs/design.md');
  assert.equal(bothGaps[6].trim(), '[q] close');
  const noticed = unstyled(renderDisplayCard(display, 80, 6, 'Notice').text).split('\n');
  assert.equal(noticed[1], '');
  assert.equal(noticed[4], '[a] docs/design.md');
  assert.match(noticed[5], /^Notice +\[q\] close$/);
  const noGoal = unstyled(renderDisplayCard({ ...display, goal: '', remaining: '' }, 80, 5).text).split('\n');
  assert.match(noGoal[0], /^Assignment: Stage/);
  assert.equal(noGoal[1], '');
  assert.equal(noGoal[2], '[a] docs/design.md');
  const noRefs = unstyled(renderDisplayCard({ ...display, documents: [] }, 80, 5).text).split('\n');
  assert.equal(noRefs[1], '');
  assert.equal(noRefs[3], 'Remaining: Next work');
  assert.equal(noRefs[4].trim(), '[q] close');
});

test('three complete document labels fit one row at 150 columns, before full-width links', () => {
  const display = { ...card, documents: ['docs/plan.md', 'docs/design.md', 'docs/review.md'].map((label, i) => ({ label, target: { i } })),
    prs: [{ url: 'https://example.com/pr', description: 'Review' }] };
  const rendered = renderDisplayCard(display, 150, 9);
  const lines = unstyled(rendered.text).split('\n');
  const docs = lines.filter(line => /\[[abc]\]/.test(line));
  assert.deepEqual(docs, ['[a] docs/plan.md  [b] docs/design.md  [c] docs/review.md']);
  assert.deepEqual([...rendered.documents.entries()], display.documents.map((document, i) => ['abc'[i], document.target]));
  assert.ok(lines.indexOf('[1] Issue: Design') > lines.indexOf(docs[0]));
  assert.equal(lines[lines.indexOf('[1] Issue: Design') + 1], '[2] PR: Review');
});

test('grid aligns unequal columns row-major, with an incomplete last row', () => {
  const display = { ...card, goal: '', now: '', remaining: '', issues: [],
    documents: ['a.md', 'long-file.md', 'c.md', 'd.md', 'e.md'].map(label => ({ label, target: label })) };
  const rendered = renderDisplayCard(display, 30, 9);
  const docs = unstyled(rendered.text).split('\n').filter(line => /\[[a-e]\]/.test(line));
  assert.deepEqual(docs, ['[a] a.md  [b] long-file.md', '[c] c.md  [d] d.md', '[e] e.md']);
  assert.equal(docs[0].indexOf('[b]'), docs[1].indexOf('[d]'));
  assert.deepEqual([...rendered.documents.values()], display.documents.map(document => document.target));
});

test('grid falls back before truncating full names and remains grapheme and cell safe', () => {
  const labels = ['docs/界😀é.md', 'docs/a\u0d4e.md', 'docs/a-rather-long-filename.md', 'short.md'];
  const display = { ...card, documents: labels.map(label => ({ label, target: label })), issues: [], goal: '', now: '', remaining: '' };
  const narrow = renderDisplayCard(display, 35, 10);
  assert.equal(unstyled(narrow.text).split('\n').filter(line => /\[[a-d]\]/.test(line)).length, 4);
  for (const label of labels) assert.ok(unstyled(narrow.text).includes(label));
  const segmenter = new Intl.Segmenter(undefined, { granularity: 'grapheme' });
  for (let columns = 1; columns <= 80; columns++) for (const rows of [2, 3, 5, 10]) for (const notice of [undefined, 'Notice']) {
    const rendered = renderDisplayCard(display, columns, rows, notice);
    for (const line of rendered.text.split('\n')) {
      const text = unstyled(line);
      assert.ok(stringWidth(text) <= columns);
      const boundaries = new Set([0, text.length, ...Array.from(segmenter.segment(text), part => part.index)]);
      for (const match of line.matchAll(/\x1b\[[\d;]*m/g)) assert.ok(boundaries.has(unstyled(line.slice(0, match.index)).length));
    }
    for (const [key, label] of rendered.documents) {
      assert.ok(unstyled(rendered.text).includes('[' + key + ']'));
      assert.equal(label, labels['abcd'.indexOf(key)]);
    }
  }
});

test('reference budget counts grid rows but overflow counts hidden items, with docs before links', () => {
  const display = { ...card, goal: 'goal '.repeat(30), now: 'now '.repeat(30), remaining: 'next '.repeat(30),
    documents: Array.from({ length: 9 }, (_, i) => ({ label: 'd' + i + '.md', target: i })) };
  const rendered = renderDisplayCard(display, 35, 5);
  const text = unstyled(rendered.text);
  assert.equal(rendered.documents.size, 3);
  assert.match(text, /\[a\] d0.md  \[b\] d1.md  \[c\] d2.md/);
  assert.match(text, /… 7 more/);
  assert.doesNotMatch(text, /\[d\]|Issue:/);
});

test('every painted full grid shortcut maps exactly, while footer-clipped later keys stay inert', () => {
  const display = { ...card, goal: '', title: '', now: '', remaining: '', issues: [],
    documents: ['a.md', 'b.md', 'c.md'].map(label => ({ label, target: { label } })) };
  const full = renderDisplayCard(display, 28, 3);
  assert.equal(full.documents.size, 3);
  const covered = renderDisplayCard(display, 28, 2);
  assert.deepEqual([...covered.documents.keys()], ['a', 'b']);
  assert.ok(unstyled(covered.text).endsWith('[q] close'));
  assert.doesNotMatch(unstyled(covered.text), /\[c\]/);
  const partial = renderDisplayCard(display, 30, 2);
  assert.deepEqual([...partial.documents.keys()], ['a', 'b']);
  for (const rendered of [full, covered, partial]) for (const [key, target] of rendered.documents) {
    assert.deepEqual(target, display.documents['abc'.indexOf(key)].target);
  }
});

test('empty structured fields omit empty sections and keep title-only rows without colons', () => {
  const display = { title: 'Agent', status: 'active',
    goal: '', now: '', remaining: '', documents: [], issues: [], prs: [] };
  const lines = unstyled(renderDisplayCard(display, 40, 5).text).split('\n');
  assert.equal(lines[0], 'Agent'.padEnd(32) + '● active');
  assert.equal(lines[1], '');
  assert.equal(lines[2], '');
  assert.doesNotMatch(renderDisplayCard(display, 40, 5).text, /Remaining/);
  assert.doesNotMatch(renderDisplayCard({ ...display, remaining: ' \n\t' }, 40, 5).text, /Remaining/);
  const untitled = { ...display, title: '', goal: 'Purpose', now: 'Stage', remaining: '' };
  const untitledText = renderDisplayCard(untitled, 40, 5).text;
  assert.ok(untitledText.startsWith('\x1b[2mPurpose\x1b[0m\n\nStage'));
  const body = unstyled(untitledText).split('\n');
  assert.equal(body[0], 'Purpose');
  assert.equal(body[1], '');
  assert.equal(body[2], 'Stage'.padEnd(32) + '● active');
  const punctuation = renderDisplayCard({ ...display, title: 'Agent:' }, 40, 5).text;
  assert.ok(punctuation.startsWith('\x1b[1mAgent:\x1b[0m'));
});


for (const field of ['goal', 'title']) test(`${field} style splits preserve title/colon graphemes with tiny panes and footer`, () => {
  const segmenter = new Intl.Segmenter(undefined, { granularity: 'grapheme' });
  const display = { title: 'Agent', status: 'active',
    goal: 'Goal', now: 'Now', remaining: 'Remaining', documents: [], issues: [], prs: [] };
  display[field] = 'a\u0d4e';
  if (field === 'goal') assert.equal(unstyled(renderDisplayCard(display, 5, 1).text), 'a\u0d4e…[q]');
  const wide = renderDisplayCard(display, 80, 8).text;
  assert.ok(wide.includes(field === 'goal' ? '\x1b[2ma\u0d4e\x1b[0m' : '\x1b[1ma\u0d4e:\x1b[0m Now'));
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
  const c = { ...card, title: 'Own' + evil,
    goal: '界😀é\ud800' + evil + ' Purpose | literal pipe', now: '界'.repeat(30) + evil, remaining: 'Scope\udc00',
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
        assert.ok(lines.some(line => line.includes(`[${key}]`)));
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
    assert.ok(unstyled(text).split('\n').some(line => line.endsWith(`${symbol} ${status}`)));
  }
});

test('long words wrap only at grapheme boundaries, including one-cell wide glyph replacement', () => {
  for (const word of ['界'.repeat(6), 'é'.repeat(8), '👍🏽'.repeat(6), '🇫🇷'.repeat(6), 'abcdefghi']) {
    for (const columns of [1, 5, 20]) {
      const c = { ...card, title: '', goal: word, now: '', remaining: '', documents: [], issues: [] };
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

test('wrapped Goal, Now and Remaining bodies stay normal while only headings are bold', () => {
  const display = { ...card, title: 'Agent',
    goal: 'goal '.repeat(8), now: 'now '.repeat(8), remaining: 'next '.repeat(8), documents: [], issues: [] };
  const text = renderDisplayCard(display, 18, 30).text;
  assert.deepEqual([...text.matchAll(/\x1b\[1m([^\x1b]*)\x1b\[0m/g)].map(match => match[1]), ['Agent:', 'Remaining:']);
  assert.doesNotMatch(text, /\x1b\[3m/);
  for (const word of ['goal', 'now', 'next']) assert.equal(unstyled(text).match(new RegExp(word, 'g')).length, 8);
});

test('palette styling and OSC8 close before the footer and every line has balanced trusted styles', () => {
  const text = renderDisplayCard(card, 80, 8).text;
  assert.ok(text.startsWith('\x1b[2mPurpose\x1b[0m\n'));
  assert.doesNotMatch(text, /Workspace|Goal:/);
  assert.ok(text.includes('\x1b[1mAssignment:\x1b[0m Stage'));
  assert.ok(text.includes('\x1b[1mRemaining:\x1b[0m Next work'));
  assert.ok(text.includes('\x1b[36m[a]\x1b[0m \x1b[2mdocs/\x1b[0mdesign.md'));
  assert.doesNotMatch(text, /\x1b\[3m|\x1b\[1mdesign\.md/);
  assert.ok(text.endsWith('\x1b[2m[q] close\x1b[0m'));
  for (const line of text.split('\n')) {
    assert.equal([...line.matchAll(/\x1b\[(?:1|2|3|31|32|33|36)m/g)].length, [...line.matchAll(/\x1b\[0m/g)].length);
  }
});
