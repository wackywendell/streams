import { createHash } from 'node:crypto';
import { open } from 'node:fs/promises';
import { resolve } from 'node:path';
import { parseSessionEntries, type ExtensionAPI, type ExtensionContext } from '@earendil-works/pi-coding-agent';
import { Type } from 'typebox';
import { loadConfig, type WorkCardsConfig } from '../config.js';
import { RecordStore, recordDirectory, validateMembership, type Authority } from '../records.js';
import { composeView, resolvedReferences, selectReference } from '../presentation.js';
import { WorkspaceClient, agentAuthority, type NativeContext } from '../herdr/workspace-client.js';
import { createCheckScheduler, type CheckScheduler } from './scheduler.js';
import { normalizeEvidence } from './evidence.js';
import { WorkspaceLifecycle, type WorkspaceScope, type WorkspaceSession } from './workspace-lifecycle.js';
import { WorkspaceRefresher } from './workspace-refresh.js';
import type { SummaryRegistry } from './summarizer.js';

const stateEntry = 'work-cards.workspace';
const usage = 'Usage: /work-card new <title> | open [workspace-id] | refresh | show | detach | remove <substring>';
const documentGuidance = "Substantive plans and ADRs belong in the attached card's project checkout (cwd); card metadata only references documents. This tool does not create documents.";
interface Dependencies {
  recordDirectory: typeof recordDirectory;
  loadConfig: typeof loadConfig;
  client: WorkspaceClient;
  env: NodeJS.ProcessEnv;
}
interface Evidence {
  revision: string;
  generation: number;
  session: WorkspaceSession;
  scope?: WorkspaceScope;
  authority: Authority;
}
function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}
function errorText(error: unknown): string { return error instanceof Error ? error.message : String(error); }
function cleanupContext(value: unknown): NativeContext | undefined {
  if (!object(value) || !['socket', 'terminalId', 'workspaceId', 'tabId'].every(k => typeof value[k] === 'string' && value[k])) return;
  if (value.bindingNonce !== undefined && typeof value.bindingNonce !== 'string') return;
  return { socket: value.socket as string, terminalId: value.terminalId as string, workspaceId: value.workspaceId as string, tabId: value.tabId as string,
    ...(value.bindingNonce === undefined ? {} : { bindingNonce: value.bindingNonce as string }) };
}

