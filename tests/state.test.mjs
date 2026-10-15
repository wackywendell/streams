import test from 'node:test';
import assert from 'node:assert/strict';
import { parseStateSections } from '../dist/state.js';
import { validateCardPatch, createCard, readCard, updateCard } from '../dist/card.js';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const canonical = 'Goal: Reliable recovery | Now: Failure reproduced | Remaining: Choose and implement retry';

test('recognizes only complete ordered canonical state without changing stored strings', () => {
  assert.deepEqual(parseStateSections(canonical), {
    goal: 'Reliable recovery', now: 'Failure reproduced', remaining: 'Choose and implement retry',
  });
  assert.deepEqual(validateCardPatch({ state: canonical }), { state: canonical });
  assert.deepEqual(parseStateSections('Goal: Read a|b and a |b | Now: Compare | Remaining: Decide'), {
    goal: 'Read a|b and a |b', now: 'Compare', remaining: 'Decide',
  });
  for (const control of ['\n', '\r', '\t', '\u202e']) assert.throws(() => validateCardPatch({ state: canonical + control }));
});

test('canonical carrier round-trips through existing YAML state and patch contracts', async t => {
  const root = await mkdtemp(join(tmpdir(), 'work-cards-sections-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const saved = await createCard({ title: 'Recovery', status: 'active', state: 'Legacy progress', cwd: '/repo', documents: [], issues: [], prs: [] }, root);
  const updated = await updateCard(saved.slug, { state: canonical }, saved.revision, root);
  assert.equal(updated.card.state, canonical);
  assert.equal((await readCard(saved.slug, root)).card.state, canonical);
  assert.match(await readFile(saved.path, 'utf8'), /state: ['"]?Goal:/);
  assert.deepEqual(Object.keys(updated.card).sort(), ['cwd', 'documents', 'issues', 'prs', 'state', 'status', 'title']);
});

test('legacy, incomplete, duplicate, ambiguous and extra fields fall back without partial extraction', () => {
  for (const state of ['', 'Ordinary prose with Goal: context',
    'Goal: Purpose | Now: Stage', 'Goal: Purpose | Remaining: Work | Now: Stage',
    'Goal: Purpose | Now:  | Remaining: Work', 'Goal: Purpose | Now: Stage | Remaining: ',
    'Goal: Purpose | Now: Stage | Remaining: Work | Other: Extra',
    'Goal: Purpose | Now: Stage | Now: Duplicate | Remaining: Work',
    'Goal: Purpose | Now: Stage Remaining: Ambiguous | Remaining: Work',
    'Goal: Purpose | Now: Stage;Now: Duplicate | Remaining: Work',
    'Goal: Purpose | Now: Stage | Remaining: Work Goal: Duplicate',
    ' Goal: Purpose | Now: Stage | Remaining: Work',
    'Goal: Purpose | Now: Stage | Remaining: Work ',
    '**Goal:** Purpose | Now: Stage | Remaining: Work',
    'Goal: Purpose\n | Now: Stage | Remaining: Work',
  ]) assert.equal(parseStateSections(state), undefined, state);
});
