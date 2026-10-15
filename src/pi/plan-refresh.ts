import { isDeepStrictEqual } from 'node:util';
import {
  type Authority, type DocumentIdentity, type RecordStore,
  type Snapshot, type WorkspaceRecord, evaluatedGoal, unevaluatedGoal,
  probeWorkspaceDocument, type DocumentProbe, type DocumentProbeReason, type MissingDocument,
} from '../records.js';
import { capturePlanSources, isMarkdownDocument, recheckPlanSources, validatePlanSelection } from './plan-sources.js';
import type { SummaryRegistry, SummaryRequestOptions } from './summarizer.js';
import type { CheckIntent } from './scheduler.js';
import { requestPlanSelection, requestWorkspaceGoal } from './workspace-summary.js';

export type RefreshDiagnostic =
  | { stage: 'own' | 'shared' | 'goal'; outcome: 'applied' | 'unchanged' | 'no_sources' | 'skipped' }
  | { stage: 'shared'; outcome: 'documents_checked' | 'documents_omitted'; missingCount: number; removedCount: number; unresolvedCounts: Partial<Record<DocumentProbeReason, number>> }
  | { stage: 'selection'; outcome: 'selected' | 'no_sources'; selectedCount: number; cacheHit: boolean }
  | { stage: 'own' | 'shared' | 'selection' | 'goal'; outcome: 'failed'; cancelled: boolean };

export interface PlanRefreshInput {
  authority: Authority;
  signal: AbortSignal;
  /** Reject when the host generation or live context no longer matches this request. */
  assertCurrent: () => Promise<void>;
  /** Explicit requests reselect metadata and regenerate Goal even with unchanged provenance. */
  intent?: CheckIntent;
  onRequest?: () => (() => void);
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
      const unchanged = sources.length
        ? !forced && previous.kind !== 'unevaluated' && (previous.kind === 'unknown' || !!previous.short?.trim())
          && isDeepStrictEqual(sources, previous.sources) && captured.fingerprint === previous.fingerprint
        : previous.kind === 'unevaluated';
      const goal = unchanged || !sources.length ? null
        : await requestWorkspaceGoal(registry, captured, { ...options, intent }, signal, loadHelperProfiles, onRequest);

      const latest = await current(original.revision);
      await recheckPlanSources(latest.record, sources, captured).catch(error => { sourceReadFailed = true; throw error; });
      // Filesystem preparation must finish before the final host guard and CAS effect.
      const checked = await current(original.revision);
      if (unchanged) {
        onDiagnostic?.({ stage: 'goal', outcome: sources.length ? 'unchanged' : 'no_sources' });
        return checked;
      }
      const result = await store.applyPlan(authority, {
        goal: sources.length ? evaluatedGoal(goal!.goal, sources, captured.fingerprint, goal!.goalShort) : unevaluatedGoal(),
        ...(goal?.title !== undefined ? { title: goal.title } : {}),
      }, original.revision, signal);
      onDiagnostic?.({ stage: 'goal', outcome: !sources.length ? 'no_sources' : result.revision === checked.revision ? 'unchanged' : 'applied' });
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
