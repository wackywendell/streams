import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { captureReferenceBoundary, projectSharedEvidence } from '../dist/pi/shared-evidence.js';

const user = text => ({ role: 'user', text });
const assistant = text => ({ role: 'assistant', text });
const projection = (transcript, compactionSummary = null) => ({ compactionSummary, transcript });
const digest = value => createHash('sha256').update(JSON.stringify(value)).digest('hex');

function assertReset(result, current) {
  assert.deepEqual(result, { kind: 'reset-needed', boundary: captureReferenceBoundary(current) });
  assert.equal('evidence' in result, false);
}

test('session-start boundary permits the first fresh task but excludes inherited fork history', () => {
  const inherited = projection([user('Parent request'), assistant('Parent result')], 'Parent compaction secret');
  const boundary = captureReferenceBoundary(inherited);
  const current = projection([...inherited.transcript, user('First fresh task')], inherited.compactionSummary);
  const result = projectSharedEvidence(current, boundary, 'continue');
  assert.deepEqual(result, { kind: 'suffix', evidence: { transcript: [user('First fresh task')] } });
  assert.doesNotMatch(JSON.stringify(result), /Parent/);
  assert.equal('compactionSummary' in result.evidence, false);
});

test('explicit tree or unproven-resume reset excludes an old branch despite an exact prefix', () => {
  const baseline = projection([user('Common prefix')]);
  const oldBranch = projection([...baseline.transcript, assistant('Old branch reference')]);
  assertReset(projectSharedEvidence(oldBranch, captureReferenceBoundary(baseline), 'reset'), oldBranch);
});

test('move resets to the observed destination baseline and permits only later dialogue', () => {
  const source = projection([user('Source workspace request')]);
  const moved = projection([...source.transcript, assistant('Before move detection')]);
  const reset = projectSharedEvidence(moved, captureReferenceBoundary(source), 'reset');
  assertReset(reset, moved);
  const later = projection([...moved.transcript, user('Destination request')]);
  assert.deepEqual(projectSharedEvidence(later, reset.boundary, 'continue'), {
    kind: 'suffix', evidence: { transcript: [user('Destination request')] },
  });
});

test('changed compaction, redacted prefix, changed roles, or shorter history requires a fresh baseline', () => {
  const original = projection([user('Original request'), assistant('Original response')], 'Checkpoint');
  const boundary = captureReferenceBoundary(original);
  for (const current of [
    projection([...original.transcript, user('New task')], 'Changed checkpoint'),
    projection([...original.transcript, user('New task')], null),
    projection([user('[redacted]'), assistant('Original response'), user('New task')], 'Checkpoint'),
    projection([assistant('Original request'), assistant('Original response')], 'Checkpoint'),
    projection([user('Original request')], 'Checkpoint'),
  ]) assertReset(projectSharedEvidence(current, boundary, 'continue'), current);
});

test('unchanged projection yields an empty suffix without exposing compaction text', () => {
  const current = projection([user('Existing task')], 'Compaction secret');
  assert.deepEqual(projectSharedEvidence(current, captureReferenceBoundary(current), 'continue'), {
    kind: 'suffix', evidence: { transcript: [] },
  });
  const empty = projection([]);
  assert.deepEqual(projectSharedEvidence(empty, captureReferenceBoundary(empty), 'continue'), {
    kind: 'suffix', evidence: { transcript: [] },
  });
});

test('all later content survives beyond the former 24k limit', () => {
  const baseline = projection([user('Inherited')]);
  const fresh = [user('New task ' + '\n"\\'.repeat(12000)), assistant('Outcome '.repeat(6000))];
  assert.deepEqual(projectSharedEvidence(projection([...baseline.transcript, ...fresh]), captureReferenceBoundary(baseline), 'continue'), {
    kind: 'suffix', evidence: { transcript: fresh },
  });
});

test('boundaries contain only constant-size metadata and deterministic SHA256 digests', () => {
  const small = projection([user('a')], 'summary');
  const large = projection(Array.from({ length: 10000 }, () => user('Long text '.repeat(100))), 'summary');
  const boundary = captureReferenceBoundary(small);
  assert.deepEqual(boundary, {
    compactionDigest: digest('summary'), transcriptLength: 1, prefixDigest: digest([['user', 'a']]),
  });
  assert.deepEqual(Object.keys(captureReferenceBoundary(large)).sort(), Object.keys(boundary).sort());
  assert.ok(JSON.stringify(captureReferenceBoundary(large)).length < 220);
  assert.deepEqual(captureReferenceBoundary(projection([{ text: 'a', role: 'user' }], 'summary')), boundary);
  assert.notEqual(captureReferenceBoundary(projection([])).compactionDigest, captureReferenceBoundary(projection([], '')).compactionDigest);
});

test('input mutation cannot change captured metadata; returned mutation cannot affect own-agent evidence', () => {
  const baseline = projection([user('Inherited')], 'Summary');
  const boundary = captureReferenceBoundary(baseline);
  const captured = structuredClone(boundary);
  baseline.transcript[0].text = 'Changed input';
  baseline.compactionSummary = 'Changed summary';
  assert.deepEqual(boundary, captured);
  assert.throws(() => { boundary.transcriptLength = 0; }, TypeError);

  const current = projection([user('Inherited'), assistant('New answer')], 'Summary');
  const before = structuredClone(current);
  const result = projectSharedEvidence(current, boundary, 'continue');
  assert.deepEqual(current, before);
  result.evidence.transcript[0].text = 'Changed result';
  result.evidence.transcript.push(user('Injected result'));
  assert.deepEqual(current, before);
  current.transcript[1].text = 'Changed source';
  assert.equal(result.evidence.transcript[0].text, 'Changed result');
});

test('missing or invalid continuity never authorizes historical evidence', () => {
  const current = projection([user('Existing task')]);
  const boundary = captureReferenceBoundary(projection([]));
  assert.throws(() => projectSharedEvidence(current, boundary), /continuity/i);
  assert.throws(() => projectSharedEvidence(current, boundary, 'guess'), /continuity/i);
});
