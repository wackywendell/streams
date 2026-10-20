import { randomUUID } from 'node:crypto';
import { realpath } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { isDeepStrictEqual } from 'node:util';
import { configPath, documentCommand, type WorkCardsConfig } from '../config.js';
import { decideName, NameStore } from './name-provenance.js';
import { HerdrTransport, object, type ClientOptions } from './transport.js';
import { classifyPair, firstPane, locateTerminal, nativePane, pairKeys, parseNativeSnapshot, type NativeSnapshot, type Pane, type PairState } from './workspace-ownership.js';
import { withWorkspaceLock } from './workspace-lock.js';
import { resolveStreamsDocument } from './streams-documents.js';
import { STREAMS_READ_ENV_KEYS, sameStreamsScope, type StreamsCardSnapshot, type StreamsCardSource } from './streams-source.js';
import type { StreamsDocumentTarget } from './streams-view.js';
import { StreamsNativeState, streamsDigest, type StreamsAttempt } from './streams-native-state.js';

export interface StreamsNativeContext { socket: string; paneId: string; terminalId: string; workspaceId: string; tabId: string }
export interface StreamsWorkspaceOptions extends ClientOptions { stateDirectory?: string }
type Source = Pick<StreamsCardSource, 'read'>;
const tuple = (p: Pane) => [p.pane_id, p.terminal_id, p.workspace_id, p.tab_id];
const contextTuple = (c: StreamsNativeContext) => [c.paneId, c.terminalId, c.workspaceId, c.tabId];
const pluginId = 'streams-work-cards';
// Herdr trims, removes controls and caps tokens at 80 Unicode scalar values.
// An optional native hint must preserve the catalog text, not invent a shorter summary.
function compactToken(value: string | null): string | null {
  return value && Array.from(value).length <= 80 && !/[\p{Cc}\p{Cs}]/u.test(value)
    && !/^\p{White_Space}|\p{White_Space}$/u.test(value) ? value : null;
}
const pluginPanes = [
  { id: 'card', title: 'Streams work card', placement: 'split', command: ['node', 'dist/herdr/streams-pane.js'] },
  { id: 'document', title: 'Document', placement: 'tab', command: ['/bin/sh', 'bin/open-document.sh'] },
];
interface Verified {
  saved: StreamsCardSnapshot; snapshot: NativeSnapshot; caller: Pane; owner: Pane;
  identity: string; key: string; binding: Record<string, string>; pair: PairState;
}

