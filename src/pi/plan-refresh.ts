import { isDeepStrictEqual } from 'node:util';
import {
  validateMembership, type Authority, type DocumentIdentity, type RecordStore,
  type Snapshot, type WorkspaceRecord,
} from '../records.js';
import { capturePlanSources, isMarkdownDocument, recheckPlanSources, validatePlanSelection } from './plan-sources.js';
import type { SummaryRegistry, SummaryRequestOptions } from './summarizer.js';
import { requestPlanSelection, requestWorkspaceGoal } from './workspace-summary.js';

export interface PlanRefreshInput {
  authority: Authority;
  signal: AbortSignal;
  /** Reject when the host generation or live context no longer matches this request. */
  assertCurrent: () => Promise<void>;
  /** Reselect metadata and regenerate the goal even when captured provenance is unchanged. */
  force?: boolean;
  onRequest?: () => (() => void);
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
    const { signal, assertCurrent, onRequest } = input;
    const current = async (expectedRevision?: string): Promise<Snapshot<WorkspaceRecord>> => {
      signal.throwIfAborted();
      const agent = await store.readAgent(authority.sessionId);
      if (agent.record.workspaceId !== authority.workspaceId || agent.record.membershipToken !== authority.membershipToken) throw new Error('Stale workspace membership');
      if (agent.record.evidenceGeneration !== authority.evidenceGeneration) throw new Error('Stale evidence generation');
      const workspace = await store.readWorkspace(authority.workspaceId);
      validateMembership(workspace.record, agent.record);
      if (expectedRevision !== undefined && workspace.revision !== expectedRevision) throw new Error('Stale workspace revision during plan refresh');
      await assertCurrent();
      signal.throwIfAborted();
      return workspace;
    };

    const original = await current();
    const workspace = original.record;
    const candidates = workspace.documents.filter(isMarkdownDocument);
    const metadata = JSON.stringify(candidates.map(d => [d.checkout, d.path, d.description]).sort());
    const cached = this.selection;
    let sources: DocumentIdentity[];
    if (!candidates.length) sources = [];
    else if (!input.force && cached?.workspaceId === workspace.id && cached.creationToken === workspace.creationToken && cached.metadata === metadata) {
      sources = validatePlanSelection(workspace, cached.sources);
    } else {
      const decision = await requestPlanSelection(registry, workspace, options, signal, loadHelperProfiles, onRequest);
      await current(original.revision);
      sources = decision.sources;
    }
    // Cache verified metadata choice, not source-read success or an applied goal.
    this.selection = { workspaceId: workspace.id, creationToken: workspace.creationToken, metadata, sources: structuredClone(sources) };
    const captured = await capturePlanSources(workspace, sources);
    await current(original.revision);
    const unchanged = sources.length
      ? !input.force && isDeepStrictEqual(sources, workspace.planSources) && captured.fingerprint === workspace.sourceFingerprint
      : workspace.goal === '' && workspace.planSources.length === 0 && workspace.sourceFingerprint === null;
    const goal = unchanged || !sources.length ? null
      : await requestWorkspaceGoal(registry, captured, options, signal, loadHelperProfiles, onRequest);

    const latest = await current(original.revision);
    await recheckPlanSources(latest.record, sources, captured);
    // Filesystem preparation must finish before the final host guard and CAS effect.
    const checked = await current(original.revision);
    if (unchanged) return checked;
    return store.applyPlan(authority, {
      goal: goal?.goal ?? '', sources, fingerprint: sources.length ? captured.fingerprint : null,
      ...(goal?.title !== undefined ? { title: goal.title } : {}),
    }, original.revision, signal);
  }
}
