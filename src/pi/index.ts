import { createHash } from 'node:crypto';
import { dirname, join, resolve } from 'node:path';
import type { ExtensionAPI, ExtensionContext } from '@earendil-works/pi-coding-agent';
import { Type } from 'typebox';
import { cardDirectory, createCard, readCard, updateCard, type CardSnapshot } from '../card.js';
import { loadConfig, type WorkCardsConfig } from '../config.js';
import { attachCard, detachCard } from '../herdr/client.js';
import { createCheckScheduler, type CheckScheduler } from './scheduler.js';
import { buildSummaryContext, requestSummary } from './summarizer.js';
import { normalizeEvidence, type ProjectedEvidence } from './evidence.js';

const bindingEntry = 'work-cards.binding';
const usage = 'Usage: /work-card new <title> | open [slug] | refresh | show | detach';
const documentGuidance = "Substantive plans and ADRs belong in the attached card's project checkout (cwd); card metadata only references them. This tool does not create documents.";
interface Dependencies {
  cardDirectory: typeof cardDirectory;
  loadConfig: typeof loadConfig;
  attachCard: (snapshot: CardSnapshot, config: WorkCardsConfig) => Promise<unknown>;
  detachCard: typeof detachCard;
  env: NodeJS.ProcessEnv;
}
interface Binding {
  readonly target: Readonly<Pick<CardSnapshot, 'slug' | 'path'>>;
  readonly sessionId: string;
  updates: boolean;
}
interface Evidence {
  revision: string;
  generation: number;
  sessionId: string;
  path: string;
  input: ProjectedEvidence;
}
function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}
function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Dependency seam is limited to local host boundaries; card persistence stays in the core. */
export function registerWorkCards(pi: ExtensionAPI, overrides: Partial<Dependencies> = {}): void {
  const deps: Dependencies = { cardDirectory, loadConfig, attachCard, detachCard, env: process.env, ...overrides };
  let generation = 0;
  let binding: Binding | undefined;
  let config: WorkCardsConfig | undefined;
  let scheduler: CheckScheduler<Evidence> | undefined;
  let hasTextualEvidence = false;
  let runtime: {
    sessionId: string;
    registry: ExtensionContext['modelRegistry'];
    notify: ExtensionContext['ui']['notify'];
    mode: ExtensionContext['mode'];
    hasUI: boolean;
    ui: ExtensionContext['ui'];
  } | undefined;
  let requestFinished: Promise<void> | undefined;
  const warnings = new Set<string>();
  let stopActivity: (() => void) | undefined;

  function startActivity(): () => void {
    if (!config?.show_refresh_spinner || runtime?.mode !== 'tui' || !runtime.hasUI) return () => {};
    const ui = runtime.ui;
    const frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    let frame = 0;
    const render = () => ui.setStatus('work-cards.refresh', `${frames[frame++ % frames.length]} work-card`);
    render();
    const timer = setInterval(render, 80);
    timer.unref();
    const stop = () => {
      if (stopActivity !== stop) return;
      clearInterval(timer);
      ui.setStatus('work-cards.refresh', undefined);
      stopActivity = undefined;
    };
    stopActivity = stop;
    return stop;
  }

  function invalidate(): number {
    generation++;
    hasTextualEvidence = false;
    scheduler?.reset();
    stopActivity?.();
    runtime = undefined;
    return generation;
  }
  function adopt(ctx: ExtensionContext): void {
    runtime = {
      sessionId: ctx.sessionManager.getSessionId(),
      registry: ctx.modelRegistry,
      notify: ctx.ui.notify.bind(ctx.ui),
      mode: ctx.mode, hasUI: ctx.hasUI, ui: ctx.ui,
    };
  }
  function warn(error: unknown): void {
    const text = `Work cards: ${errorText(error)}`;
    if (!runtime || warnings.has(text)) return;
    warnings.add(text);
    runtime.notify(text, 'warning');
  }
  function applicable(evidence: Evidence, signal: AbortSignal): boolean {
    return !signal.aborted && generation === evidence.generation && binding?.updates === true &&
      binding.sessionId === evidence.sessionId && binding.target.path === evidence.path &&
      runtime?.sessionId === evidence.sessionId;
  }
  async function check(evidence: Evidence, signal: AbortSignal): Promise<void> {
    // A reconfigured scheduler must also wait for a retired scheduler's request to exit.
    if (requestFinished) await requestFinished;
    if (!applicable(evidence, signal) || !config || !runtime || !binding) return;
    let finish!: () => void;
    requestFinished = new Promise<void>(resolve => { finish = resolve; });
    const registry = runtime.registry;
    const settings = { ...config };
    const target = binding.target;
    try {
      const current = await readCard(target.slug, dirname(target.path));
      if (!applicable(evidence, signal)) return;
      const context = buildSummaryContext(current.card, evidence.input, settings);
      const decision = await requestSummary(registry, context, settings, signal, undefined, startActivity);
      if (!applicable(evidence, signal) || decision.action === 'no_change') return;
      // The core also checks cancellation immediately before committing its prepared file.
      await updateCard(target.slug, decision.patch, current.revision, dirname(target.path), signal);
    } finally {
      finish();
      requestFinished = undefined;
    }
  }
  async function configure(token: number): Promise<number | undefined> {
    try {
      const next = await deps.loadConfig();
      if (generation !== token) return;
      const changed = !config || JSON.stringify(config) !== JSON.stringify(next);
      if (changed && config) {
        // Configuration changes invalidate old requests without replacing the live host context.
        generation++;
        stopActivity?.();
        token = generation;
      }
      config = next;
      if (changed || !scheduler) {
        hasTextualEvidence = false;
        scheduler?.reset();
        scheduler = createCheckScheduler<Evidence>({ ...next, check, onError: (error, evidence) => {
          if (generation === evidence.generation) warn(error);
        } });
      }
      return token;
    } catch (error) {
      if (generation === token) {
        config = undefined;
        stopActivity?.();
        scheduler?.reset();
        scheduler = undefined;
        warn(error);
      }
      return;
    }
  }
  async function restore(ctx: ExtensionContext): Promise<void> {
    let token = invalidate();
    binding = undefined;
    warnings.clear();
    adopt(ctx);
    // Capture branch and session data before any await; lifecycle can replace ctx.
    const sessionId = runtime!.sessionId;
    const directory = resolve(deps.cardDirectory());
    const last = ctx.sessionManager.getBranch().filter(entry => entry.type === 'custom' && entry.customType === bindingEntry).at(-1);
    const data = last?.type === 'custom' ? last.data : undefined;
    const configured = await configure(token);
    if (configured !== undefined) token = configured;
    if (generation !== token || !object(data) || data.sessionId !== sessionId || data.slug === null) return;
    if (typeof data.slug !== 'string' || typeof data.path !== 'string' ||
        data.path !== join(directory, `${data.slug}.yaml`)) {
      warn('Saved binding does not identify a card in the current state directory; use /work-card open <slug> explicitly.');
      return;
    }
    try {
      const snapshot = await readCard(data.slug, directory);
      if (generation === token && snapshot.path === data.path) {
        binding = { target: { slug: snapshot.slug, path: snapshot.path }, sessionId, updates: !Object.hasOwn(data, 'updates') || data.updates === true };
      }
    } catch (error) { if (generation === token) warn(error); }
    // Restoring metadata never attaches or focuses a pane.
  }
  function evidence(ctx: ExtensionContext, allowEmpty = false): Evidence | undefined {
    if (!binding?.updates || !config || !scheduler) return;
    const sessionId = ctx.sessionManager.getSessionId();
    if (sessionId !== binding.sessionId) return;
    adopt(ctx);
    const projection = ctx.sessionManager.buildSessionProjection();
    const input = normalizeEvidence(projection.messages);
    const nonempty = input.compactionSummary !== null || input.transcript.length > 0;
    // Empty initial input is not dialogue; later redaction to empty is an input change.
    if (!allowEmpty && !nonempty && !hasTextualEvidence) return;
    hasTextualEvidence ||= nonempty;
    return {
      revision: createHash('sha256').update(JSON.stringify(input)).digest('hex'),
      generation, sessionId, path: binding.target.path, input,
    };
  }
  function reportPartial(snapshot: CardSnapshot, reason: string): void {
    const text = `Work cards: card file exists at ${snapshot.path}, but attachment is partial: ${reason}. Inspect Herdr, then /work-card open ${snapshot.slug}.`;
    if (runtime) runtime.notify(text, 'warning');
    else process.stderr.write(`${text}\n`);
  }
  function requireHerdr(): void {
    if (deps.env.HERDR_ENV !== '1' || !deps.env.HERDR_PANE_ID || !deps.env.HERDR_SOCKET_PATH) {
      throw new Error('Run /work-card new or open in a Herdr-managed pane (HERDR_ENV, HERDR_PANE_ID and HERDR_SOCKET_PATH are required). No GUI fallback is used.');
    }
  }

  pi.on('session_start', async (_event, ctx) => restore(ctx));
  pi.on('session_tree', async (_event, ctx) => restore(ctx));
  pi.on('session_before_tree', () => { invalidate(); });
  pi.on('session_before_switch', () => { invalidate(); });
  pi.on('session_before_fork', () => { invalidate(); });
  pi.on('session_shutdown', () => { invalidate(); });
  // context runs after delivered user messages are persisted; turn_start/message_end do not.
  // Read the SDK projection, never append raw event text (later handlers may redact it).
  pi.on('context', (_event, ctx) => {
    const snapshot = evidence(ctx);
    if (snapshot) scheduler!.observe(snapshot);
  });
  pi.on('turn_end', (event, ctx) => {
    if (event.outcome !== 'completed') return;
    const snapshot = evidence(ctx);
    if (snapshot) scheduler!.observe(snapshot);
  });
  pi.on('agent_settled', (event, ctx) => {
    const snapshot = evidence(ctx);
    if (snapshot) scheduler!.onSettled(snapshot, event.aborted);
  });

  pi.registerCommand('work-card', {
    description: 'Explicitly create/open, refresh, show, or detach a durable work card',
    async handler(args, ctx) {
      const parsed = /^(\S+)(?:[ \t]+([\s\S]*))?$/.exec(args.trim());
      const command = parsed?.[1];
      const value = parsed?.[2] ?? '';
      let token = command === 'show' || command === 'refresh' ? generation : invalidate();
      adopt(ctx);
      const sessionId = runtime!.sessionId;
      if (command === 'detach' && !value && binding?.sessionId === sessionId) {
        binding.updates = false;
        pi.appendEntry(bindingEntry, { slug: binding.target.slug, sessionId, path: binding.target.path, updates: false });
      }
      const cwd = ctx.cwd;
      const directory = resolve(deps.cardDirectory());
      const configured = await configure(token);
      if (configured === undefined) return;
      token = configured;
      if (generation !== token) return;
      const settings = { ...config! };
      let created: CardSnapshot | undefined;
      try {
        if (command === 'refresh' && !value) {
          if (!binding || binding.sessionId !== sessionId) {
            runtime!.notify('No work card attached. Use /work-card open <slug> explicitly.', 'info');
            return;
          }
          if (!binding.updates) {
            runtime!.notify('Work card updates are paused. Use /work-card open explicitly to resume.', 'info');
            return;
          }
          warnings.clear();
          const snapshot = evidence(ctx, true);
          if (snapshot) scheduler!.refresh(snapshot);
        } else if (command === 'show' && !value) {
          if (!binding || binding.sessionId !== sessionId) {
            runtime!.notify('No work card attached. Use /work-card new <title> or open <slug>.', 'info');
            return;
          }
          const snapshot = await readCard(binding.target.slug, dirname(binding.target.path));
          if (generation === token) {
            runtime!.notify(`${snapshot.path}\n${JSON.stringify(snapshot.card, null, 2)}`, 'info');
          }
        } else if (command === 'detach' && !value) {
          if (!binding || binding.sessionId !== sessionId) {
            runtime!.notify('No work card attached.', 'info');
            return;
          }
          await deps.detachCard({ path: binding.target.path }, settings);
          if (generation !== token) return;
          binding = undefined;
          pi.appendEntry(bindingEntry, { slug: null, sessionId });
          runtime!.notify('Work card detached.', 'info');
        } else if ((command === 'new' && value) || (command === 'open' && !/\s/.test(value))) {
          const savedBinding = command === 'open' && !value ? binding : undefined;
          if (command === 'open' && !value && savedBinding?.sessionId !== sessionId) {
            runtime!.notify('No work card bound to this session. Use /work-card open <slug> or /work-card new <title>.', 'info');
            return;
          }
          requireHerdr();
          const snapshot = command === 'new'
            ? created = await createCard({ title: value, status: 'active', state: '', cwd, documents: [], issues: [], prs: [] }, directory)
            : savedBinding ? await readCard(savedBinding.target.slug, dirname(savedBinding.target.path)) : await readCard(value, directory);
          if (generation !== token) {
            if (created) reportPartial(created, 'session or binding changed before attachment');
            return;
          }
          await deps.attachCard(snapshot, settings);
          if (generation !== token) {
            reportPartial(snapshot, 'session or binding changed while Herdr was attaching; no session binding was saved');
            return;
          }
          binding = { target: { slug: snapshot.slug, path: snapshot.path }, sessionId, updates: true };
          pi.appendEntry(bindingEntry, { slug: snapshot.slug, sessionId, path: snapshot.path, updates: true });
          runtime!.notify(`Attached work card ${snapshot.slug}: ${snapshot.path}`, 'info');
          if (command === 'open') {
            warnings.clear();
            const snapshot = evidence(ctx, true);
            if (snapshot) scheduler!.refresh(snapshot);
          }
        } else {
          runtime!.notify(usage, 'error');
        }
      } catch (error) {
        if (generation !== token) {
          if (created) reportPartial(created, errorText(error));
          return;
        }
        const partial = created ? `Card file created at ${created.path}, but attachment is partial/failed. Inspect Herdr, then /work-card open ${created.slug}. ` : '';
        const cleanup = command === 'detach' ? 'Automatic updates stopped; binding retained so you can retry /work-card detach after inspecting Herdr. ' : '';
        runtime!.notify(`Work cards: ${partial}${cleanup}${errorText(error)}`, 'error');
      }
    },
  });

  pi.registerTool({
    name: 'work_card',
    label: 'Work card (read-only)',
    description: `Read-only: return the currently attached work card, its source path and document references. Never creates, selects, updates or detaches a card. ${documentGuidance}`,
    parameters: Type.Object({}, { additionalProperties: false }),
    annotations: { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    async execute(_id, _params, signal, _onUpdate, ctx) {
      const token = generation;
      const sessionId = ctx.sessionManager.getSessionId();
      const currentBinding = binding;
      if (!currentBinding || currentBinding.sessionId !== sessionId) {
        return { content: [{ type: 'text', text: 'No work card attached.' }], details: null };
      }
      signal?.throwIfAborted();
      const snapshot = await readCard(currentBinding.target.slug, dirname(currentBinding.target.path));
      signal?.throwIfAborted();
      if (generation !== token || binding !== currentBinding) throw new Error('Work-card binding changed during read; call work_card again.');
      return { content: [{ type: 'text', text: JSON.stringify({ ...snapshot, documents: snapshot.card.documents, guidance: documentGuidance }) }], details: snapshot };
    },
  });
}

export default function workCards(pi: ExtensionAPI): void {
  registerWorkCards(pi);
}
