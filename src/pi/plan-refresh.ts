import { createHash } from 'node:crypto';
import { isDeepStrictEqual } from 'node:util';
import type { ProjectedEvidence } from './evidence.js';
import {
  type Authority, type DocumentIdentity, type RecordStore,
  type Snapshot, type WorkspaceRecord, type Goal, evaluatedGoal, conversationGoal,
  probeWorkspaceDocument, type DocumentProbe, type DocumentProbeReason, type MissingDocument,
} from '../records.js';
import { capturePlanSources, isMarkdownDocument, recheckPlanSources, validatePlanSelection } from './plan-sources.js';
import type { SummaryRegistry, SummaryRequestOptions } from './summarizer.js';
import type { CheckIntent } from './scheduler.js';
import { requestConversationGoal, requestPlanSelection, requestWorkspaceGoal } from './workspace-summary.js';

export type RefreshDiagnostic =
  | { stage: 'own' | 'shared' | 'goal'; outcome: 'applied' | 'unchanged' | 'no_sources' | 'skipped' }
  | { stage: 'shared'; outcome: 'documents_checked' | 'documents_omitted'; missingCount: number; removedCount: number; unresolvedCounts: Partial<Record<DocumentProbeReason, number>> }
  | { stage: 'selection'; outcome: 'selected' | 'no_sources'; selectedCount: number; cacheHit: boolean }
  | { stage: 'own' | 'shared' | 'selection' | 'goal'; outcome: 'failed'; cancelled: boolean };

export interface PlanRefreshInput {
  authority: Authority;
  /** Projected dialogue and compaction only; never reference-only tool activity. */
  evidence?: ProjectedEvidence;
  signal: AbortSignal;
  /** Reject when the host generation or live context no longer matches this request. */
  assertCurrent: () => Promise<void>;
  /** Explicit requests reselect metadata and regenerate Goal even with unchanged provenance. */
  intent?: CheckIntent;
  onRequest?: () => (() => void);
  /** Guarded helper title publication, not cache retention or protected manual proposals. */
  onWorkspaceTitlePublished?: (workspace: Snapshot<WorkspaceRecord>) => void;
  /** Read-only observer; callbacks must not throw. Contains no source or dialogue contents. */
  onDiagnostic?: (event: RefreshDiagnostic) => void;
}

export function reportDocumentProbes(onDiagnostic: PlanRefreshInput['onDiagnostic'], outcome: 'documents_checked' | 'documents_omitted', probes: readonly DocumentProbe[], removedCount = 0): void {
  const missingCount = probes.filter(p => p.kind === 'missing').length;
  const unresolvedCounts: Partial<Record<DocumentProbeReason, number>> = {};
  for (const probe of probes) if (probe.kind === 'unresolved') unresolvedCounts[probe.reason] = (unresolvedCounts[probe.reason] ?? 0) + 1;
  if (missingCount || removedCount || Object.keys(unresolvedCounts).length) onDiagnostic?.({ stage: 'shared', outcome, missingCount, removedCount, unresolvedCounts });
}

/** A single host-owned refresher, not a scheduler. Discard on host invalidation. */
export class PlanRefresher<TModel extends { api?: string }> {
  private retainedConversation?: { workspaceId: string; creationToken: string; sessionId: string; digest: string; goal: Goal };
  private selection?: { workspaceId: string; creationToken: string; metadata: string; sources: DocumentIdentity[] };

  constructor(private readonly config: {
    store: RecordStore;
    registry: SummaryRegistry<TModel>;
    options: SummaryRequestOptions;
    loadHelperProfiles?: () => Promise<unknown>;
  }) {}