/** Streams projections only: no RecordStore, catalog writes, enrollment or workspace launch. */
export class StreamsWorkspaceClient extends HerdrTransport {
  private readonly stateDirectory?: string;
  private readonly owners = new WeakMap<StreamsNativeContext, { scope: StreamsCardSnapshot['scope']; tuple: string[]; binding?: string }>();
  constructor(options: StreamsWorkspaceOptions = {}) { super(options); this.stateDirectory = options.stateDirectory; }
  private state(context: StreamsNativeContext): StreamsNativeState {
    return new StreamsNativeState(this.stateDirectory ?? join(dirname(context.socket), '.streams-work-cards'));
  }
  async snapshot(config: WorkCardsConfig, signal?: AbortSignal): Promise<NativeSnapshot> {
    return parseNativeSnapshot((await this.result(['api', 'snapshot'], config, 'session_snapshot', signal)).snapshot);
  }
  async bootstrap(config: WorkCardsConfig, signal?: AbortSignal): Promise<StreamsNativeContext> {
    const current = await this.current(config, signal), socket = await this.socket(config, signal);
    const live = locateTerminal(await this.snapshot(config, signal), current.terminal_id);
    if (!isDeepStrictEqual(tuple(live), tuple(current)) || live.pane_id !== this.env.HERDR_PANE_ID
      || (this.env.HERDR_TAB_ID && this.env.HERDR_TAB_ID !== live.tab_id)
      || (this.env.HERDR_WORKSPACE_ID && this.env.HERDR_WORKSPACE_ID !== live.workspace_id)) throw new Error('Caller native tuple changed during bootstrap');
    return { socket, paneId: live.pane_id, terminalId: live.terminal_id, workspaceId: live.workspace_id, tabId: live.tab_id };
  }
  private async guard(source: Source, context: StreamsNativeContext, config: WorkCardsConfig, signal?: AbortSignal, preRead?: StreamsCardSnapshot): Promise<Verified> {
    signal?.throwIfAborted();
    const saved = preRead ?? await source.read(signal);
    if (await this.socket(config, signal) !== context.socket) throw new Error('Native endpoint changed');
    const snapshot = await this.snapshot(config, signal);
    const caller = locateTerminal(snapshot, context.terminalId);
    if (!isDeepStrictEqual(tuple(caller), contextTuple(context))
      || caller.pane_id !== this.env.HERDR_PANE_ID) throw new Error('Caller native tuple moved or changed');
    const owner = snapshot.panes.find(p => p.pane_id === saved.native.pane_id);
    if (!owner || owner.tab_id !== saved.native.tab_id || owner.workspace_id !== saved.native.workspace_id
      || owner.workspace_id !== context.workspaceId || owner.tab_id !== context.tabId) throw new Error('Attested owner moved or does not match this card workspace/tab');
    const captured = this.owners.get(context);
    if (captured && (!sameStreamsScope(captured.scope, saved.scope) || !isDeepStrictEqual(captured.tuple, tuple(owner)))) throw new Error('Attested owner tuple or scope changed');
    const identity = streamsDigest(saved.scope), key = streamsDigest([context.socket, saved.scope, tuple(owner)]);
    const binding = { strm_catalog: streamsDigest(saved.scope.catalogSource), strm_host: saved.scope.hostId, strm_stream: saved.scope.streamId };
    const workspace = snapshot.workspaces.find(w => w.workspace_id === context.workspaceId)!;
    if (captured?.binding && captured.binding !== workspace.tokens.strm_binding) throw new Error('Native workspace binding lifetime changed');
    this.owners.set(context, { scope: { ...saved.scope }, tuple: tuple(owner), binding: workspace.tokens.strm_binding });
    const marked = Object.keys(workspace.tokens).some(k => k.startsWith('strm_'));
    if (marked && (!workspace.tokens.strm_binding || Object.entries(binding).some(([k, v]) => workspace.tokens[k] !== v))) throw new Error('Conflicting native Stream workspace binding');
    const pair = classifyPair(snapshot, identity, owner.terminal_id);
    if (pair.kind === 'card-only') {
      if (pair.card.tokens.strm_native !== saved.scope.nativeSessionId) throw new Error('Native Pi UUID mismatch');
    } else if (pair.kind !== 'empty' && (!isDeepStrictEqual(tuple(pair.agent), tuple(owner)) || pair.agent.tokens.strm_native !== saved.scope.nativeSessionId)) throw new Error('Native Pi UUID or owner tuple mismatch');
    if (caller.terminal_id !== owner.terminal_id && (pair.kind !== 'paired' || pair.card.terminal_id !== caller.terminal_id
      || pair.nonce !== this.env.STREAMS_CARDS_NONCE || pair.card.tokens.strm_native !== saved.scope.nativeSessionId
      || this.env.STREAMS_CARDS_OWNER_TERMINAL !== owner.terminal_id)) throw new Error('Caller is not the exact owned Streams card');
    const latest = preRead ?? await source.read(signal);
    if (!sameStreamsScope(latest.scope, saved.scope) || !isDeepStrictEqual(latest.native, saved.native)) throw new Error('Streams context changed during native verification');
    signal?.throwIfAborted();
    return { saved: latest, snapshot, caller, owner, identity, key, binding, pair };
  }
  private bound(v: Verified, context: StreamsNativeContext): string {
    const w = v.snapshot.workspaces.find(w => w.workspace_id === context.workspaceId)!;
    if (!w.tokens.strm_binding || Object.entries(v.binding).some(([k, value]) => w.tokens[k] !== value)) throw new Error('Verified native Stream binding required');
    return w.tokens.strm_binding;
  }
  private async evidence(v: Verified, context: StreamsNativeContext): Promise<StreamsAttempt | null> {
    if (v.pair.kind === 'card-only') throw new Error('Uncertain Streams launch retained; owner must complete the reserved pair');
    const attempt = await this.state(context).read(v.key);
    if (attempt && attempt.phase === 'pending') {
      if (v.pair.kind === 'absent' && attempt.nonce === v.pair.nonce && attempt.terminalId === v.pair.agent.tokens.strm_card
        && !v.snapshot.panes.some(p => p.terminal_id === attempt.terminalId || p.pane_id === attempt.paneId)) {
        return { ...attempt, phase: 'closed' };
      }
      if (v.pair.kind !== 'paired' || v.pair.nonce !== attempt.nonce || v.pair.card.pane_id !== attempt.paneId
        || v.pair.card.terminal_id !== attempt.terminalId || v.pair.card.tab_id !== attempt.tabId) {
        throw new Error('Uncertain Streams launch retained; inspect native launch evidence before retrying');
      }
      return { ...attempt, phase: 'ready' };
    }
    if (attempt && v.pair.kind === 'empty' && v.snapshot.panes.some(p => p.terminal_id === attempt.terminalId || p.pane_id === attempt.paneId)) throw new Error('Recorded card is still live without verified ownership; no duplicate or cleanup permitted');
    if (v.pair.kind !== 'empty' && (!attempt || v.pair.nonce !== attempt.nonce || v.pair.agent.tokens.strm_card !== attempt.terminalId
      || ('card' in v.pair && (v.pair.card.pane_id !== attempt.paneId || v.pair.card.terminal_id !== attempt.terminalId || v.pair.card.tab_id !== attempt.tabId)))) throw new Error('Pair does not match durable native launch evidence');
    return attempt;
  }
  private async marked(source: Source, context: StreamsNativeContext, config: WorkCardsConfig, signal?: AbortSignal, preRead?: StreamsCardSnapshot): Promise<Verified> {
    const v = await this.guard(source, context, config, signal, preRead); this.bound(v, context);
    await this.evidence(v, context);
    if (v.pair.kind === 'empty' || v.pair.kind === 'separated') throw new Error('Verified paired or absent owner markers required');
    return v;
  }
  /** Read-only proof for deciding that no effect or provenance write is needed. */
  private async observeProjection(source: Source, context: StreamsNativeContext, config: WorkCardsConfig, signal?: AbortSignal): Promise<Verified> {
    const v = await this.guard(source, context, config, signal);
    if (v.pair.kind !== 'empty') {
      this.bound(v, context);
      await this.evidence(v, context);
      if (v.pair.kind === 'separated') throw new Error('Verified paired or absent owner markers required');
      return v;
    }
    if (v.caller.terminal_id !== v.owner.terminal_id) throw new Error('Only the attested Pi owner may initialize projections');
    if (pairKeys.some(k => Object.hasOwn(v.owner.tokens, k))) throw new Error('Incomplete Streams owner markers');
    await this.evidence(v, context);
    if (v.snapshot.workspaces.find(w => w.workspace_id === context.workspaceId)!.tokens.strm_binding) this.bound(v, context);
    return v;
  }
  /** Initial binding publication requires the workspace lock; observation alone never authorizes an effect. */
  private async projectionProof(source: Source, context: StreamsNativeContext, config: WorkCardsConfig, signal?: AbortSignal): Promise<Verified> {
    let v = await this.observeProjection(source, context, config, signal);
    if (v.pair.kind !== 'empty') return v;
    const workspace = v.snapshot.workspaces.find(w => w.workspace_id === context.workspaceId)!;
    if (!workspace.tokens.strm_binding) await this.stamp(source, context, workspace, { ...v.binding, strm_binding: randomUUID() }, config, signal);
    v = await this.guard(source, context, config, signal);
    this.bound(v, context); await this.evidence(v, context);
    if (v.pair.kind !== 'empty') throw new Error('Pair changed while establishing projection proof');
    return v;
  }
  /** Every metadata effect re-reads both app attestation and the exact native target. */
  private async stamp(source: Source, context: StreamsNativeContext, target: Pane | NativeSnapshot['workspaces'][number], values: Record<string, string | null>, config: WorkCardsConfig, signal?: AbortSignal, verify?: (v: Verified) => Promise<unknown>, beforeEffect?: () => void): Promise<Verified> {
    const v = await this.guard(source, context, config, signal);
    await verify?.(v);
    const kind = 'pane_id' in target ? 'pane' : 'workspace', targetId = 'pane_id' in target ? target.pane_id : target.workspace_id;
    const live = kind === 'pane' ? v.snapshot.panes.find(p => p.pane_id === targetId) : v.snapshot.workspaces.find(w => w.workspace_id === targetId);
    if (!live || !isDeepStrictEqual(live, target)) throw new Error('Native metadata target changed');
    const argv = [kind, 'report-metadata', targetId, '--source', pluginId];
    for (const [k, value] of Object.entries(values)) argv.push(value === null ? '--clear-token' : '--token', value === null ? k : `${k}=${value}`);
    signal?.throwIfAborted(); beforeEffect?.(); await this.run(argv, config.herdr_timeout_ms, signal);
    const after = await this.guard(source, context, config, signal);
    await verify?.(after);
    const surface = kind === 'pane' ? after.snapshot.panes.find(p => p.pane_id === targetId) : after.snapshot.workspaces.find(w => w.workspace_id === targetId);
    if (!surface || Object.entries(values).some(([k, value]) => value === null ? Object.hasOwn(surface.tokens, k) : surface.tokens[k] !== value)) throw new Error('Native metadata acknowledgement did not round-trip; effect may be partial');
    return after;
  }
  private async ensureStreamsPlugin(config: WorkCardsConfig, verify: () => Promise<unknown>, signal?: AbortSignal): Promise<boolean> {
    const root = await realpath(this.pluginRoot), manifest = join(root, 'streams-herdr-plugin.toml');
    const check = (raw: unknown) => {
      const p = object(raw, 'Streams plugin');
      if (p.plugin_id !== pluginId || p.enabled !== true || p.plugin_root !== root || p.manifest_path !== manifest
        || p.source?.kind !== 'local' || p.version !== '0.1.0' || p.min_herdr_version !== '0.9.3'
        || !isDeepStrictEqual(p.platforms, ['macos', 'linux']) || !isDeepStrictEqual(p.panes, pluginPanes)) throw new Error(`Conflicting Streams plugin; explicitly link ${manifest}. No registration was replaced.`);
    };
    const plugins = (await this.result(['plugin', 'list', '--plugin', pluginId, '--json'], config, 'plugin_list', signal)).plugins;
    if (!Array.isArray(plugins) || plugins.length > 1) throw new Error('Unexpected Streams plugin registrations');
    if (plugins.length) { check(plugins[0]); return false; }
    await verify(); check((await this.result(['plugin', 'link', manifest], config, 'plugin_linked', signal)).plugin);
    return true;
  }
  private async reservation(v: Verified, context: StreamsNativeContext, expected: StreamsAttempt): Promise<Pane> {
    this.bound(v, context);
    if (v.caller.terminal_id !== v.owner.terminal_id || expected.key !== v.key || expected.phase !== 'pending'
      || !isDeepStrictEqual(await this.state(context).read(v.key), expected)) throw new Error('Reserved pair identity or durable evidence changed');
    const card = v.snapshot.panes.find(p => p.pane_id === expected.paneId);
    if (!card || card.terminal_id !== expected.terminalId || card.tab_id !== expected.tabId
      || card.workspace_id !== v.owner.workspace_id || card.tab_id !== v.owner.tab_id) throw new Error('Reserved card tuple moved or changed');
    if (v.pair.kind === 'empty') {
      if (pairKeys.some(k => Object.hasOwn(card.tokens, k))) throw new Error('Incomplete or conflicting reserved pair markers');
    } else if ((v.pair.kind !== 'card-only' && v.pair.kind !== 'paired') || v.pair.nonce !== expected.nonce
      || !isDeepStrictEqual(v.pair.card, card)) throw new Error('Conflicting reserved pair publisher');
    return card;
  }
  private async completeReservation(source: Source, context: StreamsNativeContext, config: WorkCardsConfig, v: Verified, attempt: StreamsAttempt, signal?: AbortSignal): Promise<PairState> {
    const card = await this.reservation(v, context, attempt);
    const markers = { strm_agent: v.identity, strm_nonce: attempt.nonce, strm_pi: v.owner.terminal_id, strm_card: card.terminal_id, strm_native: v.saved.scope.nativeSessionId };
    const verify = (fresh: Verified) => this.reservation(fresh, context, attempt);
    if (v.pair.kind === 'empty') v = await this.stamp(source, context, card, { ...markers, strm_role: 'card' }, config, signal, verify);
    v = await this.stamp(source, context, v.owner, { ...markers, strm_role: 'agent' }, config, signal, verify);
    if (v.pair.kind !== 'paired' || v.pair.nonce !== attempt.nonce) throw new Error('Reserved pair did not round-trip; evidence retained');
    await this.state(context).write({ ...attempt, phase: 'ready' });
    return v.pair;
  }
  async openDisplay(source: Source, context: StreamsNativeContext, config: WorkCardsConfig, signal?: AbortSignal, mode: 'automatic' | 'explicit' = 'automatic'): Promise<PairState | { kind: 'suppressed' }> {
    const observed = await this.guard(source, context, config, signal);
    if (observed.caller.terminal_id !== observed.owner.terminal_id) throw new Error('Only the attested Pi owner may open a display');
    const record = await this.state(context).read(observed.key);
    if (record?.phase !== 'pending') {
      const attempt = await this.evidence(observed, context);
      if (observed.pair.kind === 'paired') { this.bound(observed, context); return observed.pair; }
      if (mode === 'automatic' && (attempt || observed.pair.kind === 'absent')) return { kind: 'suppressed' };
    }
    return withWorkspaceLock(context.socket, context.workspaceId, async () => {
      let v = await this.guard(source, context, config, signal);
      if (v.caller.terminal_id !== v.owner.terminal_id) throw new Error('Only the attested Pi owner may open a display');
      const state = this.state(context);
      if (v.pair.kind === 'empty' || v.pair.kind === 'card-only') {
        const pending = await state.read(v.key);
        if (pending?.phase === 'pending' && pending.paneId && pending.terminalId && pending.tabId) return this.completeReservation(source, context, config, v, pending, signal);
      }
      const attempt = await this.evidence(v, context);
      if (v.pair.kind === 'paired') { this.bound(v, context); await state.write(attempt!); return v.pair; }
      if (v.pair.kind === 'separated') throw new Error('Owned card moved; no resource was closed or replaced');
      if (mode === 'automatic' && (attempt || v.pair.kind === 'absent')) {
        if (attempt) await state.write(attempt);
        return { kind: 'suppressed' };
      }
      if (v.pair.kind === 'empty' && pairKeys.some(k => Object.hasOwn(v.owner.tokens, k))) throw new Error('Incomplete Streams owner markers');
      const linked = await this.ensureStreamsPlugin(config, () => this.guard(source, context, config, signal), signal);
      if (linked) v = await this.guard(source, context, config, signal);
      const workspace = v.snapshot.workspaces.find(w => w.workspace_id === context.workspaceId)!;
      if (!workspace.tokens.strm_binding) {
        v = await this.stamp(source, context, workspace, { ...v.binding, strm_binding: randomUUID() }, config, signal);
      }
      this.bound(v, context);
      const nonce = randomUUID(), root = await realpath(this.pluginRoot);
      if (v.pair.kind !== 'empty' && v.pair.kind !== 'absent') throw new Error('Pair changed before launch');
      const vars: Record<string, string> = {
        STREAMS_CARDS_SCOPE: JSON.stringify(v.saved.scope), STREAMS_CARDS_OWNER: JSON.stringify(v.saved.native),
        STREAMS_CARDS_OWNER_TERMINAL: v.owner.terminal_id, STREAMS_CARDS_NONCE: nonce,
        STREAMS_CARDS_READ_ENV: JSON.stringify(Object.fromEntries(STREAMS_READ_ENV_KEYS.filter(k => this.env[k] !== undefined).map(k => [k, this.env[k]]))),
        HERDR_SOCKET_PATH: context.socket,
        STREAMS_CARDS_STATE_DIR: state.directory, STREAMS_CARDS_CONFIG: this.env.STREAMS_CARDS_CONFIG || configPath(this.env), EDITOR: this.env.EDITOR ?? '',
        ...(this.env.PATH === undefined ? {} : { PATH: this.env.PATH }),
      };
      const pending: StreamsAttempt = { version: 1, key: v.key, phase: 'pending', nonce };
      let effectStarted = false, opened;
      // A failed reservation write may leave only its gate: never roll that back.
      await state.write(pending);
      try {
        if (v.pair.kind === 'absent') {
          v = await this.stamp(source, context, v.owner, Object.fromEntries(pairKeys.map(k => [k, null])), config, signal, undefined, () => { effectStarted = true; });
          if (v.pair.kind !== 'empty') throw new Error('Pair changed before launch; evidence retained');
        }
        signal?.throwIfAborted(); effectStarted = true;
        opened = await this.result(['plugin', 'pane', 'open', '--plugin', pluginId, '--entrypoint', 'card', '--placement', 'split', '--target-pane', v.owner.pane_id, '--direction', 'down', '--cwd', root, '--no-focus', ...Object.entries(vars).flatMap(([k, value]) => ['--env', `${k}=${value}`])], config, 'plugin_pane_opened', signal);
      } catch (error) {
        if (!effectStarted) await state.discardUnlaunched(pending, attempt);
        throw error;
      }
      const info = object(opened.plugin_pane, 'Streams launch'), returned = nativePane(info.pane);
      if (info.plugin_id !== pluginId || info.entrypoint !== 'card' || returned.workspace_id !== context.workspaceId || returned.tab_id !== context.tabId
        || v.snapshot.panes.some(p => p.pane_id === returned.pane_id || p.terminal_id === returned.terminal_id)) throw new Error('Unexpected/replayed native card response; launch evidence retained');
      const reserved: StreamsAttempt = { version: 1, key: v.key, phase: 'pending', nonce, paneId: returned.pane_id, terminalId: returned.terminal_id, tabId: returned.tab_id };
      await state.write(reserved);
      if (config.pane_height_fraction !== 0.5) {
        const target = config.pane_height_fraction < 0.5 ? v.owner : returned;
        const resized = await this.result(['pane', 'resize', '--pane', target.pane_id, '--direction', config.pane_height_fraction < 0.5 ? 'down' : 'up', '--amount', String(Math.abs(0.5 - config.pane_height_fraction))], config, 'pane_resize', signal);
        if (resized.resize?.pane_id !== target.pane_id) throw new Error('Initial sizing acknowledgement mismatch; display retained and sizing will not retry');
      }
      v = await this.guard(source, context, config, signal);
      const card = locateTerminal(v.snapshot, returned.terminal_id);
      if (!isDeepStrictEqual(card, returned) || pairKeys.some(k => Object.hasOwn(card.tokens, k))) throw new Error('New card identity changed; launch evidence retained');
      return this.completeReservation(source, context, config, v, reserved, signal);
    }, { signal, waitMs: config.herdr_timeout_ms });
  }
  async verifyDisplay(source: Source, context: StreamsNativeContext, config: WorkCardsConfig, signal?: AbortSignal, preRead?: StreamsCardSnapshot) {
    const v = await this.marked(source, context, config, signal, preRead);
    if (v.pair.kind !== 'paired') throw new Error('Streams card is absent');
    return v.pair;
  }
  async closeDisplay(source: Source, context: StreamsNativeContext, config: WorkCardsConfig, signal?: AbortSignal): Promise<void> {
    await withWorkspaceLock(context.socket, context.workspaceId, async () => {
      const v = await this.marked(source, context, config, signal);
      if (v.pair.kind !== 'paired') return;
      const attempt = await this.evidence(v, context);
      await this.state(context).write({ ...attempt!, phase: 'closed' });
      const fresh = await this.marked(source, context, config, signal);
      if (!isDeepStrictEqual(fresh.pair, v.pair)) throw new Error('Pair changed before close; nothing closed');
      signal?.throwIfAborted();
      const pair = v.pair;
      const closed = await this.result(['plugin', 'pane', 'close', pair.card.pane_id], config, 'plugin_pane_closed', signal);
      const saved = await source.read(signal), after = await this.snapshot(config, signal), owner = locateTerminal(after, v.owner.terminal_id);
      if (closed.pane_id !== v.pair.card.pane_id || !sameStreamsScope(saved.scope, v.saved.scope) || !isDeepStrictEqual(saved.native, v.saved.native) || !isDeepStrictEqual(owner, v.owner)
        || after.panes.some(p => p.terminal_id === pair.card.terminal_id || p.pane_id === pair.card.pane_id)
        || after.workspaces.find(w => w.workspace_id === context.workspaceId)?.tokens.strm_binding !== this.bound(v, context)) throw new Error('Close acknowledgement/owner changed; effect may be partial and markers retained');
      // Keep exact owner markers as durable absent/suppression evidence. Never clear
      // another publisher's markers or detach the Session/workspace.
    }, { signal, waitMs: config.herdr_timeout_ms });
  }
  private summaryChanges(v: Verified, context: StreamsNativeContext, kind: 'workspace' | 'pane') {
    const target = kind === 'pane' ? v.owner : v.snapshot.workspaces.find(w => w.workspace_id === context.workspaceId)!;
    // Full narratives remain in the catalog/card, never in bounded native tokens.
    const values = kind === 'pane' ? { strm_status: v.saved.session.status, strm_state_short: compactToken(v.saved.session.state_short) }
      : { strm_goal_short: compactToken(v.saved.stream.goal_short) };
    return Object.fromEntries(Object.entries(values).filter(([k, value]) => (target.tokens[k] ?? null) !== value));
  }
  async projectSummaries(source: Source, context: StreamsNativeContext, config: WorkCardsConfig, signal?: AbortSignal): Promise<void> {
    const observed = await this.observeProjection(source, context, config, signal);
    if (observed.caller.terminal_id !== observed.owner.terminal_id) throw new Error('Only the owning Pi may publish Session progress');
    if (observed.snapshot.workspaces.find(w => w.workspace_id === context.workspaceId)!.tokens.strm_binding
      && !Object.keys(this.summaryChanges(observed, context, 'workspace')).length && !Object.keys(this.summaryChanges(observed, context, 'pane')).length) return;
    await withWorkspaceLock(context.socket, context.workspaceId, async () => {
      let v: Verified | undefined;
      for (const kind of ['workspace', 'pane'] as const) {
        v ??= await this.projectionProof(source, context, config, signal);
        if (v.caller.terminal_id !== v.owner.terminal_id) throw new Error('Only the owning Pi may publish Session progress');
        const target = kind === 'pane' ? v.owner : v.snapshot.workspaces.find(w => w.workspace_id === context.workspaceId)!;
        const changed = this.summaryChanges(v, context, kind);
        if (Object.keys(changed).length) {
          await this.stamp(source, context, target, changed, config, signal);
          v = undefined;
        }
      }
    }, { signal, waitMs: config.herdr_timeout_ms });
  }
  async projectCaption(source: Source, context: StreamsNativeContext, config: WorkCardsConfig, signal?: AbortSignal): Promise<void> {
    const observed = await this.marked(source, context, config, signal);
    if (observed.pair.kind !== 'paired' || observed.pair.card.tokens.strm_caption_manual === '1') return;
    const desired = this.title(observed.saved.stream.title);
    if (observed.pair.card.label === desired && observed.pair.card.label === (observed.pair.card.tokens.strm_caption ?? 'Streams work card')) return;
    await withWorkspaceLock(context.socket, context.workspaceId, async () => {
      const v = await this.marked(source, context, config, signal);
      if (v.pair.kind !== 'paired') return;
      const name = this.title(v.saved.stream.title);
      if (v.pair.card.tokens.strm_caption_manual === '1') return;
      const expected = v.pair.card.tokens.strm_caption ?? 'Streams work card';
      if (v.pair.card.label !== expected) {
        await this.stamp(source, context, v.pair.card, { strm_caption_manual: '1' }, config, signal); return;
      }
      if (v.pair.card.label === name) return;
      signal?.throwIfAborted();
      const renamed = nativePane((await this.result(['pane', 'rename', v.pair.card.pane_id, name], config, 'pane_info', signal)).pane);
      if (!isDeepStrictEqual(tuple(renamed), tuple(v.pair.card)) || renamed.label !== name) throw new Error('Caption acknowledgement tuple mismatch');
      const after = await this.marked(source, context, config, signal);
      if (after.pair.kind !== 'paired' || after.pair.card.label !== name || after.saved.stream.title !== name) throw new Error('Caption acknowledgement mismatch; effect may be partial');
      await this.stamp(source, context, after.pair.card, { strm_caption: name }, config, signal);
    }, { signal, waitMs: config.herdr_timeout_ms });
  }
  private title(name: string): string {
    if (!name.trim() || name.startsWith('-') || /[\p{Cc}\p{Cf}]/u.test(name)) throw new Error('Unsafe or blank native title');
    return name;
  }
  private async unchangedNames(source: Source, context: StreamsNativeContext, config: WorkCardsConfig, signal?: AbortSignal): Promise<Record<'workspace' | 'tab', string> | null> {
    const result = { workspace: 'disabled', tab: 'disabled' };
    if (!config.workspace_naming && !config.tab_naming) return result;
    const v = await this.observeProjection(source, context, config, signal);
    if (v.caller.terminal_id !== v.owner.terminal_id) throw new Error('Only owning Pi may name agent surfaces');
    if (!v.snapshot.workspaces.find(w => w.workspace_id === context.workspaceId)!.tokens.strm_binding) return null;
    const names = new NameStore(this.state(context).directory);
    for (const kind of ['workspace', 'tab'] as const) {
      if (!config[`${kind}_naming`]) continue;
      if (kind === 'tab' && !firstPane(v.snapshot, v.owner)) { result[kind] = 'not-first'; continue; }
      const surface = kind === 'workspace' ? v.snapshot.workspaces.find(w => w.workspace_id === context.workspaceId)! : v.snapshot.tabs.find(t => t.tab_id === context.tabId)!;
      const desired = this.title(kind === 'workspace' ? v.saved.stream.title : v.saved.session.title);
      const scope = await names.scope({ socket: context.socket, workspaceId: context.workspaceId, bindingNonce: this.bound(v, context), kind, surfaceId: kind === 'workspace' ? context.workspaceId : context.tabId });
      const prior = await names.read(scope);
      if (!prior) return null;
      const decision = decideName({ scope, provenance: prior.record,
        owner: { terminalId: v.owner.terminal_id, nonce: v.pair.kind === 'empty' ? v.saved.scope.nativeSessionId : v.pair.nonce },
        currentLabel: surface.label, desiredName: desired,
        defaultLabelProven: kind === 'tab' && surface.label === String(v.snapshot.tabs.filter(t => t.workspace_id === context.workspaceId).findIndex(t => t.tab_id === context.tabId) + 1) });
      if (decision.kind !== 'noop' && !(decision.kind === 'manual' && isDeepStrictEqual(prior.record, decision.provenance))) return null;
      result[kind] = decision.kind === 'noop' ? 'unchanged' : decision.kind;
    }
    signal?.throwIfAborted();
    return result;
  }
  async projectNames(source: Source, context: StreamsNativeContext, config: WorkCardsConfig, signal?: AbortSignal, intent: 'automatic' | 'explicit' | 'explicit-tab' | 'explicit-workspace' = 'automatic'): Promise<Record<'workspace' | 'tab', string>> {
    const unchanged = intent === 'automatic' ? await this.unchangedNames(source, context, config, signal) : null;
    if (unchanged) return unchanged;
    return withWorkspaceLock(context.socket, context.workspaceId, async () => {
      const result = { workspace: 'disabled', tab: 'disabled' };
      for (const kind of ['workspace', 'tab'] as const) {
        const kindIntent: 'automatic' | 'explicit' = intent === 'explicit' || (intent === 'explicit-workspace' && kind === 'workspace') || (intent === 'explicit-tab' && kind === 'tab') ? 'explicit' : 'automatic';
        if (kindIntent === 'automatic' && !config[`${kind}_naming`]) continue;
        const read = async () => {
          const v = await this.projectionProof(source, context, config, signal);
          if (v.caller.terminal_id !== v.owner.terminal_id) throw new Error('Only owning Pi may name agent surfaces');
          const surface = kind === 'workspace' ? v.snapshot.workspaces.find(w => w.workspace_id === context.workspaceId)! : v.snapshot.tabs.find(t => t.tab_id === context.tabId)!;
          return { v, surface, desired: this.title(kind === 'workspace' ? v.saved.stream.title : v.saved.session.title) };
        };
        const before = await read();
        if (kind === 'tab' && !firstPane(before.v.snapshot, before.v.owner)) { result[kind] = 'not-first'; continue; }
        const names = new NameStore(this.state(context).directory); await this.state(context).prepare();
        const scope = await names.scope({ socket: context.socket, workspaceId: context.workspaceId, bindingNonce: this.bound(before.v, context), kind, surfaceId: kind === 'workspace' ? context.workspaceId : context.tabId });
        const prior = await names.read(scope), owner = { terminalId: before.v.owner.terminal_id, nonce: before.v.pair.kind === 'empty' ? before.v.saved.scope.nativeSessionId : before.v.pair.nonce };
        const fresh = await read();
        if (fresh.desired !== before.desired || !isDeepStrictEqual(fresh.surface, before.surface) || !isDeepStrictEqual(fresh.v.owner, before.v.owner) || !isDeepStrictEqual(fresh.v.pair, before.v.pair)) throw new Error('Naming source/owner/label changed');
        const decision = decideName({ scope, intent: kindIntent, provenance: prior?.record ?? null, owner, currentLabel: before.surface.label, desiredName: before.desired,
          defaultLabelProven: kind === 'tab' && before.surface.label === String(before.v.snapshot.tabs.filter(t => t.workspace_id === context.workspaceId).findIndex(t => t.tab_id === context.tabId) + 1) });
        if (decision.kind === 'rename') {
          signal?.throwIfAborted();
          const response = await this.result([kind, 'rename', scope.surfaceId, decision.name], config, kind === 'workspace' ? 'workspace_info' : 'tab_info', signal);
          const ack = response[kind];
          if (!ack || ack[kind === 'workspace' ? 'workspace_id' : 'tab_id'] !== scope.surfaceId || ack.label !== decision.name
            || ack.workspace_id !== context.workspaceId || ack.number !== before.surface.number) throw new Error('Native name acknowledgement mismatch');
          const after = await read();
          if (after.desired !== before.desired || !isDeepStrictEqual(tuple(after.v.owner), tuple(before.v.owner)) || this.bound(after.v, context) !== scope.bindingNonce
            || after.surface.label !== decision.name || after.surface.number !== before.surface.number || (kind === 'tab' && !firstPane(after.v.snapshot, after.v.owner))) throw new Error('Name changed after native effect; provenance not published');
          await names.write(scope, decision.afterSuccess, prior?.revision ?? null, signal); result[kind] = 'renamed';
        } else if (decision.kind === 'adopt' || decision.kind === 'manual') {
          await names.write(scope, decision.kind === 'adopt' ? decision.afterSuccess : decision.provenance, prior?.revision ?? null, signal);
          result[kind] = decision.kind === 'adopt' ? 'unchanged' : 'manual';
        } else result[kind] = 'unchanged';
      }
      return result;
    }, { signal, waitMs: config.herdr_timeout_ms });
  }
  async openDocument(source: Source, context: StreamsNativeContext, painted: StreamsCardSnapshot, target: StreamsDocumentTarget, config: WorkCardsConfig, signal?: AbortSignal): Promise<void> {
    await withWorkspaceLock(context.socket, context.workspaceId, async () => {
      const verify = async () => {
        const v = await this.marked(source, context, config, signal);
        if (v.pair.kind !== 'paired' || !sameStreamsScope(v.saved.scope, painted.scope)) throw new Error('Document viewing scope/card changed');
        return { v, resolved: await resolveStreamsDocument(v.saved, target, { env: this.env, signal }) };
      };
      let current = await verify();
      const key = current.resolved.key, state = this.state(context), evidenceKey = streamsDigest([context.socket, context.workspaceId, 'editor', key]);
      const matching = (v: Verified) => {
        const matches = v.snapshot.panes.filter(p => p.workspace_id === context.workspaceId && p.tokens.strm_doc === key);
        if (matches.length > 1) throw new Error('Duplicate owned editor identities');
        const p = matches[0];
        if (p && (!p.tokens.strm_editor || p.tokens.strm_doc_pane !== p.pane_id || p.tokens.strm_doc_terminal !== p.terminal_id || p.tokens.strm_doc_tab !== p.tab_id || !firstPane(v.snapshot, p))) throw new Error('Owned editor tuple changed');
        return p;
      };
      const focus = async (pane: Pane) => {
        const evidence = await state.read(evidenceKey);
        const fresh = await verify(), p = matching(fresh.v);
        if (fresh.resolved.key !== key || !p || !isDeepStrictEqual(tuple(p), tuple(pane))) throw new Error('Editor changed before focus');
        if (!evidence || evidence.phase !== 'ready' || evidence.nonce !== p.tokens.strm_editor || evidence.paneId !== p.pane_id || evidence.terminalId !== p.terminal_id || evidence.tabId !== p.tab_id) throw new Error('Editor has no exact durable launch proof');
        signal?.throwIfAborted(); await this.run(['tab', 'focus', p.tab_id], config.herdr_timeout_ms, signal);
        const after = await verify();
        if (!matching(after.v) || !isDeepStrictEqual(tuple(matching(after.v)!), tuple(p))) throw new Error('Editor changed after focus; effect may be partial');
      };
      const existing = matching(current.v);
      if (existing) { await focus(existing); return; }
      const old = await state.read(evidenceKey);
      if (old?.phase === 'pending') throw new Error('Uncertain editor launch retained; no duplicate launched');
      if (old && current.v.snapshot.panes.some(p => p.terminal_id === old.terminalId || p.pane_id === old.paneId || p.tab_id === old.tabId)) throw new Error('Recorded editor resource is still live without verified ownership; no duplicate launched');
      await this.ensureStreamsPlugin(config, verify, signal);
      const command = documentCommand(config, this.env), root = await realpath(this.pluginRoot);
      current = await verify();
      if (matching(current.v)) throw new Error('Editor appeared during preparation; retry after redraw');
      const nonce = randomUUID(), pending: StreamsAttempt = { version: 1, key: evidenceKey, phase: 'pending', nonce };
      let effectStarted = false, opened;
      await state.write(pending);
      try {
        signal?.throwIfAborted(); effectStarted = true;
        opened = await this.result(['plugin', 'pane', 'open', '--plugin', pluginId, '--entrypoint', 'document', '--placement', 'tab', '--workspace', context.workspaceId, '--cwd', root, '--no-focus',
          '--env', `WORK_CARDS_DOCUMENT_COMMAND=${command}`, '--env', `WORK_CARDS_DOCUMENT_CWD=${current.resolved.root}`, '--env', `WORK_CARDS_DOCUMENT_PATH=${current.resolved.file}`,
          ...(this.env.PATH === undefined ? [] : ['--env', `PATH=${this.env.PATH}`])], config, 'plugin_pane_opened', signal);
      } catch (error) {
        if (!effectStarted) await state.discardUnlaunched(pending, old);
        throw error;
      }
      const info = object(opened.plugin_pane, 'editor launch'), returned = nativePane(info.pane);
      if (info.plugin_id !== pluginId || info.entrypoint !== 'document' || returned.workspace_id !== context.workspaceId
        || current.v.snapshot.panes.some(p => p.pane_id === returned.pane_id || p.terminal_id === returned.terminal_id || p.tab_id === returned.tab_id)) throw new Error('Unexpected/replayed editor launch; evidence retained');
      const after = await verify(), pane = locateTerminal(after.v.snapshot, returned.terminal_id);
      if (after.resolved.key !== key || !isDeepStrictEqual(pane, returned) || !firstPane(after.v.snapshot, pane)) throw new Error('Launched editor tuple changed; evidence retained');
      await this.stamp(source, context, pane, { strm_doc: key, strm_editor: nonce, strm_doc_pane: pane.pane_id, strm_doc_terminal: pane.terminal_id, strm_doc_tab: pane.tab_id }, config, signal);
      await state.write({ version: 1, key: evidenceKey, phase: 'ready', nonce, paneId: pane.pane_id, terminalId: pane.terminal_id, tabId: pane.tab_id });
      // Only this exact editor lifetime may establish its naming provenance.
      if (config.tab_naming) {
        const read = async () => {
          const fresh = await verify(), p = matching(fresh.v);
          if (!p || p.tokens.strm_editor !== nonce || !isDeepStrictEqual(tuple(p), tuple(pane))) throw new Error('Editor naming tuple changed');
          const tab = fresh.v.snapshot.tabs.find(t => t.tab_id === p.tab_id)!;
          return { fresh, p, tab };
        };
        const before = await read(), names = new NameStore(state.directory);
        const scope = await names.scope({ socket: context.socket, workspaceId: context.workspaceId,
          bindingNonce: this.bound(before.fresh.v, context), kind: 'tab', surfaceId: pane.tab_id });
        const prior = await names.read(scope), name = this.title(target.path);
        const currentNaming = await read();
        if (!isDeepStrictEqual(currentNaming.tab, before.tab) || !isDeepStrictEqual(currentNaming.p, before.p)) throw new Error('Editor naming source changed');
        const decision = decideName({ scope, provenance: prior?.record ?? null, owner: { terminalId: pane.terminal_id, nonce },
          currentLabel: before.tab.label, desiredName: name, defaultLabelProven: before.tab.label === String(before.fresh.v.snapshot.tabs.filter(t => t.workspace_id === context.workspaceId).findIndex(t => t.tab_id === pane.tab_id) + 1) });
        if (decision.kind === 'rename') {
          signal?.throwIfAborted();
          const ack = (await this.result(['tab', 'rename', pane.tab_id, name], config, 'tab_info', signal)).tab;
          const after = await read();
          if (!ack || ack.tab_id !== pane.tab_id || ack.workspace_id !== context.workspaceId || ack.number !== before.tab.number || ack.label !== name
            || after.tab.label !== name || after.tab.number !== before.tab.number) throw new Error('Editor name acknowledgement mismatch; provenance not published');
          await names.write(scope, decision.afterSuccess, prior?.revision ?? null, signal);
        } else if (decision.kind === 'manual') await names.write(scope, decision.provenance, prior?.revision ?? null, signal);
        else if (decision.kind === 'adopt') await names.write(scope, decision.afterSuccess, prior?.revision ?? null, signal);
      }
      await focus(pane);
    }, { signal, waitMs: config.herdr_timeout_ms });
  }
}
