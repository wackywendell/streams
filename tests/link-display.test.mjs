import test from 'node:test';
import assert from 'node:assert/strict';
import stringWidth from 'string-width';
import { renderDisplayCard } from '../dist/herdr/view.js';

const card = { title: 'Agent', status: 'active', goal: '', now: '', remaining: '', documents: [], issues: [], prs: [] };
const plain = value => value.replace(/\x1b\[[\d;]*m/g, '').replace(/\x1b\]8;;[^\x1b]*\x1b\\/g, '');

test('short link names retain artifact numbers, independent shortcuts and exact destinations', () => {
  const issue = { url: 'https://github.com/acme/project/issues/42#issuecomment-900', description: 'The full issue description stays in the record', label: 'OTLP field mismatch' };
  const pr = { url: 'https://github.com/acme/project/pull/19?diff=split', description: 'The full PR description stays in the record', label: 'Go port' };
  const input = { ...card, issues: [issue], prs: [pr] }, before = structuredClone(input);
  const rendered = renderDisplayCard(input, 100, 8);
  assert.match(plain(rendered.text), /\[1\] Issue #42: OTLP field mismatch/);
  assert.match(plain(rendered.text), /\[2\] PR #19: Go port/);
  assert.doesNotMatch(plain(rendered.text), /full .* description/);
  for (const link of [issue, pr]) assert.ok(rendered.text.includes(`\x1b]8;;${link.url}\x1b\\`));
  assert.deepEqual(input, before);
  assert.equal(rendered.documents.size, 0);
});

test('recognized repository paths support enterprise hosts and GitLab nested projects', () => {
  for (const [kind, url, expected] of [
    ['prs', 'https://github.enterprise.test/team/repo/pull/7/files', 'PR #7'],
    ['prs', 'https://forge.test/team/repo/pulls/8', 'PR #8'],
    ['prs', 'https://gitlab.example/team/subgroup/repo/-/merge_requests/123/diffs', 'PR #123'],
    ['issues', 'https://gitlab.example/team/subgroup/repo/-/issues/456', 'Issue #456'],
  ]) {
    const text = plain(renderDisplayCard({ ...card, [kind]: [{ url, description: 'Full fallback' }] }, 100, 6).text);
    assert.ok(text.includes(`[1] ${expected}: Full fallback`), text);
  }
});

test('unknown URLs and mismatched artifact kinds do not invent issue or PR numbers', () => {
  for (const [kind, url] of [
    ['issues', 'https://github.com/acme/repo/actions/runs/123'],
    ['issues', 'https://github.com/acme/repo/pull/19'],
    ['prs', 'https://github.com/acme/repo/issues/19'],
    ['prs', 'https://example.com/19?pull=20#21'],
    ['prs', 'https://github.com/acme/repo/pull/19suffix'],
    ['issues', 'https://example.com/a/b/issues/0'],
  ]) {
    const text = plain(renderDisplayCard({ ...card, [kind]: [{ url, description: 'Other artifact' }] }, 100, 6).text);
    assert.ok(text.includes(`[1] ${kind === 'prs' ? 'PR' : 'Issue'}: Other artifact`));
    assert.doesNotMatch(text, /(?:PR|Issue) #/);
  }
});

test('missing short names fall back to full descriptions or exact URLs', () => {
  const url = 'https://github.com/acme/repo/pull/19';
  const full = plain(renderDisplayCard({ ...card, prs: [{ url, description: 'Full description' }] }, 100, 6).text);
  assert.match(full, /\[1\] PR #19: Full description/);
  const empty = plain(renderDisplayCard({ ...card, prs: [{ url, description: '' }] }, 100, 6).text);
  assert.ok(empty.includes(`[1] PR #19: ${url}`));
});

test('short link text is sanitized and cell-bounded without making digits actionable', () => {
  const link = { url: 'https://github.com/acme/repo/pull/19', description: 'Full', label: '界😀é\x1b[2J\u202e'.repeat(8) };
  for (let width = 1; width <= 80; width++) {
    const rendered = renderDisplayCard({ ...card, prs: [link] }, width, 6);
    assert.ok(plain(rendered.text).split('\n').every(line => stringWidth(line) <= width));
    assert.doesNotMatch(rendered.text, /\x1b\[2J|\u202e/);
    assert.equal(rendered.documents.size, 0);
  }
});
