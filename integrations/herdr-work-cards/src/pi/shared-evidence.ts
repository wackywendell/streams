import { createHash } from 'node:crypto';
import type { ReferenceBoundary } from '../records.js';
import type { DialogueText, ProjectedEvidence } from './evidence.js';

export interface SharedEvidence {
  readonly transcript: readonly DialogueText[];
  readonly compactionSummary?: never;
}

export type SharedEvidenceResult =
  | Readonly<{ kind: 'suffix'; evidence: SharedEvidence }>
  | Readonly<{ kind: 'reset-needed'; boundary: Readonly<ReferenceBoundary> }>;

function compactionDigest(evidence: ProjectedEvidence): string {
  return createHash('sha256').update(JSON.stringify(evidence.compactionSummary)).digest('hex');
}

function prefixDigest(transcript: readonly DialogueText[], length: number): string {
  const hash = createHash('sha256').update('[');
  for (let i = 0; i < length; i++) {
    if (i) hash.update(',');
    // Fixed tuples ignore object key order without retaining a second transcript.
    hash.update(JSON.stringify([transcript[i].role, transcript[i].text]));
  }
  return hash.update(']').digest('hex');
}

/** Capture constant-size metadata; the normalized dialogue remains owned by the caller. */
export function captureReferenceBoundary(evidence: ProjectedEvidence): Readonly<ReferenceBoundary> {
  return Object.freeze({
    compactionDigest: compactionDigest(evidence),
    transcriptLength: evidence.transcript.length,
    prefixDigest: prefixDigest(evidence.transcript, evidence.transcript.length),
  });
}

/** Check continuity and identify fresh startup input, not reference eligibility. Tree/resume still require reset. */
export function projectSharedEvidence(
  evidence: ProjectedEvidence,
  boundary: Readonly<ReferenceBoundary>,
  continuity: 'continue' | 'reset',
): SharedEvidenceResult {
  if (continuity !== 'continue' && continuity !== 'reset') throw new Error('Explicit continuity decision required');
  if (continuity === 'reset'
    || evidence.transcript.length < boundary.transcriptLength
    || compactionDigest(evidence) !== boundary.compactionDigest
    || prefixDigest(evidence.transcript, boundary.transcriptLength) !== boundary.prefixDigest) {
    // Runtime must persist this boundary with a new evidence generation before sharing again.
    return { kind: 'reset-needed', boundary: captureReferenceBoundary(evidence) };
  }
  const transcript: DialogueText[] = [];
  for (let i = boundary.transcriptLength; i < evidence.transcript.length; i++) {
    const { role, text } = evidence.transcript[i];
    transcript.push({ role, text });
  }
  return { kind: 'suffix', evidence: { transcript } };
}
