import { isDeepStrictEqual } from 'node:util';
import type { WorkCardsConfig } from '../config.js';
import { agentAuthority, type Membership, type NativeContext, type WorkspaceClient } from '../herdr/workspace-client.js';
import { pairKeys } from '../herdr/workspace-ownership.js';
import { type AgentRecord, type Authority, type RecordStore, type ReferenceBoundary, type Snapshot } from '../records.js';
import type { ProjectedEvidence } from './evidence.js';
import { captureReferenceBoundary, projectSharedEvidence } from './shared-evidence.js';

export interface WorkspaceSession {
  sessionId: string; sessionFile?: string; cwd: string; evidence: ProjectedEvidence;
}
export interface WorkspaceLifecycleOptions {
  store: RecordStore; client: WorkspaceClient; config: WorkCardsConfig;
  policy: 'automatic' | 'manual'; current: () => WorkspaceSession;
  /** Synchronous notification: the host cancels its own helper work here. */
  onInvalidate?: () => void;
}
export interface WorkspaceScope {
  readonly generation: number;
  readonly signal: AbortSignal;
  readonly authority: Readonly<Authority>;
  readonly context: Readonly<NativeContext>;
}
export interface WorkspaceLifecycleState {
  readonly kind: 'unattached' | 'restored' | 'active' | 'suspended' | 'legacy' | 'disposed';
  readonly agent?: Snapshot<AgentRecord>;
}
interface Ticket { generation: number; signal: AbortSignal; sessionId: string }
interface CleanupTarget { context: NativeContext; agent: Snapshot<AgentRecord> }

function excerpt(text: string): string {
  return Array.from(text.replace(/[\p{Cc}\p{Cf}]/gu, ' ').replace(/\s+/gu, ' ').trim()).slice(0, 80).join('');
}

/** One session, host-driven and unwired: no SDK handlers, scheduler, timers or helper requests. */
export class WorkspaceLifecycle {
  private generation = 0;
  private abort = new AbortController();
  private tail: Promise<unknown> = Promise.resolve();
  private sessionId?: string;
  private baseline?: Readonly<ReferenceBoundary>;
  private context?: NativeContext;
  private membership?: Membership;
  private owned?: CleanupTarget;
  private retiring?: CleanupTarget;
  private legacy = false;
  private retired = false;
  private pendingReset = false;
  private disposed = false;
  private value: WorkspaceLifecycleState = { kind: 'unattached' };

  constructor(private readonly host: WorkspaceLifecycleOptions) {}
  get state(): WorkspaceLifecycleState { return this.value; }

