import {
  validateMembership, type AgentRecord, type RecordStore, type Snapshot, type WorkspaceRecord,
} from '../records.js';
import type { ProjectedEvidence } from './evidence.js';
import { PlanRefresher, type PlanRefreshInput } from './plan-refresh.js';
import { projectSharedEvidence } from './shared-evidence.js';
import type { SummaryRegistry, SummaryRequestOptions } from './summarizer.js';
import { requestOwnSummary, requestSharedAdditions } from './workspace-summary.js';

export interface WorkspaceRefreshInput extends PlanRefreshInput {
  evidence: ProjectedEvidence;
}
export interface WorkspaceRefreshResult {
  agent: Snapshot<AgentRecord>;
  workspace: Snapshot<WorkspaceRecord>;
}

/** One sequential pass under host single-flight. Discard on host invalidation. */
export class WorkspaceRefresher<TModel extends { api?: string }> {
  private readonly plans: PlanRefresher<TModel>;

  constructor(private readonly config: {
    store: RecordStore;
    registry: SummaryRegistry<TModel>;
    options: SummaryRequestOptions;
    loadHelperProfiles?: () => Promise<unknown>;
  }) {
    this.plans = new PlanRefresher(config);
  }

  async refresh(input: WorkspaceRefreshInput): Promise<WorkspaceRefreshResult> {
    const { store, registry, options, loadHelperProfiles } = this.config;
    const authority = { ...input.authority };
    const evidence = structuredClone(input.evidence);
    const { signal, assertCurrent, force, onRequest, onDiagnostic } = input;
    const current = async (): Promise<WorkspaceRefreshResult> => {
      signal.throwIfAborted();
      const agent = await store.readAgent(authority.sessionId);
      if (agent.record.workspaceId !== authority.workspaceId || agent.record.membershipToken !== authority.membershipToken) throw new Error('Stale workspace membership');
      if (agent.record.evidenceGeneration !== authority.evidenceGeneration) throw new Error('Stale evidence generation');
      const workspace = await store.readWorkspace(authority.workspaceId);
      validateMembership(workspace.record, agent.record);
      await assertCurrent();
      signal.throwIfAborted();
      return { agent, workspace };
    };

    let stage: 'own' | 'shared' = 'own';
    try {
      const original = await current();
      if (projectSharedEvidence(evidence, original.agent.record.boundary, 'continue').kind === 'reset-needed') throw new Error('Evidence boundary needs reset by host');
      const own = await requestOwnSummary(registry, original.agent.record, evidence, original.workspace.record, options, signal, loadHelperProfiles, onRequest);
      await current();
      let ownChanged = false;
      if (own.action === 'update') {
        // Never replace the original whole-agent CAS revision with a later read.
        const result = await store.updateOwn(authority, own.patch, original.agent.revision, signal);
        ownChanged = result.revision !== original.agent.revision;
      }
      onDiagnostic?.({ stage: 'own', outcome: ownChanged ? 'applied' : 'unchanged' });

      if (evidence.compactionSummary?.trim() || evidence.transcript.length) {
        stage = 'shared';
        const latest = await current();
        const additions = await requestSharedAdditions(registry, latest.workspace.record, latest.agent.record.checkout, evidence, options, signal, loadHelperProfiles, onRequest);
        await current();
        let changed = false;
        if (additions.action === 'update') {
          // Merge against current references; foreground exclusions remain authoritative.
          const { action: _, ...proposal } = additions;
          const result = await store.updateSharedReferences(authority, latest.workspace, proposal, signal);
          changed = result.changed;
        }
        onDiagnostic?.({ stage: 'shared', outcome: changed ? 'applied' : 'unchanged' });
      }
    } catch (error) {
      onDiagnostic?.({ stage, outcome: 'failed', cancelled: signal.aborted });
      onDiagnostic?.({ stage: 'goal', outcome: 'skipped' });
      throw error;
    }

    await this.plans.refresh({ authority, signal, assertCurrent, force, onRequest, onDiagnostic });
    try { return await current(); }
    catch (error) {
      onDiagnostic?.({ stage: 'goal', outcome: 'failed', cancelled: signal.aborted });
      throw error;
    }
  }
}