  async refresh(input: PlanRefreshInput): Promise<Snapshot<WorkspaceRecord>> {
    const { store, registry, options, loadHelperProfiles } = this.config;
    // Caller-owned authority cannot change under an in-flight refresh.
    const authority = { ...input.authority };
    const evidence = structuredClone(input.evidence ?? { compactionSummary: null, transcript: [] });
    const { signal, assertCurrent, intent = 'automatic', onRequest, onDiagnostic } = input;
    const forced = intent !== 'automatic';
    const current = async (expectedRevision?: string): Promise<Snapshot<WorkspaceRecord>> => {
      signal.throwIfAborted();
      const { workspace } = await store.readMembership(authority.sessionId, authority);
      if (expectedRevision !== undefined && workspace.revision !== expectedRevision) throw new Error('Stale workspace revision during plan refresh');
      await assertCurrent();
      signal.throwIfAborted();
      return workspace;
    };

    const publishedTitle = async (result: Snapshot<WorkspaceRecord>, title: string | undefined): Promise<void> => {
      const published = await current(result.revision);
      if (title !== undefined && published.record.goal.kind === 'known' && published.record.title === title) input.onWorkspaceTitlePublished?.(published);
    };
    let stage: 'selection' | 'goal' = 'selection';
    let sourceReadFailed = false;
    let recovery: { baseline: Snapshot<WorkspaceRecord>; sources: DocumentIdentity[] } | undefined;
    try {
      const original = await current();
      const workspace = original.record;
      const candidates = workspace.documents.filter(isMarkdownDocument);
      const metadata = JSON.stringify(candidates.map(d => [d.checkout, d.path, d.description]).sort());
      const cached = this.selection;
      let sources: DocumentIdentity[];
      let cacheHit = false;
      if (!candidates.length) sources = [];
      else if (!forced && cached?.workspaceId === workspace.id && cached.creationToken === workspace.creationToken && cached.metadata === metadata) {
        sources = validatePlanSelection(workspace, cached.sources);
        cacheHit = true;
      } else {
        const decision = await requestPlanSelection(registry, workspace, options, signal, loadHelperProfiles, onRequest);
        await current(original.revision);
        sources = decision.sources;
      }
      // Cache verified metadata choice, not source-read success or an applied goal.
      this.selection = { workspaceId: workspace.id, creationToken: workspace.creationToken, metadata, sources: structuredClone(sources) };
      onDiagnostic?.({ stage: 'selection', outcome: sources.length ? 'selected' : 'no_sources', selectedCount: sources.length, cacheHit });
      stage = 'goal';
      recovery = { baseline: original, sources };
      const captured = await capturePlanSources(workspace, sources).catch(error => { sourceReadFailed = true; throw error; });
      await current(original.revision);
      const previous = workspace.goal;
      if (!sources.length) {
        // Source absence grants no authority to erase established shared purpose.
        if (previous.kind === 'known' && previous.provenance.kind === 'documents') {
          onDiagnostic?.({ stage: 'goal', outcome: 'unchanged' });
          return current(original.revision);
        }
        const origin = { sessionId: authority.sessionId, digest: createHash('sha256').update(JSON.stringify({
          compactionSummary: evidence.compactionSummary, transcript: evidence.transcript,
        })).digest('hex') };
        const cachedConversation = this.retainedConversation;
        const unchanged = !forced && (
          previous.kind !== 'unevaluated' && previous.provenance.kind === 'conversation'
            && (previous.kind === 'unknown' || !!previous.short?.trim())
            && previous.provenance.sessionId === origin.sessionId && previous.provenance.digest === origin.digest
          || cachedConversation?.workspaceId === workspace.id && cachedConversation.creationToken === workspace.creationToken
            && cachedConversation.sessionId === origin.sessionId && cachedConversation.digest === origin.digest
            && isDeepStrictEqual(cachedConversation.goal, previous));
        const proposal = unchanged ? null
          : await requestConversationGoal(registry, workspace, evidence, { ...options, intent }, signal, loadHelperProfiles, onRequest);
        const checked = await current(original.revision);
        if (unchanged || !proposal) {
          onDiagnostic?.({ stage: 'goal', outcome: previous.kind === 'known' || unchanged ? 'unchanged' : 'no_sources' });
          return checked;
        }
        // An empty proposal from sparse/new-agent context is not shared deletion authority, even on reset.
        const retain = previous.kind === 'known' && !proposal.goal.trim();
        const result = await store.applyPlan(authority, {
          goal: retain ? previous : conversationGoal(proposal.goal, origin, proposal.goalShort),
          ...(!retain && intent === 'reset' ? { preserveShort: false } : {}),
          ...(!retain && proposal.title !== undefined ? { title: proposal.title } : {}),
        }, original.revision, signal);
        await publishedTitle(result, retain ? undefined : proposal.title);
        // Only retention needs a host-local cache; accepted results use their durable provenance.
        this.retainedConversation = retain
          ? { workspaceId: workspace.id, creationToken: workspace.creationToken, ...origin, goal: result.record.goal } : undefined;
        onDiagnostic?.({ stage: 'goal', outcome: result.revision === checked.revision ? 'unchanged' : 'applied' });
        return result;
      }
      const unchanged = !forced && previous.kind !== 'unevaluated' && (previous.kind === 'unknown' || !!previous.short?.trim())
        && previous.provenance.kind === 'documents' && isDeepStrictEqual(sources, previous.provenance.sources) && captured.fingerprint === previous.provenance.fingerprint;
      const goal = unchanged ? null
        : await requestWorkspaceGoal(registry, captured, { ...options, intent }, signal, loadHelperProfiles, onRequest);

      const latest = await current(original.revision);
      await recheckPlanSources(latest.record, sources, captured).catch(error => { sourceReadFailed = true; throw error; });
      // Filesystem preparation must finish before the final host guard and CAS effect.
      const checked = await current(original.revision);
      if (unchanged) {
        onDiagnostic?.({ stage: 'goal', outcome: 'unchanged' });
        return checked;
      }
      const result = await store.applyPlan(authority, {
        goal: evaluatedGoal(goal!.goal, sources, captured.fingerprint, goal!.goalShort),
        ...(intent === 'reset' ? { preserveShort: false } : {}),
        ...(goal?.title !== undefined ? { title: goal.title } : {}),
      }, original.revision, signal);
      await publishedTitle(result, goal?.title);
      onDiagnostic?.({ stage: 'goal', outcome: result.revision === checked.revision ? 'unchanged' : 'applied' });
      return result;
    } catch (error) {
      if (sourceReadFailed && recovery) {
        try {
          // Repair only a failed source-read snapshot, never recover through stale authority or retry inference.
          await current(recovery.baseline.revision);
          const probes = await Promise.all(recovery.sources.map(identity => probeWorkspaceDocument(recovery!.baseline.record, identity)));
          await current(recovery.baseline.revision);
          const missing = probes.filter((p): p is MissingDocument => p.kind === 'missing');
          let removed = 0;
          if (missing.length) {
            const result = await store.repairMissingDocuments(authority, recovery.baseline, missing, { signal, assertCurrent });
            removed = result.removed;
            probes.push(...result.unresolved);
          }
          reportDocumentProbes(onDiagnostic, 'documents_checked', probes, removed);
        } catch {
          // Preserve the original read error when guarded recovery cannot apply.
        }
      }
      onDiagnostic?.({ stage, outcome: 'failed', cancelled: signal.aborted });
      if (stage === 'selection') onDiagnostic?.({ stage: 'goal', outcome: 'skipped' });
      throw error;
    }
  }
}