  invalidate(): void {
    this.abort.abort();
    this.abort = new AbortController();
    this.generation++;
    this.membership = undefined;
    if (!this.disposed && !this.legacy) this.value = { kind: this.value.agent ? 'suspended' : 'unattached', agent: this.value.agent };
    this.host.onInvalidate?.();
  }
  dispose(): void {
    this.disposed = true;
    this.invalidate();
    this.abort.abort();
    this.value = { kind: 'disposed', agent: this.value.agent };
  }
  private ticket(): Ticket {
    return { generation: this.generation, signal: this.abort.signal, sessionId: this.host.current().sessionId };
  }
  private check(t: Ticket): void {
    t.signal.throwIfAborted();
    if (this.disposed || t.generation !== this.generation) throw new Error('Stale lifecycle generation');
    if (this.host.current().sessionId !== t.sessionId) {
      this.invalidate();
      throw new Error('Session changed; restore before effects');
    }
  }
  private renew(t: Ticket): void { this.invalidate(); Object.assign(t, this.ticket()); }
  private enqueue<T>(operation: (t: Ticket) => Promise<T>): Promise<T> {
    const t = this.ticket();
    const result = this.tail.catch(() => {}).then(() => { this.check(t); return operation(t); });
    this.tail = result;
    return result;
  }
  private async readAgent(sessionId: string): Promise<Snapshot<AgentRecord> | undefined> {
    try { return await this.host.store.readAgent(sessionId); }
    catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; return undefined; }
  }
  private async resetAgent(t: Ticket, agent: Snapshot<AgentRecord>, boundary: Readonly<ReferenceBoundary>): Promise<Snapshot<AgentRecord>> {
    const updated = await this.host.store.resetEvidence(agentAuthority(agent.record), boundary, agent.revision, t.signal); this.check(t);
    if (this.owned?.agent.record.sessionId === t.sessionId) this.owned = { ...this.owned, agent: updated };
    return updated;
  }
  private async release(t: Ticket): Promise<void> {
    if (!this.retiring) return;
    const target = this.retiring;
    // A cancelled commit may have advanced this session's membership, evidence or display attempt.
    // Refresh only the captured UUID; the adapter still proves its exact live owner and current nonce.
    const agent = await this.host.store.readAgent(target.agent.record.sessionId); this.check(t);
    const live = await this.host.client.observe(target.context, this.host.config); this.check(t);
    await this.host.client.releaseDisplay(this.host.store, live.context, agent, this.host.config, t.signal); this.check(t);
    this.retiring = undefined;
    if (this.owned === target) this.owned = undefined;
  }
  /** Successful-switch cleanup only. Reload/cancellable before-events use invalidate/dispose instead. */
  retire(): Promise<void> {
    this.retired = true;
    this.retiring ??= this.owned;
    this.invalidate();
    return this.enqueue(t => this.release(t));
  }
  /** Call only for a committed transition, or reactivation after a cancelled before-event. */
  restore(mode: 'records' | 'legacy' = 'records'): Promise<WorkspaceLifecycleState> {
    const current = this.host.current();
    const boundary = captureReferenceBoundary(current.evidence);
    if (this.sessionId !== undefined && this.sessionId !== current.sessionId) this.retiring ??= this.owned;
    this.invalidate();
    this.sessionId = current.sessionId;
    this.baseline = boundary;
    this.legacy = mode === 'legacy';
    this.retired = false;
    this.pendingReset = true;
    this.value = { kind: this.legacy ? 'legacy' : 'unattached' };
    return this.enqueue(async t => {
      if (this.legacy) return this.value;
      await this.release(t); this.check(t);
      let agent = await this.readAgent(t.sessionId); this.check(t);
      if (agent) agent = await this.resetAgent(t, agent, boundary);
      this.pendingReset = false;
      this.value = { kind: agent ? 'restored' : 'unattached', agent };
      return this.value;
    });
  }
  reset(): Promise<WorkspaceLifecycleState> { return this.restore(this.legacy ? 'legacy' : 'records'); }

  reconcile(): Promise<WorkspaceLifecycleState> { return this.enqueue(t => this.reconcileCurrent(t)); }
  open(title?: string): Promise<WorkspaceLifecycleState> { return this.enqueue(t => this.reconcileCurrent(t, { title })); }

  private async reconcileCurrent(t: Ticket, explicit?: { title?: string }): Promise<WorkspaceLifecycleState> {
    const { store, client, config, policy } = this.host;
    if (this.legacy) return this.value;
    if (this.sessionId !== t.sessionId) this.invalidate();
    if (this.retired || this.sessionId !== t.sessionId || !this.baseline) throw new Error('Restore required before reconciliation');
    try {
      await this.release(t); this.check(t);
      let agent = await this.readAgent(t.sessionId); this.check(t);
      let projection = this.host.current();
      if (this.pendingReset) {
        if (agent) agent = await this.resetAgent(t, agent, captureReferenceBoundary(projection.evidence));
        this.pendingReset = false;
      }
      const boundary = agent?.record.boundary ?? this.baseline;
      const shared = projectSharedEvidence(projection.evidence, boundary, 'continue');
      if (shared.kind === 'reset-needed') {
        this.renew(t);
        this.baseline = shared.boundary;
        if (agent) agent = await this.resetAgent(t, agent, shared.boundary);
      }
      const fresh = shared.kind === 'suffix' ? shared.evidence.transcript.find(m => m.role === 'user' && excerpt(m.text)) : undefined;
      const title = explicit?.title === undefined ? (agent?.record.title ?? (fresh ? excerpt(fresh.text) : '')) : excerpt(explicit.title);
      if (!agent && !explicit && (policy === 'manual' || !fresh)) return this.value;
      if (!title) throw new Error('A nonblank title or fresh user dialogue is required');
      const previous = this.context;
      if (!this.context) { this.context = await client.bootstrap(config); this.check(t); }
      const observed = await client.observe(this.context, config); this.check(t);
      const context = observed.context;
      if (!explicit && this.context.workspaceId === context.workspaceId && this.context.bindingNonce && this.context.bindingNonce !== context.bindingNonce) throw new Error('Native binding lifetime changed; explicit recovery required');
      const changed = previous && !isDeepStrictEqual(previous, context);
      const binding = observed.snapshot.workspaces.find(w => w.workspace_id === context.workspaceId)?.tokens;
      const needsJoin = !agent || binding?.wcw_id !== agent.record.workspaceId;
      const finishReservation = !needsJoin && binding?.wcw_phase === 'reserved';
      if (changed || (agent && needsJoin)) this.renew(t);
      if (agent && policy === 'manual' && !explicit && needsJoin) {
        this.value = { kind: 'suspended', agent };
        return this.value;
      }
      if (agent) {
        const ownership = await client.ownership(store, context, agent, config, t.signal); this.check(t);
        if (ownership.kind !== 'empty' && ownership.agent.terminal_id !== context.terminalId) throw new Error('Foreign agent caller');
      } else if (pairKeys.some(key => Object.hasOwn(observed.caller.tokens, key))) {
        throw new Error('Unknown owner markers; refusing to adopt another session');
      }
      if (needsJoin || finishReservation) {
        // Completing matching reserved membership is recovery, not a new manual-policy join.
        // The adapter revalidates the complete binding tuple before making it ready.
        // Sample after live observation, not when the poll was queued. Initial creation alone uses START.
        projection = this.host.current();
        const joined = await client.joinWorkspace(store, context, {
          sessionId: t.sessionId, ...(projection.sessionFile === undefined ? {} : { sessionFile: projection.sessionFile }),
          cwd: projection.cwd, title, boundary: agent ? captureReferenceBoundary(projection.evidence) : this.baseline,
          ...(agent ? { expected: agentAuthority(agent.record) } : {}),
        }, config, t.signal); this.check(t);
        agent = joined.agent;
        this.value = { kind: 'restored', agent };
      }
      let verified = await client.verifyMembership(store, context, agent!, config, t.signal); this.check(t);
      this.owned = { agent: verified.agent, context: verified.context };
      if (explicit || policy === 'automatic') {
        const display = verified.ownership.kind === 'separated'
          ? await client.replaceSeparatedDisplay(store, verified.context, verified.agent, config, t.signal)
          : await client.openDisplay(store, verified.context, verified.agent, explicit ? 'explicit' : 'automatic', config, t.signal);
        this.check(t);
        verified = await client.verifyMembership(store, verified.context, display.agent, config, t.signal); this.check(t);
      }
      this.context = verified.context;
      this.owned = { agent: verified.agent, context: verified.context };
      this.membership = verified;
      this.value = { kind: 'active', agent: verified.agent };
      return this.value;
    } catch (error) {
      if (!t.signal.aborted && t.generation === this.generation) {
        // Suspend before fallible recovery. A partial launch may already have persisted its attempt.
        this.renew(t);
        this.value = { kind: 'suspended', agent: this.value.agent };
        try {
          const agent = await this.readAgent(t.sessionId); this.check(t);
          if (this.owned?.agent.record.sessionId === t.sessionId && agent) this.owned = { ...this.owned, agent };
          this.value = { kind: 'suspended', agent };
        } catch { /* Retain the suspended snapshot and report the original failure. */ }
      }
      throw error;
    }
  }
  captureScope(): WorkspaceScope | undefined {
    if (!this.membership || this.value.kind !== 'active' || this.host.current().sessionId !== this.sessionId) return undefined;
    return Object.freeze({ generation: this.generation, signal: this.abort.signal,
      authority: Object.freeze(agentAuthority(this.membership.agent.record)), context: Object.freeze({ ...this.membership.context }) });
  }
  /** Recheck immediately before future effects; this is not a lock across host/model work. */
  assertCurrent(scope: WorkspaceScope): Promise<Membership> {
    return this.enqueue(async t => {
      try {
        if (!scope || scope.authority.sessionId !== t.sessionId || scope.signal.aborted || scope.generation !== t.generation || !this.membership ||
            !isDeepStrictEqual(scope.authority, agentAuthority(this.membership.agent.record)) ||
            !isDeepStrictEqual(scope.context, this.membership.context)) throw new Error('Stale summary scope');
        if (projectSharedEvidence(this.host.current().evidence, this.membership.agent.record.boundary, 'continue').kind !== 'suffix') throw new Error('Evidence reset required');
        const verified = await this.host.client.verifyMembership(this.host.store, scope.context, this.membership.agent, this.host.config, t.signal); this.check(t);
        this.membership = verified;
        this.owned = { context: verified.context, agent: verified.agent };
        this.value = { kind: 'active', agent: verified.agent };
        return verified;
      } catch (error) {
        if (!t.signal.aborted && scope?.generation === this.generation) this.invalidate();
        throw error;
      }
    });
  }
}
