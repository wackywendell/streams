import test from 'node:test';
import assert from 'node:assert/strict';
import { renderDisplayCard, plain, KeyInput } from '../dist/herdr/view.js';

const card = { title: 'Assignment', status: 'active', goal: 'Purpose', now: 'Stage', remaining: '', documents: [], issues: [], prs: [] };
const document = path => ({ label: path, target: { checkout: 'repo', path } });
const unstyled = text => text.replace(/\x1b\[[\d;]*m/g, '').replace(/\x1b\]8;;[^\x1b]*\x1b\\/g, '');

test('compact rendering bounds rows and exposes shortcuts only for displayed documents', () => {
  const c = { ...card, documents: Array.from({ length: 5 }, (_, i) => document(`${i}.md`)) };
  const rendered = renderDisplayCard(c, 25, 4);
  assert.equal(rendered.text.split('\n').length, 4);
  assert.deepEqual([...rendered.documents.values()], [{ checkout: 'repo', path: '0.md' }, { checkout: 'repo', path: '1.md' }]);
  assert.match(unstyled(rendered.text), /… 3 more/);
  for (const key of rendered.documents.keys()) assert.ok(unstyled(rendered.text).includes(`[${key}]`));
  assert.equal(renderDisplayCard(c, 10, 1).documents.size, 0);
});

test('document labels are full paths; issue/PR numbers are independent and consecutive beyond nine', () => {
  const c = { ...card, documents: [document('docs/design.md'), document('adrs/design.md')],
    issues: Array.from({ length: 9 }, (_, i) => ({ url: `https://example.com/issues/${i + 1}`, description: `Issue ${i + 1}` })),
    prs: [{ url: 'https://example.com/pulls/10', description: 'Ten' }, { url: 'https://example.com/pulls/11', description: '' }] };
  const rendered = renderDisplayCard(c, 100, 20), text = unstyled(rendered.text);
  assert.match(text, /\[a\] docs\/design.md  \[b\] adrs\/design.md/);
  assert.match(text, /\[9\] Issue: Issue 9\n\[10\] PR: Ten\n\[11\] PR: https:\/\/example.com\/pulls\/11/);
  assert.deepEqual([...rendered.documents.values()], c.documents.map(d => d.target));
  for (const link of [...c.issues, ...c.prs]) assert.ok(rendered.text.includes(`\x1b]8;;${link.url}\x1b\\`));
  const onlyPR = renderDisplayCard({ ...card, prs: c.prs }, 100, 8).text;
  assert.match(unstyled(onlyPR), /\[1\] PR: Ten/);
});

test('document keys skip q and stop after z without numeric shortcuts', () => {
  const c = { ...card, documents: Array.from({ length: 28 }, (_, i) => document(`docs/${i}.md`)) };
  const rendered = renderDisplayCard(c, 80, 40);
  assert.deepEqual([...rendered.documents.keys()], [...'abcdefghijklmnoprstuvwxyz']);
  assert.match(unstyled(rendered.text), /\[p\] docs\/15.md +\[r\] docs\/16.md/);
  assert.match(unstyled(rendered.text), /\[z\] docs\/24.md\s+docs\/25.md\s+docs\/26.md\s+docs\/27.md/);
  assert.doesNotMatch(rendered.text, /\[\d\]|undefined/);
});

test('clipped and sanitized document labels activate only fully painted keys', () => {
  const c = { ...card, documents: [document('docs/very-long-design\x1b\u202e.md')] };
  for (const width of [0, 1, 2, 3, 4, 8, 80]) {
    const rendered = renderDisplayCard(c, width, 3);
    const lines = unstyled(rendered.text).split('\n');
    assert.ok(lines.every(line => line.length <= width));
    assert.equal(rendered.documents.has('a'), lines.some(line => line.includes('[a]')));
    assert.doesNotMatch(rendered.text, /\u202e/);
  }
});

test('only validated HTTP targets receive OSC8; controls and unsafe URI variants stay inert', () => {
  for (const url of ['file:///etc/passwd', 'javascript:alert(1)', 'https://example.com/%1b', 'https://example.com/%E2%80%AE', 'https://example.com/a b', 'https://example.com/\\evil', 'not a URL']) {
    const rendered = renderDisplayCard({ ...card, issues: [{ url, description: 'Untrusted\x07\u202e' }] }, 100, 8);
    assert.doesNotMatch(rendered.text, /\x1b\]8;;|\x07|\u202e/);
  }
  const rendered = renderDisplayCard({ ...card, issues: [{ url: 'https://example.com/1', description: 'Issue' }] }, 100, 8);
  assert.match(rendered.text, /\x1b\]8;;https:\/\/example.com\/1/);
  assert.equal(plain('a\x00\u202eb'), 'ab');
});

test('raw input ignores escape strings, arrows, split paste sequences and pasted shortcuts', () => {
  const input = new KeyInput();
  assert.deepEqual(input.feed(Buffer.from('01234567891011')), []);
  assert.deepEqual(input.feed(Buffer.from('\x1b[A\x1b]8;;q1\x1b\\')), []);
  assert.deepEqual(input.feed(Buffer.from('\x1b[20')), []);
  assert.deepEqual(input.feed(Buffer.from('0~qa123\x1b[201~aq')), ['a', 'q']);
  assert.deepEqual(input.feed(Buffer.from('\x03')), ['\x03']);
  for (const sequence of ['\x1bOq', '\x1bPaq\x1b\\', '\x1bXaq\x1b\\', '\x1b^aq\x1b\\', '\x1b_aq\x1b\\']) {
    assert.deepEqual(input.feed(Buffer.from(sequence)), []);
  }
});
