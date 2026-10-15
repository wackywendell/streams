import { pathToFileURL } from 'node:url';
import { configPath, loadConfig } from '../config.js';
import { KeyInput, plain } from './view.js';
import { absolute, RecordStore, text, type AgentRecord, type Snapshot } from '../records.js';
import { WorkspaceClient, type NativeContext } from './workspace-client.js';
import { WorkspaceCardView } from './workspace-view.js';

export async function runPane(env: NodeJS.ProcessEnv = process.env): Promise<void> {
  const input = process.stdin, output = process.stdout;
  if (!input.isTTY || !output.isTTY) throw new Error('Work-card pane requires an interactive terminal; open it through explicit /work-card open inside Herdr.');
  if (env.WORK_CARDS_CARD !== undefined) throw new Error('WORK_CARDS_CARD is unsupported; reopen with a workspace agent identity.');
  const sessionId = text(env.WORK_CARDS_SESSION_ID, true);
  const store = new RecordStore(absolute(env.WORK_CARDS_STATE_DIR));
  if (absolute(env.WORK_CARDS_AGENT) !== store.agentPath(sessionId)) throw new Error('WORK_CARDS_AGENT must exactly match the session agent path under WORK_CARDS_STATE_DIR.');
  text(env.WORK_CARDS_OWNER_TERMINAL, true);
  if (!/^[A-Za-z0-9_-]{1,80}$/.test(env.WORK_CARDS_NONCE ?? '')) throw new Error('WORK_CARDS_NONCE must be a valid pair nonce.');
  const client = new WorkspaceClient({ env }), view = new WorkspaceCardView(store, sessionId), parser = new KeyInput();
  const config = await loadConfig(env.WORK_CARDS_CONFIG || configPath(env));
  let context: NativeContext | undefined;
  const controller = new AbortController();
  const verifyOwner = async (current: NativeContext, agent: Snapshot<AgentRecord>) => {
    const pair = await client.verifyDisplay(store, current, agent, config, controller.signal);
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
      try {
        context ??= await client.bootstrap(config);
        if (stopped) break;
        const live = await client.observe(context, config);
        if (stopped) break;
        context = live.context;
        if (saved) {
          await verifyOwner(context, saved.agent);
          readyContext = context;
          await client.projectCaption(store, context, saved.agent, config, controller.signal);
          if (stopped) break;
          view.clearNotice('projection');
        }
      } catch (error) { view.setNotice('projection', `Herdr projection unavailable: ${(error as Error).message}`); }
      if (stopped) break;
      const pending = keys; keys = [];
      for (const key of pending) {
        if (!readyContext && key !== 'q' && key !== '\x03') continue;
        const intent = await view.key(key);
        if (stopped) break;
        if (intent.kind === 'exit') { stop(); break; }
        if (intent.kind === 'open-document') {
          try {
            if (readyContext) {
              await verifyOwner(readyContext, intent.agent);
              await client.openDocument(store, readyContext, intent.workspace, intent.agent, intent.identity, config, controller.signal);
            }
          } catch (error) { view.setNotice('document', `Cannot open ${intent.identity.path}: ${(error as Error).message}`); }
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
