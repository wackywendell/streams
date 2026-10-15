import { dirname, isAbsolute } from 'node:path';
import { pathToFileURL } from 'node:url';
import { configPath, loadConfig } from '../config.js';
import { HerdrClient, type Attachment } from './client.js';
import { CardView, KeyInput, plain } from './view.js';
import { absolute, RecordStore, text, type AgentRecord, type Snapshot } from '../records.js';
import { WorkspaceClient, type NativeContext } from './workspace-client.js';
import { WorkspaceCardView } from './workspace-view.js';

export async function runPane(env: NodeJS.ProcessEnv = process.env): Promise<void> {
  const input = process.stdin, output = process.stdout;
  if (!input.isTTY || !output.isTTY) throw new Error('Work-card pane requires an interactive terminal; open it through explicit /work-card open inside Herdr.');
  const path = env.WORK_CARDS_CARD;
  let workspace: { store: RecordStore; client: WorkspaceClient; view: WorkspaceCardView } | undefined;
  if (env.WORK_CARDS_AGENT !== undefined) {
    if (path !== undefined) throw new Error('Legacy card and workspace agent modes cannot be mixed.');
    const sessionId = text(env.WORK_CARDS_SESSION_ID, true);
    const store = new RecordStore(absolute(env.WORK_CARDS_STATE_DIR));
    if (absolute(env.WORK_CARDS_AGENT) !== store.agentPath(sessionId)) throw new Error('WORK_CARDS_AGENT must exactly match the session agent path under WORK_CARDS_STATE_DIR.');
    text(env.WORK_CARDS_OWNER_TERMINAL, true);
    if (!/^[A-Za-z0-9_-]{1,80}$/.test(env.WORK_CARDS_NONCE ?? '')) throw new Error('WORK_CARDS_NONCE must be a valid pair nonce.');
    workspace = { store, client: new WorkspaceClient({ env }), view: new WorkspaceCardView(store, sessionId) };
  } else {
    if (['WORK_CARDS_SESSION_ID', 'WORK_CARDS_OWNER_TERMINAL', 'WORK_CARDS_NONCE'].some(key => env[key] !== undefined)) throw new Error('Workspace launch identity requires WORK_CARDS_AGENT; modes cannot be mixed.');
    if (!path || !isAbsolute(path)) throw new Error('WORK_CARDS_CARD must identify an absolute YAML card on this machine.');
    if (env.WORK_CARDS_STATE_DIR && dirname(path) !== env.WORK_CARDS_STATE_DIR) throw new Error('Card/state directory mismatch; reopen explicitly.');
  }
  const config = await loadConfig(env.WORK_CARDS_CONFIG || configPath(env));
  const view = workspace?.view ?? new CardView(path!), client = new HerdrClient({ env }), parser = new KeyInput();
  let attachment: Attachment | undefined;
  let context: NativeContext | undefined;
  const controller = new AbortController();
  const verifyOwner = async (current: NativeContext, agent: Snapshot<AgentRecord>) => {
    const pair = await workspace!.client.verifyDisplay(workspace!.store, current, agent, config, controller.signal);
    controller.signal.throwIfAborted();
    if (pair.kind !== 'paired' || pair.agent.terminal_id !== env.WORK_CARDS_OWNER_TERMINAL
      || pair.card.terminal_id !== current.terminalId || pair.nonce !== env.WORK_CARDS_NONCE) {
      throw new Error('Launcher owner/card identity does not match a verified paired display.');
    }
  };
  let stopped = false, wake: (() => void) | undefined;
  let keys: string[] = [];
  let previous = '';
  const originalRaw = input.isRaw;
  const stop = () => { stopped = true; controller.abort(); wake?.(); };
  const data = (bytes: Buffer) => { keys.push(...parser.feed(bytes)); wake?.(); };
  const resize = () => wake?.();
  const signals: NodeJS.Signals[] = ['SIGTERM', 'SIGINT', 'SIGHUP'];
  try {
    input.setRawMode(true); input.resume();
    input.on('data', data); input.on('end', stop); output.on('resize', resize);
    for (const signal of signals) process.on(signal, stop);
    output.write('\x1b[?1049h\x1b[?25l\x1b[?7l\x1b[?2004h');
    while (!stopped) {
      const saved = await view.refresh();
      if (stopped) break;
      // Only this iteration's verified destination permits document effects.
      let readyContext: NativeContext | undefined;
      if (workspace) {
        try {
          context ??= await workspace.client.bootstrap(config);
          if (stopped) break;
          const live = await workspace.client.observe(context, config);
          if (stopped) break;
          context = live.context;
          if (saved && 'agent' in saved) {
            await verifyOwner(context, saved.agent);
            readyContext = context;
            await workspace.client.project(workspace.store, context, saved.agent, config, controller.signal);
            if (stopped) break;
            view.clearNotice('projection');
          }
        } catch (error) { view.setNotice('projection', `Herdr projection unavailable: ${(error as Error).message}`); }
      } else if (saved && !('agent' in saved)) {
        try {
          attachment ??= await client.paneAttachment(saved, config);
          await client.projectCard(saved, config, attachment);
          // Clear only a transient publishing notice, not document/read errors.
          view.clearNotice('projection');
        } catch (error) { view.setNotice('projection', `Herdr projection unavailable: ${(error as Error).message}`); }
      }
      if (stopped) break;
      const pending = keys; keys = [];
      for (const key of pending) {
        if (workspace && !readyContext && key !== 'q' && key !== '\x03') continue;
        const intent = await view.key(key);
        if (stopped) break;
        if (intent.kind === 'exit') { stop(); break; }
        if (intent.kind === 'open-document') {
          try {
            if ('identity' in intent && workspace && readyContext) {
              await verifyOwner(readyContext, intent.agent);
              await workspace.client.openDocument(workspace.store, readyContext, intent.workspace, intent.agent, intent.identity, config, controller.signal);
            } else if ('snapshot' in intent) await client.openDocument(intent.snapshot, intent.relativePath, config);
          } catch (error) { view.setNotice('document', `Cannot open ${'identity' in intent ? intent.identity.path : intent.relativePath}: ${(error as Error).message}`); }
          // Discard queued autorepeat after focus or partial failure; retry needs a fresh keypress.
          keys = [];
          break;
        }
      }
      const columns = output.columns || 80, rows = output.rows || 24;
      const rendered = view.render(columns, rows);
      const frame = `${columns}:${rows}:${rendered}`;
      if (frame !== previous) { output.write(`\x1b[2J\x1b[H${rendered}`); previous = frame; }
      if (stopped) break;
      if (!keys.length) await new Promise<void>(resolve => {
        const timer = setTimeout(done, config.pane_poll_interval_ms);
        function done() { clearTimeout(timer); wake = undefined; resolve(); }
        wake = done;
      });
    }
  } finally {
    controller.abort();
    input.off('data', data); input.off('end', stop); output.off('resize', resize);
    for (const signal of signals) process.off(signal, stop);
    input.setRawMode(originalRaw); input.pause();
    output.write('\x1b[?2004l\x1b[?7h\x1b[?25h\x1b[?1049l');
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  runPane().catch(error => { process.stderr.write(`${plain((error as Error).message)}\n`); process.exitCode = 1; });
}
