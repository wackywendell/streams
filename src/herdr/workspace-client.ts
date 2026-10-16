import { createHash, randomUUID } from 'node:crypto';
import { mkdir, realpath } from 'node:fs/promises';
import { isDeepStrictEqual } from 'node:util';
import { configPath, documentCommand, type WorkCardsConfig } from '../config.js';
import { resolvedReferences } from '../presentation.js';
import { RecordStore, agentAuthority, goalText, goalShort, sameAgentMembership, resolveWorkspaceDocument, documentIdentity, referenceIdentity, validateMembership, type AgentRecord, type DocumentIdentity, type JoinInput, type Snapshot, type WorkspaceRecord } from '../records.js';
import { withWorkspaceLock } from './workspace-lock.js';
import { NameStore, decideName, type NameOwner } from './name-provenance.js';
import type { Pane, Workspace } from './workspace-ownership.js';
import { HerdrTransport, type ClientOptions } from './transport.js';
import { classifyPair, firstPane, locateTerminal, nativeObject, nativePane, pairKeys, pairMarkers, parseNativeSnapshot, type LivePair, type NativeSnapshot, type PairState } from './workspace-ownership.js';

/** Capture once with bootstrap; observe follows terminals. Effects reject a moved painted context. */
export interface NativeContext { socket: string; terminalId: string; workspaceId: string; tabId: string; bindingNonce?: string }
export interface WorkspaceBinding { id: string; store: string; token: string; nonce: string; phase: 'reserved' | 'ready' }
export interface Membership { workspace: Snapshot<WorkspaceRecord>; agent: Snapshot<AgentRecord>; binding: WorkspaceBinding; context: NativeContext }
export type ReadyWorkspaceBinding = Omit<WorkspaceBinding, 'phase'> & { phase: 'ready' };
export interface ReadyMembership extends Membership {
  binding: ReadyWorkspaceBinding;
  context: NativeContext & { bindingNonce: string };
  ownership: PairState;
}
export type NativeNamingResult = { workspace: 'renamed' | 'unchanged' | 'manual'; tab: 'renamed' | 'unchanged' | 'manual' | 'not-first' | 'not-ready' };
type NativeNameRead = (NativeNameAuthority & { kind: 'ready' }) | { kind: 'skip'; reason: 'not-first' | 'not-ready' };
interface NativeNameAuthority {
  bindingNonce: string; surfaceId: string; owner: NameOwner; label: string; desired: string;
  defaultLabelProven: boolean; proof: { number: number } & Record<string, unknown>;
}
export type DisplayResult = { kind: 'paired'; pair: LivePair; agent: Snapshot<AgentRecord> } | { kind: 'suppressed'; ownership: PairState; agent: Snapshot<AgentRecord> };
export type WorkspaceJoinInput = Omit<JoinInput, 'workspaceId' | 'membershipToken'> & { targetWorkspaceId?: string };
const bindingKeys = ['wcw_id', 'wcw_store', 'wcw_token', 'wcw_nonce', 'wcw_phase'];
const digest = (text: string): string => createHash('sha256').update(text).digest('hex');
const cap = (text: string): string => Array.from(text).slice(0, 80).join('');
function bindingOf(workspace: Workspace, store: string): WorkspaceBinding | undefined {
  const t = workspace.tokens;
  if (Object.keys(t).some(k => k.startsWith('wc_'))) throw new Error('Legacy workspace publisher must be explicitly resolved before joining');
  if (!bindingKeys.some(k => Object.hasOwn(t, k))) {
    if (Object.keys(t).some(k => k.startsWith('wcw_'))) throw new Error('Incomplete workspace namespace; explicit recovery required');
    return undefined;
  }
  if (!bindingKeys.every(k => t[k]) || t.wcw_store !== store || !['reserved', 'ready'].includes(t.wcw_phase) ||
      !/^[A-Za-z0-9_-]{1,80}$/.test(t.wcw_id) || !/^[A-Za-z0-9_-]{1,80}$/.test(t.wcw_token) || !/^[A-Za-z0-9_-]{1,80}$/.test(t.wcw_nonce)) throw new Error('Foreign or incomplete workspace binding; explicit recovery required');
  return { id: t.wcw_id, store, token: t.wcw_token, nonce: t.wcw_nonce, phase: t.wcw_phase as WorkspaceBinding['phase'] };
}
async function maybeAgent(store: RecordStore, session: string): Promise<Snapshot<AgentRecord> | undefined> {
  try { return await store.readAgent(session); } catch (e) { if ((e as NodeJS.ErrnoException).code !== 'ENOENT') throw e; return undefined; }
}

