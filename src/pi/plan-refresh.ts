import { isDeepStrictEqual } from 'node:util';
import {
  type Authority, type DocumentIdentity, type RecordStore,
  type Snapshot, type WorkspaceRecord,
} from '../records.js';
import { capturePlanSources, isMarkdownDocument, recheckPlanSources, validatePlanSelection } from './plan-sources.js';
import type { SummaryRegistry, SummaryRequestOptions } from './summarizer.js';
import type { CheckIntent } from './scheduler.js';
import { requestPlanSelection, requestWorkspaceGoal } from './workspace-summary.js';

export type RefreshDiagnostic =
  | { stage: 'own' | 'shared' | 'goal'; outcome: 'applied' | 'unchanged' | 'no_sources' | 'skipped' }
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
      const captured = await capturePlanSources(workspace, sources);
      await current(original.revision);
      const unchanged = sources.length
        ? !forced && (!workspace.goal.trim() || !!workspace.goalShort?.trim()) && isDeepStrictEqual(sources, workspace.planSources) && captured.fingerprint === workspace.sourceFingerprint
        : workspace.goal === '' && workspace.planSources.length === 0 && workspace.sourceFingerprint === null;
      const goal = unchanged || !sources.length ? null
        : await requestWorkspaceGoal(registry, captured, { ...options, intent }, signal, loadHelperProfiles, onRequest);

      const latest = await current(original.revision);
      await recheckPlanSources(latest.record, sources, captured);
      // Filesystem preparation must finish before the final host guard and CAS effect.
      const checked = await current(original.revision);
      if (unchanged) {
        onDiagnostic?.({ stage: 'goal', outcome: sources.length ? 'unchanged' : 'no_sources' });
        return checked;
      }
      const result = await store.applyPlan(authority, {
        goal: goal?.goal ?? '', sources, fingerprint: sources.length ? captured.fingerprint : null,
        ...(goal?.title !== undefined ? { title: goal.title } : {}),
        ...(goal?.goalShort !== undefined ? { goalShort: goal.goalShort } : {}),
      }, original.revision, signal);
      onDiagnostic?.({ stage: 'goal', outcome: !sources.length ? 'no_sources' : result.revision === checked.revision ? 'unchanged' : 'applied' });
      return result;
    } catch (error) {
      onDiagnostic?.({ stage, outcome: 'failed', cancelled: signal.aborted });
      if (stage === 'selection') onDiagnostic?.({ stage: 'goal', outcome: 'skipped' });
      throw error;
    }
  }
}
