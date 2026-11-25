import assert from 'node:assert/strict';
import test from 'node:test';
import { createJiti } from 'jiti';

const {
  SessionModelChangesSchema,
  SessionWireSchema,
  StreamModelChangesSchema,
  StreamWireSchema,
  ReferenceEntrySchema,
  ReferenceProposalSchema,
  isReferenceEntry,
  isReferenceText,
  isSession,
  isStream,
  normalizeDocumentKey,
  parseLocalContext,
  referenceUrl,
  sessionKeyOf,
  streamKeyOf,
} = await createJiti(import.meta.url).import(
  new URL('../extensions/stream-checkpoint/wire.ts', import.meta.url).pathname
);

const stream = () => ({
  host_id: 'host',
  id: 'effort',
  title: { text: 'Streams', manual: false },
  goal: null,
  status: 'active',
  revision: 1,
  updated_at_ms: 0,
});

const session = () => ({
  host_id: 'host',
  harness: 'pi',
  id: 'native-session',
  stream_id: 'effort',
  title: { text: 'Catalog', manual: false },
  status: 'active',
  recap: null,
  state: null,
  remaining: null,
  directory: { path: '/repo/work', repository: null },
  references: {},
  artifacts: {},
  revision: 1,
  updated_at_ms: 0,
});

const context = () => ({
  stream: stream(),
  session: session(),
  naming: { stream: 'automatic', session: 'manual' },
});

test('format-5 local context validates naming modes and record keys', () => {
  const current = context();
  assert.deepEqual(parseLocalContext(current), current);
  assert.equal(streamKeyOf(current.stream), 'host/effort');
  assert.equal(sessionKeyOf(current.session), 'host/pi/native-session');

  for (const change of [
    (v) => delete v.naming,
    (v) => (v.naming.stream = 'inherit'),
    (v) => (v.naming.session = 'inherit'),
    (v) => (v.session.title = 'plain string'),
  ]) {
    const invalid = context();
    change(invalid);
    assert.throws(() => parseLocalContext(invalid), /Invalid local catalog context/);
  }
});

test('local context rejects inconsistent stream_id membership', () => {
  const invalid = context();
  invalid.session.stream_id = 'other';
  assert.throws(() => parseLocalContext(invalid), /Inconsistent addressed catalog context/);

  const unattached = { ...context(), session: null };
  assert.doesNotThrow(() => parseLocalContext(unattached));
});

test('format-5 Session references validate kind, role, status, manual and exact text', () => {
  const entry = {
    kind: 'link',
    name: 'Brief',
    purpose: 'Define acceptance',
    role: 'brief',
    status: 'active',
    manual: false,
  };
  for (const kind of ['document', 'pr', 'issue', 'link']) {
    for (const role of ['deliverable', 'dependency', 'brief', 'background']) {
      for (const status of ['active', 'fulfilled', 'dropped']) {
        for (const manual of [true, false]) {
          assert.equal(isReferenceEntry({ ...entry, kind, role, status, manual }), true);
        }
      }
    }
  }
  assert.equal(isReferenceEntry({ ...entry, extra: true }), false);
  assert.equal(isReferenceEntry({ ...entry, state: 'active' }), false);
  assert.equal(isReferenceEntry({ ...entry, status: 'unknown' }), false);
  assert.equal(isReferenceEntry({ ...entry, role: 'Brief' }), false);
  assert.equal(isReferenceEntry({ ...entry, manual: 'yes' }), false);
});

test('reference text matches Rust whitespace and Unicode scalar validity without trimming', () => {
  for (const text of ['\ufeff', '😀', 'leading\nnewline', "--value='exact'", '\u001b']) {
    assert.equal(isReferenceText(text), true, JSON.stringify(text));
  }
  for (const text of ['', ' \n', '\u0085', '\u00a0\u3000', 'bad\0text', '\ud800', '\udfff']) {
    assert.equal(isReferenceText(text), false, JSON.stringify(text));
  }
});

test('referenceUrl accepts only clean http(s) URLs without credentials', () => {
  assert.equal(Boolean(referenceUrl('https://example.test/pr/42')), true);
  assert.equal(Boolean(referenceUrl('http://localhost:8080/doc')), true);
  assert.equal(Boolean(referenceUrl('https://user:pass@example.test/pr/42')), false);
  assert.equal(Boolean(referenceUrl('file:///etc/passwd')), false);
  assert.equal(Boolean(referenceUrl('not a url')), false);
});

test('normalizeDocumentKey converts paths inside session directory and rejects escapes', () => {
  assert.equal(normalizeDocumentKey('/repo/work', 'docs/plan.md'), 'docs/plan.md');
  assert.equal(normalizeDocumentKey('/repo/work', './docs/plan.md'), 'docs/plan.md');
  assert.equal(normalizeDocumentKey('/repo/work', '/repo/work/docs/plan.md'), 'docs/plan.md');
  assert.equal(normalizeDocumentKey('/repo/work', '../other/plan.md'), null);
  assert.equal(normalizeDocumentKey('/repo/work', '/other/plan.md'), null);
  assert.equal(normalizeDocumentKey('/repo/work', ''), null);
  assert.equal(normalizeDocumentKey(undefined, 'docs/plan.md'), null);
});

test('isStream and isSession validate format-5 records', () => {
  assert.equal(isStream(stream()), true);
  assert.equal(isSession(session()), true);
  assert.equal(
    isSession({
      ...session(),
      directory: {
        path: '/repo',
        repository: { url: 'git@example.test:o/r.git', within: null, branch: 'main' },
      },
    }),
    true
  );
  assert.equal(
    isSession({
      ...session(),
      directory: {
        path: '/repo/sub',
        repository: { url: 'git@example.test:o/r.git', within: 'sub', branch: null },
      },
    }),
    true
  );
  assert.equal(isStream({ ...stream(), title: 'old' }), false);
  assert.equal(isSession({ ...session(), title: 'old' }), false);
});

test('derived model-change and proposal schemas match canonical wire schemas by construction', async () => {
  const { Check } = await import('typebox/value');
  assert.equal(Check(StreamWireSchema, stream()), true);
  assert.equal(Check(SessionWireSchema, session()), true);

  // ReferenceProposalSchema has every ReferenceEntry field except manual, plus key.
  const entryKeys = Object.keys(ReferenceEntrySchema.properties).filter((k) => k !== 'manual').sort();
  const proposalKeys = Object.keys(ReferenceProposalSchema.properties).filter((k) => k !== 'key').sort();
  assert.deepEqual(proposalKeys, entryKeys);

  // Report change schemas use TextPair objects (no legacy *_short flat keys) and only allow null on remaining.
  assert.deepEqual(Object.keys(SessionModelChangesSchema.properties).sort(), [
    'branch',
    'recap',
    'remaining',
    'state',
    'status',
    'title',
  ]);
  assert.deepEqual(Object.keys(StreamModelChangesSchema.properties).sort(), ['goal', 'title']);
  assert.equal(Check(SessionModelChangesSchema, { recap: { text: 'Built', short: 'Built' } }), true);
  assert.equal(Check(SessionModelChangesSchema, { recap: 'flat string' }), false);
  assert.equal(Check(SessionModelChangesSchema, { recap: null }), false);
  assert.equal(Check(SessionModelChangesSchema, { remaining: null }), true);
  assert.equal(Check(StreamModelChangesSchema, { goal: { text: 'Purpose', short: null } }), true);
  assert.equal(Check(StreamModelChangesSchema, { goal: null }), false);
});