/** Native workspace adapter. Callers must serialize reconciliation and retain cancellation generations. */
export class WorkspaceClient extends HerdrTransport {
  constructor(options: ClientOptions = {}) { super(options); }
  async snapshot(config: WorkCardsConfig): Promise<NativeSnapshot> {
    return parseNativeSnapshot((await this.result(['api', 'snapshot'], config, 'session_snapshot')).snapshot);
  }
  async bootstrap(config: WorkCardsConfig): Promise<NativeContext> {
    const pane = await this.current(config), socket = await this.socket(config);
    const snapshot = await this.snapshot(config), live = locateTerminal(snapshot, pane.terminal_id);
    if (live.pane_id !== pane.pane_id || live.workspace_id !== pane.workspace_id || live.tab_id !== pane.tab_id) throw new Error('Caller moved during native bootstrap');
    const bindingNonce = snapshot.workspaces.find(w => w.workspace_id === live.workspace_id)?.tokens.wcw_nonce;
    return { socket, terminalId: live.terminal_id, workspaceId: live.workspace_id, tabId: live.tab_id, ...(bindingNonce ? { bindingNonce } : {}) };
  }
  async observe(context: NativeContext, config: WorkCardsConfig): Promise<{ context: NativeContext; snapshot: NativeSnapshot; caller: Pane }> {
    if (await this.socket(config) !== context.socket) throw new Error('Native socket changed');
    const snapshot = await this.snapshot(config), caller = locateTerminal(snapshot, context.terminalId);
    const bindingNonce = snapshot.workspaces.find(w => w.workspace_id === caller.workspace_id)?.tokens.wcw_nonce;
    return { context: { socket: context.socket, terminalId: context.terminalId, workspaceId: caller.workspace_id, tabId: caller.tab_id, ...(bindingNonce ? { bindingNonce } : {}) }, snapshot, caller };
  }
  private async guard(context: NativeContext, config: WorkCardsConfig, signal?: AbortSignal) {
    signal?.throwIfAborted();
    const live = await this.observe(context, config);
    signal?.throwIfAborted();
    if (live.context.workspaceId !== context.workspaceId || live.context.tabId !== context.tabId) throw new Error('Stale native context: caller moved; reconcile before effects');
    if (context.bindingNonce && live.context.bindingNonce !== context.bindingNonce) throw new Error('Stale native binding lifetime; reconcile before effects');
    return live;
  }
  private async storeKey(store: RecordStore): Promise<string> { return digest(await realpath(store.directory)); }
  private async identity(store: RecordStore, agent: Snapshot<AgentRecord>): Promise<string> {
    const path = await realpath(store.agentPath(agent.record.sessionId));
    if (await realpath(agent.path) !== path) throw new Error('Foreign agent file identity');
    return digest(path);
  }
  private async currentRecords(store: RecordStore, painted: Snapshot<AgentRecord>): Promise<{ agent: Snapshot<AgentRecord>; workspace: Snapshot<WorkspaceRecord> }> {
    if (await realpath(painted.path) !== await realpath(store.agentPath(painted.record.sessionId))) throw new Error('Foreign agent path');
    const records = await store.readMembership(painted.record.sessionId, agentAuthority(painted.record));
    if (!sameAgentMembership(painted.record, records.agent.record)) throw new Error('Stale workspace membership');
    return records;
  }
  private bound(storeKey: string, workspace: Snapshot<WorkspaceRecord>, native: NativeSnapshot, context: NativeContext): ReadyWorkspaceBinding {
    const w = native.workspaces.find(w => w.workspace_id === context.workspaceId);
    if (!w) throw new Error('Missing caller workspace');
    if (native.panes.some(p => p.workspace_id === context.workspaceId && Object.keys(p.tokens).some(k => k.startsWith('wc_')))) throw new Error('Legacy live publisher conflict');
    const binding = bindingOf(w, storeKey);
    if (!binding || binding.phase !== 'ready' || binding.id !== workspace.record.id || binding.token !== workspace.record.creationToken) throw new Error('Stale or foreign ready workspace binding');
    return { ...binding, phase: binding.phase };
  }
  private async stamp(kind: 'pane' | 'workspace', target: Pane | Workspace, values: Record<string, string | null>, context: NativeContext, config: WorkCardsConfig, signal?: AbortSignal, prepare?: () => Promise<(snapshot: NativeSnapshot) => void>): Promise<void> {
    const validate = await prepare?.();
    const live = await this.guard(context, config, signal);
    validate?.(live.snapshot);
    if (Object.keys(values).length > 16) throw new Error('Native metadata patch exceeds 16-token limit');
    const id = 'pane_id' in target ? target.pane_id : target.workspace_id;
    const surface = kind === 'pane' ? live.snapshot.panes.find(p => p.pane_id === id) : live.snapshot.workspaces.find(w => w.workspace_id === id);
    if (!surface || !isDeepStrictEqual(surface, target)) throw new Error('Native metadata target identity/markers changed');
    const merged = { ...surface.tokens };
    for (const [key, value] of Object.entries(values)) {
      if (value === null) delete merged[key];
      else { if (Array.from(value).length > 80 || /[\p{Cc}\p{Cf}]/u.test(value)) throw new Error('Native metadata value exceeds safe 80-character limit'); merged[key] = value; }
    }
    if (Object.keys(merged).length > 32) throw new Error('Native metadata exceeds 32-token resource capacity');
    signal?.throwIfAborted();
    await this.report(kind, id, values, config);
  }
  async joinWorkspace(store: RecordStore, context: NativeContext, input: WorkspaceJoinInput, config: WorkCardsConfig, signal?: AbortSignal): Promise<Membership> {
    signal?.throwIfAborted();
    await mkdir(store.directory, { recursive: true, mode: 0o700 });
    const key = await this.storeKey(store);
    const initial = await this.guard(context, config, signal);
    const target = input.targetWorkspaceId ?? bindingOf(initial.snapshot.workspaces.find(w => w.workspace_id === context.workspaceId)!, key)?.id;
    // Existing durable targets serialize across native groups, always outside the group lock.
    const join = () => this.joinWorkspaceLocked(store, context, input, config, key, target, signal);
    const result = await (target === undefined ? join() : withWorkspaceLock(context.socket, `durable-workspace\0${key}\0${target}`, join, { waitMs: config.herdr_timeout_ms, signal }));
    if (!('coordinate' in result)) return result;
    // Discovery has no effects: release native before taking the newly discovered durable target.
    const binding = result.coordinate, coordinatedContext = { ...context, bindingNonce: binding.nonce };
    const coordinated = await withWorkspaceLock(context.socket, `durable-workspace\0${key}\0${binding.id}`,
      () => this.joinWorkspaceLocked(store, coordinatedContext, input, config, key, binding.id, signal, binding), { waitMs: config.herdr_timeout_ms, signal });
    if ('coordinate' in coordinated) throw new Error('Workspace binding changed again during target coordination');
    return coordinated;
  }
  private async joinWorkspaceLocked(store: RecordStore, context: NativeContext, input: WorkspaceJoinInput, config: WorkCardsConfig, key: string, target: string | undefined, signal?: AbortSignal, expectedBinding?: WorkspaceBinding): Promise<Membership | { coordinate: WorkspaceBinding }> {
    return withWorkspaceLock(context.socket, context.workspaceId, async () => {
      let mutated = false;
      try {
        const selected = input.targetWorkspaceId === undefined ? undefined : await store.readWorkspace(input.targetWorkspaceId);
        const live = await this.guard(context, config, signal);
        if (live.snapshot.panes.some(p => p.workspace_id === context.workspaceId && Object.keys(p.tokens).some(k => k.startsWith('wc_')))) throw new Error('Legacy live publisher conflict');
        const w = live.snapshot.workspaces.find(w => w.workspace_id === context.workspaceId)!;
        let binding = bindingOf(w, key);
        if (expectedBinding && !isDeepStrictEqual(binding, expectedBinding)) throw new Error('Workspace binding changed again during target coordination');
        if (target === undefined && binding) return { coordinate: binding };
        if (binding && binding.id !== target) throw new Error('Workspace binding changed before durable target coordination; retry explicitly');
        const unique = (snapshot: NativeSnapshot) => {
          if (target && snapshot.workspaces.some(other => other.workspace_id !== context.workspaceId && other.tokens.wcw_id === target)) throw new Error('Durable workspace already has another live native binding');
        };
        unique(live.snapshot);
        if (selected) {
          if (binding && binding.id !== selected.record.id) throw new Error('Native workspace already bound; refusing replacement');
        }
        const newLifetime = !binding;
        if (!binding) {
          binding = { id: selected?.record.id ?? randomUUID(), store: key, token: selected?.record.creationToken ?? randomUUID(), nonce: randomUUID(), phase: 'reserved' };
          mutated = true;
          await this.stamp('workspace', w, { wcw_id: binding.id, wcw_store: binding.store, wcw_token: binding.token, wcw_nonce: binding.nonce, wcw_phase: 'reserved' }, context, config, signal, async () => unique);
          const reserved = await this.guard(context, config, signal);
          if (!isDeepStrictEqual(bindingOf(reserved.snapshot.workspaces.find(w => w.workspace_id === context.workspaceId)!, key), binding)) throw new Error('Reservation did not round-trip');
        }
        let workspace: Snapshot<WorkspaceRecord>;
        if (binding.phase === 'reserved') { mutated = true; workspace = await store.createWorkspace({ id: binding.id, creationToken: binding.token, title: input.title }, signal); }
        else workspace = await store.readWorkspace(binding.id);
        if (workspace.record.creationToken !== binding.token) throw new Error('Foreign workspace creation token');
        await this.guard(context, config, signal);
        let agent = await maybeAgent(store, input.sessionId);
        if (agent?.record.workspaceId === binding.id) {
          if (agent.record.cwd !== await realpath(input.cwd)) throw new Error('Conflicting membership cwd');
          validateMembership(workspace.record, agent.record);
          if (selected && (newLifetime || binding.phase === 'reserved')) {
            mutated = true;
            agent = await store.resetEvidence(agentAuthority(agent.record), input.boundary, agent.revision, signal);
          }
        } else {
          mutated = true;
          const { targetWorkspaceId: _target, ...join } = input;
          agent = await store.join({ ...join, workspaceId: binding.id, membershipToken: randomUUID() }, signal);
        }
        workspace = await store.readWorkspace(binding.id);
        validateMembership(workspace.record, agent.record);
        const again = await this.guard(context, config, signal);
        if (!isDeepStrictEqual(bindingOf(again.snapshot.workspaces.find(w => w.workspace_id === context.workspaceId)!, key), binding)) throw new Error('Workspace reservation changed');
        if (binding.phase === 'reserved') {
          mutated = true;
          await this.stamp('workspace', again.snapshot.workspaces.find(w => w.workspace_id === context.workspaceId)!, { wcw_phase: 'ready' }, context, config, signal, async () => unique);
          binding = { ...binding, phase: 'ready' };
        }
        const final = await this.guard(context, config, signal);
        unique(final.snapshot);
        this.bound(key, workspace, final.snapshot, context);
        return { workspace, agent, binding, context: { ...context, bindingNonce: binding.nonce } };
      } catch (error) {
        if (!mutated) throw error;
        throw new Error(`Workspace join may be partial; reservation and records are retained for verified retry: ${(error as Error).message}`, { cause: error });
      }
    }, { waitMs: config.herdr_timeout_ms, signal });
  }
  async ownership(store: RecordStore, context: NativeContext, painted: Snapshot<AgentRecord>, config: WorkCardsConfig, signal?: AbortSignal): Promise<PairState> {
    const { agent } = await this.currentRecords(store, painted), identity = await this.identity(store, agent);
    const live = await this.guard(context, config, signal);
    const state = classifyPair(live.snapshot, identity, context.terminalId);
    if (state.kind !== 'empty' && state.nonce !== agent.record.displayAttempt?.nonce) throw new Error('Pair nonce conflicts with persisted display attempt');
    return state;
  }
  /** Readonly Pi authority: q-absent is valid membership, never permission to relaunch. */
  async verifyMembership(store: RecordStore, context: NativeContext, painted: Snapshot<AgentRecord>, config: WorkCardsConfig, signal?: AbortSignal): Promise<ReadyMembership> {
    const records = await this.currentRecords(store, painted);
    const identity = await this.identity(store, records.agent), key = await this.storeKey(store);
    const live = await this.guard(context, config, signal);
    const binding = this.bound(key, records.workspace, live.snapshot, context);
    const ownership = classifyPair(live.snapshot, identity, context.terminalId);
    if (ownership.kind !== 'empty' && (ownership.agent.terminal_id !== context.terminalId || ownership.nonce !== records.agent.record.displayAttempt?.nonce)) throw new Error('Only the verified owning Pi agent may use this membership');
    return { ...records, binding, context: { ...context, bindingNonce: binding.nonce }, ownership };
  }
  private async pairedCard(store: RecordStore, context: NativeContext, painted: Snapshot<AgentRecord>, config: WorkCardsConfig, signal?: AbortSignal) {
    const records = await this.currentRecords(store, painted);
    const identity = await this.identity(store, records.agent), key = await this.storeKey(store);
    const live = await this.guard(context, config, signal);
    this.bound(key, records.workspace, live.snapshot, context);
    const state = classifyPair(live.snapshot, identity, context.terminalId);
    if (state.kind !== 'paired' || state.card.terminal_id !== context.terminalId || state.nonce !== this.env.WORK_CARDS_NONCE || state.nonce !== records.agent.record.displayAttempt?.nonce) throw new Error('Only the verified paired card may perform this operation');
    return { ...records, live, pair: state };
  }
  async verifyDisplay(store: RecordStore, context: NativeContext, painted: Snapshot<AgentRecord>, config: WorkCardsConfig, signal?: AbortSignal): Promise<LivePair> {
    return (await this.pairedCard(store, context, painted, config, signal)).pair;
  }
  async openDisplay(store: RecordStore, context: NativeContext, painted: Snapshot<AgentRecord>, mode: 'automatic' | 'explicit', config: WorkCardsConfig, signal?: AbortSignal): Promise<DisplayResult> {
    signal?.throwIfAborted();
    return withWorkspaceLock(context.socket, context.workspaceId, () => this.openDisplayLocked(store, context, painted, mode, config, signal), { waitMs: config.herdr_timeout_ms, signal });
  }
  private async openDisplayLocked(store: RecordStore, context: NativeContext, painted: Snapshot<AgentRecord>, mode: 'automatic' | 'explicit', config: WorkCardsConfig, signal?: AbortSignal): Promise<DisplayResult> {
    let { agent, workspace } = await this.currentRecords(store, painted);
    const storeKey = await this.storeKey(store), live = await this.guard(context, config, signal); this.bound(storeKey, workspace, live.snapshot, context);
    let state = await this.ownership(store, context, agent, config, signal);
    if (state.kind === 'paired') {
      if (state.agent.terminal_id !== context.terminalId) throw new Error('Only the owning agent may open its display');
      return { kind: 'paired', pair: state, agent };
    }
    if (state.kind === 'separated') throw new Error('Verified separated pair requires explicit controller cleanup before replacement');
    if (state.kind === 'absent' && state.agent.terminal_id !== context.terminalId) throw new Error('Foreign display opener');
    if (mode === 'automatic' && agent.record.displayAttempt) return { kind: 'suppressed', ownership: state, agent };
    if (state.kind === 'absent') {
      await this.stamp('pane', state.agent, Object.fromEntries(pairKeys.map(k => [k, null])), context, config, signal);
      state = await this.ownership(store, context, agent, config, signal);
      if (state.kind !== 'empty') throw new Error('Absent-owner cleanup failed to round-trip');
    }
    const nonce = randomUUID(), identity = await this.identity(store, agent);
    await this.guard(context, config, signal);
    agent = await store.setDisplayAttempt(agentAuthority(agent.record), { attempt: randomUUID(), nonce }, agent.revision, signal);
    const attempt = agent.record.displayAttempt;
    const prepare = async () => {
      const records = await this.currentRecords(store, agent);
      const key = await this.storeKey(store);
      if (await this.identity(store, records.agent) !== identity || !isDeepStrictEqual(records.agent.record.displayAttempt, attempt)) throw new Error('Stale display attempt identity');
      return (snapshot: NativeSnapshot) => { this.bound(key, records.workspace, snapshot, context); };
    };
    const validate = async () => {
      const check = await prepare(), current = await this.guard(context, config, signal);
      check(current.snapshot); return current;
    };
    let returned: Pane | undefined;
    try {
      await validate(); await this.ensurePlugin(config, validate);
      const root = await realpath(this.pluginRoot), agentPath = await realpath(agent.path), stateDirectory = await realpath(store.directory);
      const before = await validate();
      if (classifyPair(before.snapshot, identity, context.terminalId).kind !== 'empty') throw new Error('Pair changed before launch');
      signal?.throwIfAborted();
      const opened = await this.result(['plugin', 'pane', 'open', '--plugin', 'work-cards', '--entrypoint', 'card', '--placement', 'split', '--target-pane', before.caller.pane_id, '--direction', 'down', '--cwd', root, '--no-focus',
        '--env', `WORK_CARDS_AGENT=${agentPath}`, '--env', `WORK_CARDS_SESSION_ID=${agent.record.sessionId}`, '--env', `WORK_CARDS_STATE_DIR=${stateDirectory}`, '--env', `WORK_CARDS_OWNER_TERMINAL=${context.terminalId}`, '--env', `WORK_CARDS_NONCE=${nonce}`, '--env', `WORK_CARDS_CONFIG=${configPath(this.env)}`, '--env', `EDITOR=${this.env.EDITOR ?? ''}`, ...(this.env.PATH === undefined ? [] : ['--env', `PATH=${this.env.PATH}`])], config, 'plugin_pane_opened');
      const info = nativeObject(opened.plugin_pane); returned = nativePane(info.pane);
      if (info.plugin_id !== 'work-cards' || info.entrypoint !== 'card' || returned.workspace_id !== context.workspaceId || returned.tab_id !== context.tabId || before.snapshot.panes.some(p => p.pane_id === returned!.pane_id || p.terminal_id === returned!.terminal_id)) throw new Error('Unexpected/reused card pane');
      const after = await validate();
      const card = locateTerminal(after.snapshot, returned.terminal_id);
      if (!isDeepStrictEqual(card, returned) || pairKeys.some(k => Object.hasOwn(card.tokens, k))) throw new Error('Unexpected live card identity/markers');
      const markers = pairMarkers(identity, nonce, context.terminalId, card.terminal_id);
      await this.stamp('pane', card, { ...markers, wca_role: 'card' }, context, config, signal, prepare);
      const current = await validate();
      await this.stamp('pane', current.caller, { ...markers, wca_role: 'agent' }, context, config, signal, prepare);
      const final = await this.ownership(store, context, agent, config, signal);
      if (final.kind !== 'paired') throw new Error('Pair did not round-trip');
      const verifyCreatedPair = async () => {
        const current = await validate(), pair = classifyPair(current.snapshot, identity, context.terminalId);
        if (pair.kind !== 'paired' || pair.nonce !== nonce || pair.agent.terminal_id !== context.terminalId ||
            pair.agent.pane_id !== before.caller.pane_id || pair.card.pane_id !== returned!.pane_id || pair.card.terminal_id !== returned!.terminal_id) throw new Error('Created pair changed before sizing/return');
        return pair;
      };
      const pair = await verifyCreatedPair(), fraction = config.pane_height_fraction;
      if (fraction !== 0.5) {
        // Grow toward the shared edge: agent/down or card/up, never an unrelated outer separator.
        const target = fraction < 0.5 ? pair.agent : pair.card;
        signal?.throwIfAborted();
        try {
          const resized = await this.result(['pane', 'resize', '--pane', target.pane_id, '--direction', fraction < 0.5 ? 'down' : 'up', '--amount', String(Math.abs(0.5 - fraction))], config, 'pane_resize');
          if (nativeObject(resized.resize).pane_id !== target.pane_id) throw new Error('Resized pane identity mismatch');
        } catch {
          // Geometry is best effort; retain the proved pair and its persisted attempt.
          signal?.throwIfAborted();
        }
        return { kind: 'paired', pair: await verifyCreatedPair(), agent };
      }
      return { kind: 'paired', pair, agent };
    } catch (error) {
      throw new Error(`Display launch may be partial${returned ? ` (pane ${returned.pane_id}, terminal ${returned.terminal_id})` : ''}; persisted attempt retained, no automatic retry: ${(error as Error).message}`, { cause: error });
    }
  }
  /** One fresh positive separated proof authorizes one replacement, under the same lock. */
  async replaceSeparatedDisplay(store: RecordStore, context: NativeContext, painted: Snapshot<AgentRecord>, config: WorkCardsConfig, signal?: AbortSignal): Promise<DisplayResult> {
    signal?.throwIfAborted();
    return withWorkspaceLock(context.socket, context.workspaceId, async () => {
      const verified = await this.verifyMembership(store, context, painted, config, signal);
      if (verified.ownership.kind !== 'separated') throw new Error('Replacement requires a positively verified separated pair');
      await this.cleanupDisplayLocked(store, verified.context, verified.agent, true, config, signal);
      // No durable suppression is cleared. Any partial launch retains its new attempt.
      return this.openDisplayLocked(store, verified.context, verified.agent, 'explicit', config, signal);
    }, { waitMs: config.herdr_timeout_ms, signal });
  }
  /** Retire only this known session's own markers; absence never launches a card. */
  async releaseDisplay(store: RecordStore, context: NativeContext, painted: Snapshot<AgentRecord>, config: WorkCardsConfig, signal?: AbortSignal): Promise<void> {
    signal?.throwIfAborted();
    await withWorkspaceLock(context.socket, context.workspaceId, async () => {
      const state = await this.ownership(store, context, painted, config, signal);
      if (state.kind === 'empty') return;
      if (state.agent.terminal_id !== context.terminalId) throw new Error('Foreign release caller');
      if (state.kind === 'paired' || state.kind === 'separated') {
        await this.cleanupDisplayLocked(store, context, painted, state.kind === 'separated', config, signal);
        return;
      }
      const prepare = async () => {
        await this.currentRecords(store, painted);
        return (snapshot: NativeSnapshot) => {
          if (!isDeepStrictEqual(classifyPair(snapshot, state.identity, context.terminalId), state)) throw new Error('Absent pair changed before release');
        };
      };
      await this.stamp('pane', state.agent, Object.fromEntries([...pairKeys, 'wca_status', 'wca_now', 'wca_status_short'].map(k => [k, null])), context, config, signal, prepare);
    }, { waitMs: config.herdr_timeout_ms, signal });
  }
  /** Closes only an exact live owned display. q/absence never authorizes replacement. Shared binding survives. */
  async closeDisplay(store: RecordStore, context: NativeContext, painted: Snapshot<AgentRecord>, config: WorkCardsConfig, signal?: AbortSignal): Promise<void> {
    return this.cleanupDisplay(store, context, painted, false, config, signal);
  }
  async cleanupSeparatedPair(store: RecordStore, context: NativeContext, painted: Snapshot<AgentRecord>, config: WorkCardsConfig, signal?: AbortSignal): Promise<void> {
    return this.cleanupDisplay(store, context, painted, true, config, signal);
  }
  private async cleanupDisplay(store: RecordStore, context: NativeContext, painted: Snapshot<AgentRecord>, separated: boolean, config: WorkCardsConfig, signal?: AbortSignal): Promise<void> {
    signal?.throwIfAborted();
    await withWorkspaceLock(context.socket, context.workspaceId, () => this.cleanupDisplayLocked(store, context, painted, separated, config, signal), { waitMs: config.herdr_timeout_ms, signal });
  }
  private async cleanupDisplayLocked(store: RecordStore, context: NativeContext, painted: Snapshot<AgentRecord>, separated: boolean, config: WorkCardsConfig, signal?: AbortSignal): Promise<void> {
    const state = await this.ownership(store, context, painted, config, signal);
    if (state.kind !== (separated ? 'separated' : 'paired')) throw new Error('No positively verified live owned display; nothing closed');
    if (context.terminalId !== state.agent.terminal_id && context.terminalId !== state.card.terminal_id) throw new Error('Foreign cleanup caller');
    if (context.terminalId === state.card.terminal_id && this.env.WORK_CARDS_NONCE !== state.nonce) throw new Error('Foreign cleanup nonce');
    const live = await this.guard(context, config, signal);
    const confirmed = classifyPair(live.snapshot, state.identity, context.terminalId);
    if (!isDeepStrictEqual(confirmed, state)) throw new Error('Pair changed before close; nothing closed');
    const nonce = live.snapshot.workspaces.find(w => w.workspace_id === state.agent.workspace_id)?.tokens.wcw_nonce;
    // Closing the card intentionally removes the caller. Continue only against its already-proven owning agent.
    const cleanupContext: NativeContext = { socket: context.socket, terminalId: state.agent.terminal_id, workspaceId: state.agent.workspace_id, tabId: state.agent.tab_id, ...(nonce ? { bindingNonce: nonce } : {}) };
    try {
      signal?.throwIfAborted();
      const closed = await this.result(['plugin', 'pane', 'close', state.card.pane_id], config, 'plugin_pane_closed');
      if (closed.pane_id !== state.card.pane_id) throw new Error('Closed pane identity mismatch');
      await this.currentRecords(store, painted);
      const after = await this.guard(cleanupContext, config, signal);
      const agent = locateTerminal(after.snapshot, state.agent.terminal_id);
      if (!pairKeys.every(k => agent.tokens[k] === state.agent.tokens[k]) || after.snapshot.panes.some(p => p.terminal_id === state.card.terminal_id)) throw new Error('Pair changed during close');
      await this.stamp('pane', agent, Object.fromEntries([...pairKeys, 'wca_status', 'wca_now', 'wca_status_short'].map(k => [k, null])), cleanupContext, config, signal, async () => {
        await this.currentRecords(store, painted);
        return () => {};
      });
    } catch (error) { throw new Error(`Display close may be partial; no shared binding was cleared: ${(error as Error).message}`, { cause: error }); }
  }
  /** Card-only caption projection; summary publication belongs to the owning Pi lifetime. */
  async projectCaption(store: RecordStore, context: NativeContext, painted: Snapshot<AgentRecord>, config: WorkCardsConfig, signal?: AbortSignal): Promise<void> {
    signal?.throwIfAborted();
    const captionOf = (current: Awaited<ReturnType<typeof this.pairedCard>>) => {
      const title = current.workspace.record.title.trim().toWellFormed();
      return current.pair.card.label === title ? undefined : title;
    };
    if (captionOf(await this.pairedCard(store, context, painted, config, signal)) === undefined) return;
    await withWorkspaceLock(context.socket, context.workspaceId, async () => {
      await this.pairedCard(store, context, painted, config, signal);
      try {
        const fresh = await this.pairedCard(store, context, painted, config, signal), caption = captionOf(fresh);
        if (caption !== undefined) {
          signal?.throwIfAborted();
          if (caption === '--clear') throw new Error('Unsupported native CLI caption "--clear": Herdr treats it as a clear flag');
          const card = fresh.pair.card;
          const renamed = nativePane((await this.result(['pane', 'rename', card.pane_id, caption], config, 'pane_info')).pane);
          if (renamed.pane_id !== card.pane_id || renamed.terminal_id !== card.terminal_id || renamed.workspace_id !== card.workspace_id || renamed.tab_id !== card.tab_id || renamed.label !== caption) throw new Error('Renamed card identity/caption mismatch');
        }
      } catch (error) { throw new Error(`Projection may be partial; card caption acknowledgement failed: ${(error as Error).message}`, { cause: error }); }
    }, { waitMs: config.herdr_timeout_ms, signal });
  }
  /** q leaves agent markers intact. Never-paired/restored empty membership needs explicit open. */
  async projectSummaries(store: RecordStore, context: NativeContext, painted: Snapshot<AgentRecord>, config: WorkCardsConfig, signal?: AbortSignal): Promise<void> {
    signal?.throwIfAborted();
    const read = async () => {
      const verified = await this.verifyMembership(store, context, painted, config, signal);
      if (verified.ownership.kind === 'empty') throw new Error('Explicit open establishes projection ownership; unmarked membership cannot publish summaries');
      const key = await this.storeKey(store);
      const live = await this.guard(verified.context, config, signal);
      this.bound(key, verified.workspace, live.snapshot, verified.context);
      const ownership = classifyPair(live.snapshot, verified.ownership.identity, context.terminalId);
      if (!isDeepStrictEqual(ownership, verified.ownership)) throw new Error('Summary ownership changed during verification');
      const pane = locateTerminal(live.snapshot, context.terminalId);
      const workspace = live.snapshot.workspaces.find(w => w.workspace_id === context.workspaceId)!;
      const changed = (tokens: Readonly<Record<string, string>>, values: Record<string, string>) =>
        Object.fromEntries(Object.entries(values).filter(([name, value]) => (tokens[name] ?? '') !== value));
      return { ...verified, key, pane, nativeWorkspace: workspace,
        own: changed(pane.tokens, { wca_status: verified.agent.record.status, wca_now: cap(verified.agent.record.now), wca_status_short: verified.agent.record.statusShort ?? '' }),
        shared: changed(workspace.tokens, { wcw_goal: cap(goalText(verified.workspace.record.goal)), wcw_goal_short: goalShort(verified.workspace.record.goal) }) };
    };
    const preflight = await read();
    if (!Object.keys(preflight.own).length && !Object.keys(preflight.shared).length) return;
    await withWorkspaceLock(context.socket, context.workspaceId, async () => {
      try {
        for (const kind of ['pane', 'workspace'] as const) {
          const current = await read(), patch = kind === 'pane' ? current.own : current.shared;
          if (!Object.keys(patch).length) continue;
          await this.stamp(kind, kind === 'pane' ? current.pane : current.nativeWorkspace, patch, current.context, config, signal, async () => {
            const fresh = await read();
            if (fresh.agent.revision !== current.agent.revision || fresh.workspace.revision !== current.workspace.revision ||
                !isDeepStrictEqual(fresh.ownership, current.ownership)) throw new Error('Summary records or ownership changed before publication; retry from fresh records');
            return snapshot => {
              this.bound(fresh.key, fresh.workspace, snapshot, fresh.context);
              if (fresh.ownership.kind === 'empty' || !isDeepStrictEqual(classifyPair(snapshot, fresh.ownership.identity, context.terminalId), fresh.ownership)) throw new Error('Summary ownership changed before publication');
            };
          });
        }
      } catch (error) { throw new Error(`Summary projection may be partial; already published fields are retained: ${(error as Error).message}`, { cause: error }); }
    }, { waitMs: config.herdr_timeout_ms, signal });
  }
  /** Workspace authority is shared; only the actual first layout pane may name its tab. */
  async projectNames(store: RecordStore, context: NativeContext, painted: Snapshot<AgentRecord>, config: WorkCardsConfig, signal?: AbortSignal, intent: 'automatic' | 'explicit' = 'automatic', acceptedOwnTitle: string | null = painted.record.title): Promise<NativeNamingResult> {
    signal?.throwIfAborted();
    return withWorkspaceLock(context.socket, context.workspaceId, async () => {
      const read = async (kind: 'workspace' | 'tab'): Promise<NativeNameRead> => {
        const records = await this.currentRecords(store, painted);
        const identity = await this.identity(store, records.agent), key = await this.storeKey(store);
        const live = await this.guard(context, config, signal);
        const binding = this.bound(key, records.workspace, live.snapshot, context);
        const ownership = classifyPair(live.snapshot, identity, context.terminalId);
        if (ownership.kind === 'empty' || ownership.agent.terminal_id !== context.terminalId || ownership.nonce !== records.agent.record.displayAttempt?.nonce) throw new Error('Verified marked agent ownership required for naming');
        const pane = locateTerminal(live.snapshot, context.terminalId);
        if (kind === 'tab') {
          if (!firstPane(live.snapshot, pane)) return { kind: 'skip', reason: 'not-first' };
          if (intent === 'automatic' && (acceptedOwnTitle === null || records.agent.record.title !== acceptedOwnTitle)) return { kind: 'skip', reason: 'not-ready' };
        }
        const surface = kind === 'workspace' ? live.snapshot.workspaces.find(w => w.workspace_id === context.workspaceId)! : live.snapshot.tabs.find(t => t.tab_id === context.tabId)!;
        return { kind: 'ready', bindingNonce: binding.nonce, surfaceId: kind === 'workspace' ? context.workspaceId : context.tabId,
          owner: { terminalId: pane.terminal_id, nonce: ownership.nonce }, label: surface.label,
          desired: kind === 'workspace' ? records.workspace.record.title : records.agent.record.title,
          // Herdr derives workspace defaults from Git/CWD, but does not expose custom-name state.
          // Tabs default to their current one-based position, not their stable public number.
          defaultLabelProven: kind === 'tab' && surface.label === String(live.snapshot.tabs.filter(t => t.workspace_id === context.workspaceId).findIndex(t => t.tab_id === context.tabId) + 1),
          proof: { agent: records.agent.revision, workspace: records.workspace.revision, binding, ownership, pane,
            ...(kind === 'tab' ? { layout: live.snapshot.layouts.find(l => l.tab_id === context.tabId) } : {}), number: surface.number } };
      };
      try {
        const workspace = await this.projectNativeName(store, context, 'workspace', () => read('workspace'), config, signal, intent);
        const tab = await this.projectNativeName(store, context, 'tab', () => read('tab'), config, signal, intent);
        if (workspace === 'not-first' || workspace === 'not-ready') throw new Error('Missing workspace naming authority');
        return { workspace, tab };
      } catch (error) { throw new Error(`Native naming may be partial; verified names and pins are retained: ${(error as Error).message}`, { cause: error }); }
    }, { waitMs: config.herdr_timeout_ms, signal });
  }
  /** Caller holds the workspace lock. Both agent and editor use this surface history. */
  private async projectNativeName(store: RecordStore, context: NativeContext, kind: 'workspace' | 'tab', read: () => Promise<NativeNameRead>, config: WorkCardsConfig, signal?: AbortSignal, intent: 'automatic' | 'explicit' = 'automatic'): Promise<'renamed' | 'unchanged' | 'manual' | 'not-first' | 'not-ready'> {
    if (!context.bindingNonce) throw new Error('Native naming requires a captured binding nonce');
    const initial = await read();
    if (initial.kind === 'skip') return initial.reason;
    if (initial.bindingNonce !== context.bindingNonce) throw new Error('Native naming binding lifetime changed');
    const names = new NameStore(store.directory);
    const scope = await names.scope({ socket: context.socket, workspaceId: context.workspaceId, bindingNonce: initial.bindingNonce, kind, surfaceId: initial.surfaceId });
    const previous = await names.read(scope);
    const same = (current: NativeNameRead, label: string): NativeNameAuthority => {
      if (current.kind === 'skip' || current.label !== label || !isDeepStrictEqual({ ...current, label }, { ...initial, label })) throw new Error('Native naming records, surface, layout or label changed during preparation');
      return current;
    };
    const current = same(await read(), initial.label);
    const decision = decideName({ scope, provenance: previous?.record ?? null, owner: current.owner, currentLabel: current.label,
      desiredName: current.desired, defaultLabelProven: current.defaultLabelProven, intent });
    if (decision.kind === 'noop') return 'unchanged';
    if (decision.kind === 'manual') {
      await names.write(scope, decision.provenance, previous?.revision ?? null, signal);
      return 'manual';
    }
    if (decision.kind === 'rename') {
      // Native CLI joins all positional label arguments; reject option-like names for older CLI parsers.
      if (decision.name.startsWith('-')) throw new Error('Native names cannot start with a CLI option prefix');
      signal?.throwIfAborted();
      const result = await this.result([kind, 'rename', current.surfaceId, decision.name], config, kind === 'workspace' ? 'workspace_info' : 'tab_info');
      const returned = nativeObject(result[kind]);
      if (returned.workspace_id !== context.workspaceId || (kind === 'tab' && returned.tab_id !== current.surfaceId) || returned.label !== decision.name || returned.number !== current.proof.number) throw new Error('Native naming acknowledgement identity/label mismatch');
    }
    const after = await read();
    // Default eligibility describes the old label, not the new automatic name.
    same(after.kind === 'ready' ? { ...after, defaultLabelProven: initial.defaultLabelProven } : after, current.desired);
    signal?.throwIfAborted();
    await names.write(scope, decision.afterSuccess, previous?.revision ?? null, signal);
    return decision.kind === 'rename' ? 'renamed' : 'unchanged';
  }
  async openDocument(store: RecordStore, context: NativeContext, paintedWorkspace: Snapshot<WorkspaceRecord>, paintedAgent: Snapshot<AgentRecord>, target: DocumentIdentity, config: WorkCardsConfig, signal?: AbortSignal): Promise<void> {
    signal?.throwIfAborted();
    const identity = documentIdentity(target), command = documentCommand(config, this.env);
    await withWorkspaceLock(context.socket, context.workspaceId, async () => {
      const verify = async () => {
        const records = await this.currentRecords(store, paintedAgent);
        if (await realpath(paintedWorkspace.path) !== await realpath(records.workspace.path) || paintedWorkspace.record.id !== records.workspace.record.id) throw new Error('Stale painted workspace identity');
        const key = referenceIdentity(identity);
        if (!paintedWorkspace.record.documents.some(d => referenceIdentity(d) === key) || !records.workspace.record.documents.some(d => referenceIdentity(d) === key)) throw new Error('Document reference no longer present');
        const cwd = records.workspace.record.checkouts[identity.checkout]?.directory;
        if (cwd !== paintedWorkspace.record.checkouts[identity.checkout]?.directory) throw new Error('Stale painted checkout directory');
        const document = await resolveWorkspaceDocument(records.workspace.record, identity);
        // All filesystem/identity preparation precedes the final authoritative native read.
        const v = await this.pairedCard(store, context, paintedAgent, config, signal);
        if (v.workspace.record.checkouts[identity.checkout]?.directory !== cwd || !v.workspace.record.documents.some(d => referenceIdentity(d) === key)) throw new Error('Document target changed during validation');
        const label = resolvedReferences(v.workspace.record).documents.find(d => referenceIdentity(d) === key)!.label;
        return { ...v, cwd, document, key: digest(JSON.stringify([cwd, document])), label };
      };
      let v = await verify();
      const matching = (s: NativeSnapshot): Pane | undefined => {
        const matches = s.panes.filter(p => p.workspace_id === context.workspaceId && p.tokens.wcd_key === v.key);
        if (!matches.length) return undefined;
        const p = matches[0];
        if (matches.length !== 1 || p.tokens.wcd_pane !== p.pane_id || p.tokens.wcd_terminal !== p.terminal_id || p.tokens.wcd_tab !== p.tab_id) throw new Error('Conflicting/incomplete document editor tuple');
        return p;
      };
      const focus = async (p: Pane) => {
        v = await verify();
        const current = matching(v.live.snapshot);
        if (!current || current.terminal_id !== p.terminal_id) throw new Error('Document editor changed before focus');
        signal?.throwIfAborted();
        const response = nativeObject(JSON.parse(await this.run(['tab', 'focus', current.tab_id], config.herdr_timeout_ms)));
        if (response.error) throw new Error(`Herdr focus error: ${JSON.stringify(response.error)}`);
      };
      const existing = matching(v.live.snapshot);
      if (existing) {
        try { await focus(existing); }
        catch (error) { throw new Error(`Document focus may be partial (pane ${existing.pane_id}, terminal ${existing.terminal_id}, tab ${existing.tab_id}): ${(error as Error).message}`, { cause: error }); }
        return;
      }
      let returned: Pane | undefined;
      try {
        await this.ensurePlugin(config, verify); const root = await realpath(this.pluginRoot); v = await verify();
        const raced = matching(v.live.snapshot); if (raced) { await focus(raced); return; }
        signal?.throwIfAborted();
        const launched = { cwd: v.cwd, document: v.document, key: v.key };
        const opened = await this.result(['plugin', 'pane', 'open', '--plugin', 'work-cards', '--entrypoint', 'document', '--placement', 'tab', '--workspace', context.workspaceId, '--cwd', root, '--no-focus',
          '--env', `WORK_CARDS_DOCUMENT_COMMAND=${command}`, '--env', `WORK_CARDS_DOCUMENT_CWD=${v.cwd}`, '--env', `WORK_CARDS_DOCUMENT_PATH=${v.document}`, ...(this.env.PATH === undefined ? [] : ['--env', `PATH=${this.env.PATH}`])], config, 'plugin_pane_opened');
        const info = nativeObject(opened.plugin_pane); returned = nativePane(info.pane);
        if (info.plugin_id !== 'work-cards' || info.entrypoint !== 'document' || returned.workspace_id !== context.workspaceId || v.live.snapshot.panes.some(p => p.pane_id === returned!.pane_id || p.terminal_id === returned!.terminal_id || p.tab_id === returned!.tab_id)) throw new Error('Unexpected/reused document pane/tab/terminal');
        const storeKey = await this.storeKey(store);
        v = await verify();
        if (v.cwd !== launched.cwd || v.document !== launched.document || v.key !== launched.key) throw new Error('Launched canonical document target changed; returned editor remains unowned');
        const pane = locateTerminal(v.live.snapshot, returned.terminal_id);
        if (!isDeepStrictEqual(pane, returned) || !firstPane(v.live.snapshot, pane)) throw new Error('New document pane identity/layout changed');
        const nonce = randomUUID();
        await this.stamp('pane', pane, { wcd_key: v.key, wcd_pane: pane.pane_id, wcd_terminal: pane.terminal_id, wcd_tab: pane.tab_id, wcd_owner: nonce, wcd_store: storeKey }, context, config, signal);
        v = await verify(); const stamped = matching(v.live.snapshot);
        if (!stamped || stamped.terminal_id !== pane.terminal_id || stamped.tokens.wcd_owner !== nonce || !firstPane(v.live.snapshot, stamped)) throw new Error('Document metadata did not round-trip');
        await this.renameEditor(store, stamped, context, config, signal, verify);
        await focus(stamped);
      } catch (error) { throw new Error(`Document opening may be partial${returned ? ` (pane ${returned.pane_id}, terminal ${returned.terminal_id}, tab ${returned.tab_id})` : ''}; inspect before retrying, no editor was closed: ${(error as Error).message}`, { cause: error }); }
    }, { waitMs: config.herdr_timeout_ms, signal });
  }
  /** Opt-in editor-only qualification refresh. Never called by reuse/open, never names agent/workspace surfaces. */
  async refreshDocumentNames(store: RecordStore, context: NativeContext, painted: Snapshot<AgentRecord>, config: WorkCardsConfig, signal?: AbortSignal): Promise<void> {
    signal?.throwIfAborted();
    await withWorkspaceLock(context.socket, context.workspaceId, async () => {
      const initial = await this.pairedCard(store, context, painted, config, signal);
      const refs = resolvedReferences(initial.workspace.record).documents;
      for (const ref of refs) {
        let current = await this.pairedCard(store, context, painted, config, signal);
        const latest = resolvedReferences(current.workspace.record).documents.find(d => referenceIdentity(d) === referenceIdentity(ref));
        if (!latest) continue;
        const file = await resolveWorkspaceDocument(current.workspace.record, { checkout: ref.checkout, path: ref.path });
        const key = digest(JSON.stringify([current.workspace.record.checkouts[ref.checkout].directory, file]));
        const storeKey = await this.storeKey(store);
        current = await this.pairedCard(store, context, painted, config, signal);
        const matches = current.live.snapshot.panes.filter(p => p.workspace_id === context.workspaceId && p.tokens.wcd_key === key);
        if (!matches.length) continue;
        const pane = matches[0], t = pane.tokens;
        if (matches.length !== 1 || t.wcd_pane !== pane.pane_id || t.wcd_terminal !== pane.terminal_id || t.wcd_tab !== pane.tab_id) throw new Error('Conflicting/incomplete document editor tuple');
        if (t.wcd_store !== storeKey || !t.wcd_owner) continue;
        const verify = async () => {
          const records = await this.currentRecords(store, painted);
          const reference = resolvedReferences(records.workspace.record).documents.find(d => referenceIdentity(d) === referenceIdentity(ref));
          if (!reference) throw new Error('Document reference changed during naming');
          const path = await resolveWorkspaceDocument(records.workspace.record, { checkout: ref.checkout, path: ref.path });
          const currentKey = digest(JSON.stringify([records.workspace.record.checkouts[ref.checkout].directory, path]));
          const verified = await this.pairedCard(store, context, painted, config, signal);
          if (verified.workspace.revision !== records.workspace.revision || currentKey !== key) throw new Error('Document identity changed during naming');
          return { ...verified, key, label: reference.label };
        };
        await this.renameEditor(store, pane, context, config, signal, verify);
      }
    }, { waitMs: config.herdr_timeout_ms, signal });
  }
  private async renameEditor(store: RecordStore, pane: Pane, context: NativeContext, config: WorkCardsConfig, signal: AbortSignal | undefined,
    verify: () => Promise<{ agent: Snapshot<AgentRecord>; workspace: Snapshot<WorkspaceRecord>; live: { snapshot: NativeSnapshot }; pair: LivePair; key: string; label: string }>): Promise<void> {
    const storeKey = await this.storeKey(store);
    await this.projectNativeName(store, context, 'tab', async () => {
      const current = await verify(), snapshot = current.live.snapshot;
      const binding = this.bound(storeKey, current.workspace, snapshot, context);
      const p = locateTerminal(snapshot, pane.terminal_id), t = p.tokens;
      if (snapshot.panes.filter(p => p.workspace_id === context.workspaceId && p.tokens.wcd_key === current.key).length !== 1) throw new Error('Conflicting document editor ownership');
      if (p.workspace_id !== context.workspaceId || !isDeepStrictEqual(p, pane) || t.wcd_key !== current.key || t.wcd_store !== storeKey || !t.wcd_owner || t.wcd_pane !== p.pane_id || t.wcd_terminal !== p.terminal_id || t.wcd_tab !== p.tab_id) throw new Error('Editor naming identity changed');
      if (!firstPane(snapshot, p)) return { kind: 'skip', reason: 'not-first' };
      const tab = snapshot.tabs.find(t => t.tab_id === p.tab_id)!;
      return { kind: 'ready', bindingNonce: binding.nonce, surfaceId: p.tab_id, owner: { terminalId: p.terminal_id, nonce: t.wcd_owner },
        label: tab.label, desired: current.label,
        defaultLabelProven: tab.label === String(snapshot.tabs.filter(t => t.workspace_id === p.workspace_id).findIndex(t => t.tab_id === p.tab_id) + 1),
        proof: { agent: current.agent.revision, workspace: current.workspace.revision, binding, ownership: current.pair, pane: p, layout: snapshot.layouts.find(l => l.tab_id === p.tab_id), number: tab.number } };
    }, config, signal);
  }
}