/** Host seams are local configuration, state location and the native adapter. Persistence stays real. */
export function registerWorkCards(pi: ExtensionAPI, overrides: Partial<Dependencies> = {}): void {
  const env = overrides.env ?? process.env;
  const deps: Dependencies = { recordDirectory, loadConfig, client: new WorkspaceClient({ env }), env, ...overrides };
  let store: RecordStore;
  let session: WorkspaceSession;
  let ctx: ExtensionContext;
  let lifecycle: WorkspaceLifecycle | undefined;
  let config: WorkCardsConfig | undefined;
  let scheduler: CheckScheduler<Evidence> | undefined;
  let refresher: WorkspaceRefresher<{ api?: string }> | undefined;
  let epoch = 0;
  let generation = 0, updates = true, disposed = false, recovering = false, started = false;
  let cancellation = new AbortController();
  let savedCleanup: NativeContext | undefined;
  let savedWorkspaceId: string | undefined;
  let poll: ReturnType<typeof setTimeout> | undefined;
  let stopActivity: (() => void) | undefined;
  let tail: Promise<unknown> = Promise.resolve();
  let requestFinished: Promise<void> | undefined;
  let providerFinished: Promise<void> = Promise.resolve();
  const warnings = new Set<string>();
  const inHerdr = () => deps.env.HERDR_ENV === '1';
  const stopPoll = () => { if (poll) clearTimeout(poll); poll = undefined; };
  function cancel(): void {
    generation++;
    cancellation.abort(); cancellation = new AbortController();
    scheduler?.reset(); refresher = undefined;
    stopActivity?.(); stopPoll();
  }
  function warn(error: unknown): void {
    const text = `Work cards: ${errorText(error)}`;
    if (warnings.has(text) || !ctx) return;
    warnings.add(text); ctx.ui.notify(text, 'warning');
  }
  function live(token = generation): void {
    if (disposed || token !== generation) throw new Error('Stale work-card host context');
  }
  function capture(context: ExtensionContext): WorkspaceSession {
    const sessionId = context.sessionManager.getSessionId(), sessionFile = context.sessionManager.getSessionFile();
    return { sessionId, ...(sessionFile ? { sessionFile } : {}), cwd: context.cwd,
      evidence: normalizeEvidence(context.sessionManager.buildSessionProjection().messages) };
  }
  function accept(context: ExtensionContext): boolean {
    if (!started || disposed) return false;
    const next = capture(context);
    if (next.sessionId !== session.sessionId) { lifecycle?.invalidate(); cancel(); return false; }
    session = next; ctx = context; return true;
  }
  function enqueue<T>(operation: () => Promise<T>): Promise<T> {
    const result = tail.catch(() => {}).then(operation); tail = result; return result;
  }
  function activity(): () => void {
    if (!config?.show_refresh_spinner || ctx.mode !== 'tui' || !ctx.hasUI) return () => {};
    const ui = ctx.ui, frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏']; let frame = 0;
    const render = () => ui.setStatus('work-cards.refresh', `${frames[frame++ % frames.length]} work-card`);
    render(); const timer = setInterval(render, 80); timer.unref();
    const stop = () => { clearInterval(timer); if (stopActivity === stop) { ui.setStatus('work-cards.refresh', undefined); stopActivity = undefined; } };
    stopActivity = stop; return stop;
  }
  async function readOwn() {
    try { return await store.readAgent(session.sessionId); }
    catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; return; }
  }
  async function assertCurrent(evidence: Evidence, signal: AbortSignal): Promise<void> {
    signal.throwIfAborted(); live(evidence.generation);
    if (!updates || session.sessionId !== evidence.session.sessionId) throw new Error('Work-card updates paused or session changed');
    if (evidence.scope) await lifecycle!.assertCurrent(evidence.scope);
    else {
      // Offline authority never follows a failed native probe or changes membership.
      if (inHerdr()) throw new Error('Native membership authority is required inside Herdr');
      const agent = await store.readAgent(evidence.authority.sessionId);
      if (JSON.stringify(agentAuthority(agent.record)) !== JSON.stringify(evidence.authority)) throw new Error('Stale durable workspace authority');
    }
    signal.throwIfAborted(); live(evidence.generation);
  }
  async function check(evidence: Evidence, signal: AbortSignal, forced: boolean): Promise<void> {
    // Keep this gate across scheduler replacement: an aborted provider may settle late.
    if (requestFinished) await requestFinished;
    if (signal.aborted || disposed || evidence.generation !== generation || !config || !updates) return;
    let finish!: () => void;
    const gate = requestFinished = new Promise<void>(resolve => { finish = resolve; });
    try {
      await assertCurrent(evidence, signal);
      if (!refresher) {
        const original = ctx.modelRegistry;
        const registry: SummaryRegistry<{ api?: string }> = {
          find: (provider, modelId) => original.find(provider, modelId),
          streamSimple(model, context, options) {
            const result = original.streamSimple(model as Parameters<typeof original.streamSimple>[0], context, options).result();
            // Consume late rejection and retain transport settlement, not just abortable processing.
            providerFinished = result.then(() => {}, () => {});
            return { result: () => result };
          },
        };
        refresher = new WorkspaceRefresher({ store, registry, options: { ...config } });
      }
      await refresher.refresh({ authority: evidence.authority, evidence: evidence.session.evidence, signal,
        assertCurrent: () => assertCurrent(evidence, signal), force: forced, onRequest: activity });
    } finally {
      void providerFinished.then(() => { finish(); if (requestFinished === gate) requestFinished = undefined; });
      schedulePoll();
    }
  }
  async function configure(): Promise<void> {
    const token = generation;
    try {
      const next = await deps.loadConfig(); live(token);
      const changed = !config || JSON.stringify(config) !== JSON.stringify(next);
      if (changed) {
        lifecycle?.invalidate(); cancel(); config = next;
        scheduler = createCheckScheduler<Evidence>({ ...next, check, onError: (error, evidence) => { if (!disposed && evidence.generation === generation) warn(error); } });
        if (lifecycle) await lifecycle.restore();
      }
    } catch (error) {
      if (!disposed && token === generation) { lifecycle?.invalidate(); cancel(); config = undefined; scheduler = undefined; }
      throw error;
    }
  }
  function save(workspaceId: string | undefined, cleanup = savedCleanup): void {
    savedWorkspaceId = workspaceId;
    pi.appendEntry(stateEntry, { sessionId: session.sessionId, store: store.directory, ...(workspaceId ? { workspaceId } : {}), updates,
      ...(cleanup ? { cleanup } : {}) });
  }
  function schedulePoll(): void {
    stopPoll();
    if (disposed || !started || recovering || !updates || !config || !inHerdr()) return;
    poll = setTimeout(() => {
      poll = undefined;
      void enqueue(async () => {
        if (disposed || recovering || !updates || !config) return;
        try {
          if (!accept(ctx)) return;
          await lifecycle!.reconcile();
        } catch (error) { if (!disposed) warn(error); }
        finally { schedulePoll(); }
      });
    }, config.pane_poll_interval_ms);
    poll.unref();
  }
  async function reconcile(): Promise<void> {
    if (recovering) { await lifecycle!.restore(); live(); recovering = false; }
    if (updates && inHerdr()) await lifecycle!.reconcile();
  }
  async function snapshot(): Promise<Evidence | undefined> {
    if (!updates || !config || !scheduler) return;
    const token = generation, input = structuredClone(session), scope = inHerdr() ? lifecycle?.captureScope() : undefined;
    if (inHerdr() && !scope) return;
    const agent = await readOwn(); live(token); if (!agent) return;
    return { revision: createHash('sha256').update(JSON.stringify(input.evidence)).digest('hex'), generation: token, session: input,
      authority: agentAuthority(agent.record), ...(scope ? { scope } : {}) };
  }
  async function observe(mode: 'observe' | 'settled' | 'refresh', aborted = false): Promise<void> {
    await reconcile();
    const evidence = await snapshot();
    if (evidence) {
      if (mode === 'refresh') scheduler!.refresh(evidence);
      else if (mode === 'settled') scheduler!.onSettled(evidence, aborted);
      else scheduler!.observe(evidence);
    }
    schedulePoll();
  }
  async function start(context: ExtensionContext, tree = false): Promise<void> {
    if (disposed) return;
    // All startup identity, projection and pause state are captured before configuration I/O.
    const initial = capture(context), startEpoch = ++epoch;
    const entries = tree ? [] : context.sessionManager.getEntries().filter(entry => entry.type === 'custom' && entry.customType === stateEntry);
    const saved = entries.filter(entry => entry.type === 'custom' && object(entry.data) && entry.data.sessionId === initial.sessionId).at(-1);
    const data = saved?.type === 'custom' && object(saved.data) ? saved.data : undefined;
    const directory = resolve(deps.recordDirectory(deps.env));
    const sameIdentity = session?.sessionId === initial.sessionId && store?.directory === directory;
    const keepPause = tree && sameIdentity && !updates;
    lifecycle?.dispose(); cancel(); lifecycle = undefined;
    session = initial; ctx = context; store = new RecordStore(directory); started = true; recovering = false;
    updates = keepPause || (data && (data.store !== directory || data.updates === false)) ? false : true;
    if (!tree || !sameIdentity) {
      savedCleanup = data?.store === directory ? cleanupContext(data.cleanup) : undefined;
      savedWorkspaceId = data?.store === directory && typeof data.workspaceId === 'string' ? data.workspaceId : undefined;
    }
    const token = generation;
    await enqueue(async () => {
      live(token);
      await configure(); live(); if (epoch !== startEpoch) return;
      lifecycle = new WorkspaceLifecycle({ store, client: deps.client, get config() { return config!; }, get policy() { return config!.workspace_startup; }, current: () => session, onInvalidate: cancel });
      const latest = session;
      session = initial;
      const restored = lifecycle.restore(); // restore captures the startup baseline synchronously.
      session = latest;
      await restored; live();
      if (data && data.store !== directory) warn('Saved workspace state belongs to another state root; updates paused. Use /work-card open explicitly.');
      if (updates && inHerdr()) await lifecycle.reconcile();
      schedulePoll();
    });
  }
  function before(): void {
    if (disposed || !started) return;
    epoch++; recovering = true; lifecycle?.invalidate(); cancel();
  }
  function notification(context: ExtensionContext, mode: 'observe' | 'settled', aborted = false): void {
    if (!accept(context)) return;
    const observedEpoch = epoch;
    void enqueue(async () => {
      if (disposed || observedEpoch !== epoch || !config) return;
      try { await observe(mode, aborted); } catch (error) { if (!disposed) warn(error); }
    });
  }
  pi.on('session_start', async (_event, context) => { try { await start(context); } catch (error) { if (!disposed) warn(error); } });
  pi.on('session_tree', async (_event, context) => { try { await start(context, true); } catch (error) { if (!disposed) warn(error); } });
  pi.on('session_before_tree', before); pi.on('session_before_switch', before); pi.on('session_before_fork', before);
  pi.on('session_shutdown', async event => {
    if (disposed) return;
    disposed = true; epoch++; lifecycle?.invalidate(); cancel();
    try {
      let departure = event.reason === 'new' || event.reason === 'fork';
      if (event.reason === 'resume') {
        if (!event.targetSessionFile) throw new Error('Missing destination session identity; own display retained');
        // Inspect only the documented SDK header; opening a manager can migrate/rewrite history.
        const file = await open(event.targetSessionFile, 'r');
        let firstLine = '';
        try {
          for await (const line of file.readLines({ autoClose: false })) { firstLine = line; break; }
        } finally { await file.close(); }
        const header = parseSessionEntries(firstLine)[0];
        if (!header || header.type !== 'session' || typeof header.id !== 'string' || !header.id ||
            typeof header.cwd !== 'string' || typeof header.timestamp !== 'string') throw new Error('Invalid destination session header; own display retained');
        departure = header.id !== session.sessionId;
      }
      await tail.catch(() => {});
      if (lifecycle && inHerdr() && departure) await lifecycle.retire(savedCleanup);
    } catch (error) { warn(`Own display cleanup failed during committed departure: ${errorText(error)}. Inspect Herdr for explicit recovery; the old extension is disposed and cannot retry.`); }
    finally { lifecycle?.dispose(); cancel(); }
  });
  pi.on('context', (_event, context) => { notification(context, 'observe'); });
  pi.on('turn_end', (event, context) => { if (event.outcome === 'completed') notification(context, 'observe'); });
  pi.on('agent_settled', (event, context) => { notification(context, 'settled', event.aborted); });

  async function current(context: ExtensionContext) {
    if (!started || disposed || context.sessionManager.getSessionId() !== session.sessionId) return null;
    const token = generation, agent = await readOwn(); live(token); if (!agent) return null;
    const workspace = await store.readWorkspace(agent.record.workspaceId); live(token); validateMembership(workspace.record, agent.record);
    return { workspace, agent, references: resolvedReferences(workspace.record), view: composeView(workspace, agent), updates };
  }
  pi.registerCommand('work-card', {
    description: 'Open workspace context, set own title, refresh, show, detach, or remove a reference',
    async handler(args, context) {
      if (!accept(context)) return;
      const parsed = /^(\S+)(?:[ \t]+([\s\S]*))?$/.exec(args.trim());
      const command = parsed?.[1], value = parsed?.[2] ?? '';
      // Detach supersedes queued resume intents before capturing its own continuation.
      if (command === 'detach' && !value) epoch++;
      const commandEpoch = epoch;
      const assertHost = () => { live(); if (epoch !== commandEpoch) throw new Error('Stale work-card command context'); };
      try {
        if (command === 'detach' && !value) {
          // Pause is persisted synchronously before native cleanup or configuration can fail.
          updates = false; savedCleanup = lifecycle?.cleanupContext() ?? savedCleanup;
          lifecycle?.invalidate(); cancel();
          save(lifecycle?.state.agent?.record.workspaceId ?? savedWorkspaceId);
        }
        await enqueue(async () => {
          assertHost();
          if (command === 'show' && !value) { context.ui.notify(JSON.stringify(await current(context), null, 2), 'info'); return; }
          if (!lifecycle) throw new Error('Work-card startup did not complete. Fix configuration, run /reload, then retry.');
          await configure(); assertHost();
          if (command === 'detach' && !value) {
            if (inHerdr()) await lifecycle?.retire(savedCleanup);
            context.ui.notify('Work-card updates paused; /work-card open resumes them.', 'info'); return;
          }
          if (command === 'refresh' && !value) {
            if (!updates) throw new Error('Work-card updates paused; use /work-card open to resume.');
            await observe('refresh'); return;
          }
          if ((command === 'new' && value.trim()) || command === 'open') {
            if (!inHerdr()) throw new Error('Explicit open/new requires a Herdr-managed pane; no native guesses or offline rebinding.');
            lifecycle!.invalidate();
            if (!updates || recovering) { await lifecycle!.restore(); recovering = false; }
            await lifecycle!.open(command === 'new' ? value : undefined, command === 'open' && value ? value : undefined);
            live();
            if (command === 'new') {
              const scope = lifecycle!.captureScope(); if (!scope) throw new Error('No current own membership');
              const member = await lifecycle!.assertCurrent(scope);
              await store.setOwnTitle(scope.authority, value, member.agent.revision, scope.signal);
            }
            updates = true; savedCleanup = lifecycle!.cleanupContext();
            const agent = await readOwn(); live(); if (agent) save(agent.record.workspaceId);
            const evidence = await snapshot(); if (evidence) scheduler!.refresh(evidence);
            schedulePoll(); return;
          }
          if (command === 'remove' && value.length) {
            await reconcile();
            const evidence = await snapshot(); if (!evidence) throw new Error('No current workspace authority; open first.');
            const workspace = await store.readWorkspace(evidence.authority.workspaceId), selection = selectReference(workspace, value);
            if (selection.kind !== 'unique') throw new Error(selection.kind === 'ambiguous' ? `Ambiguous reference: ${selection.candidates.map(c => c.target).join(', ')}` : 'Reference not found.');
            await assertCurrent(evidence, cancellation.signal);
            await store.removeReference(evidence.authority, selection.selection, cancellation.signal); return;
          }
          context.ui.notify(usage, 'info');
        });
      } catch (error) { if (!disposed) warn(error); }
    },
  });
  pi.registerTool({
    name: 'work_card', label: 'Work card', description: `Read-only: return the current workspace and own agent records, source paths and resolved references. Never creates, selects, opens or joins. ${documentGuidance}`,
    parameters: Type.Object({}),
    async execute(_id, _params, _signal, _onUpdate, context) {
      const details = await current(context);
      return { content: [{ type: 'text', text: `${details ? JSON.stringify(details, null, 2) : 'No workspace/agent record for this Pi session.'}\n\n${documentGuidance}` }], details };
    },
  });
}
export default function workCards(pi: ExtensionAPI): void {
  registerWorkCards(pi);
}
