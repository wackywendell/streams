import { pathToFileURL } from 'node:url';
import { configPath, loadConfig } from '../config.js';
import { KeyInput, plain } from './view.js';
import { STREAMS_READ_ENV_KEYS, createStreamsContextReader, StreamsCardSource, type StreamsCardScope, type StreamsNativeOwner } from './streams-source.js';
import { StreamsCardView } from './streams-view.js';
import { StreamsWorkspaceClient, type StreamsNativeContext } from './streams-workspace-client.js';

export function streamsPaneSource(env: NodeJS.ProcessEnv): StreamsCardSource {
  let scope: StreamsCardScope, owner: StreamsNativeOwner;
  try { scope = JSON.parse(env.STREAMS_CARDS_SCOPE ?? ''); owner = JSON.parse(env.STREAMS_CARDS_OWNER ?? ''); }
  catch { throw new Error('Streams pane requires exact launcher scope and owner JSON'); }
  if (!scope || !['catalogSource', 'hostId', 'streamId', 'sessionId', 'nativeSessionId'].every(k => typeof scope[k as keyof StreamsCardScope] === 'string' && scope[k as keyof StreamsCardScope].length > 0)
    || !owner || owner.frontend !== 'herdr' || ![owner.pane_id, owner.tab_id, owner.workspace_id].every(v => typeof v === 'string' && v.length > 0)
    || !/^[A-Za-z0-9_-]+$/.test(env.STREAMS_CARDS_NONCE ?? '') || !env.STREAMS_CARDS_OWNER_TERMINAL || !env.STREAMS_CARDS_STATE_DIR) throw new Error('Invalid Streams launcher identity, nonce or native state directory');
  let captured: unknown;
  try { captured = JSON.parse(env.STREAMS_CARDS_READ_ENV ?? ''); }
  catch { throw new Error('Streams pane requires launcher read environment JSON'); }
  if (!captured || typeof captured !== 'object' || Array.isArray(captured)
    || Object.entries(captured).some(([k, v]) => !STREAMS_READ_ENV_KEYS.some(key => key === k) || typeof v !== 'string' || v.includes('\0'))) throw new Error('Invalid Streams launcher read environment');
  const readerEnv = { ...env };
  for (const key of STREAMS_READ_ENV_KEYS) delete readerEnv[key];
  Object.assign(readerEnv, captured);
  return new StreamsCardSource({ nativeSessionId: scope.nativeSessionId, expectedScope: scope,
    readContext: createStreamsContextReader({ env: readerEnv, owner }) });
}

/** Imported terminal loop: viewport/Unicode/keyboard behavior remains in the shared view. */
export async function runStreamsPane(env: NodeJS.ProcessEnv = process.env): Promise<void> {
  const input = process.stdin, output = process.stdout;
  if (!input.isTTY || !output.isTTY) throw new Error('Streams card requires an interactive Herdr terminal');
  const source = streamsPaneSource(env), client = new StreamsWorkspaceClient({ env, stateDirectory: env.STREAMS_CARDS_STATE_DIR });
  const controller = new AbortController();
  const view = new StreamsCardView({ read: () => source.read(controller.signal) }), parser = new KeyInput();
  const config = await loadConfig(env.STREAMS_CARDS_CONFIG || configPath(env));
  let context: StreamsNativeContext | undefined, stopped = false, closeRequested = false, selfClosing = false, wake: (() => void) | undefined;
  let keys: string[] = [], previous = '', repaintQueued = false;
  const render = () => {
    repaintQueued = false;
    if (stopped) return;
    const columns = output.columns || 80, rows = output.rows || 24, rendered = view.render(columns, rows);
    const frame = `${columns}:${rows}:${rendered}`;
    if (frame !== previous) { output.write(`\x1b[2J\x1b[H${rendered}`); previous = frame; }
  };
  const originalRaw = input.isRaw;
  const stop = () => { if (selfClosing) return; stopped = true; controller.abort(); wake?.(); };
  const data = (bytes: Buffer) => { keys.push(...parser.feed(bytes)); wake?.(); };
  // Reflow cached content without entering the observation/native-effect lane.
  const resize = () => {
    if (stopped || repaintQueued) return;
    repaintQueued = true;
    queueMicrotask(() => { if (repaintQueued) render(); });
  };
  const signals: NodeJS.Signals[] = ['SIGTERM', 'SIGINT', 'SIGHUP'];
  try {
    input.setRawMode(true); input.resume();
    input.on('data', data); input.on('end', stop); output.on('resize', resize);
    for (const signal of signals) process.on(signal, stop);
    output.write('\x1b[?1049h\x1b[?25l\x1b[?7l\x1b[?2004h');
    while (!stopped) {
      const saved = await view.refresh(); if (stopped) break;
      let ready = false;
      try {
        context ??= await client.bootstrap(config, controller.signal);
        if (saved) {
          const pair = await client.verifyDisplay(source, context, config, controller.signal);
          if (pair.agent.terminal_id !== env.STREAMS_CARDS_OWNER_TERMINAL || pair.card.terminal_id !== context.terminalId || pair.nonce !== env.STREAMS_CARDS_NONCE) throw new Error('Launcher owner/card/nonce mismatch');
          ready = true;
          if (pair.card.tokens.strm_caption_manual !== '1'
            && (pair.card.label !== saved.stream.title || pair.card.tokens.strm_caption !== saved.stream.title)) {
            await client.projectCaption(source, context, config, controller.signal);
          }
          view.clearNotice('projection');
        }
      } catch (error) { view.setNotice('projection', `Herdr projection unavailable: ${(error as Error).message}`); }
      if (stopped) break;
      const pending = keys; keys = [];
      for (const key of pending) {
        if (!ready && key !== 'q' && key !== '\x03') continue;
        const intent = await view.key(key); if (stopped) break;
        if (intent.kind === 'exit') {
          closeRequested = ready;
          if (!ready) process.stderr.write('Streams native card retained: launch ownership is unverified; inspect the pane before closing it manually.\n');
          stop(); break;
        }
        if (intent.kind === 'open-document') {
          try { if (context) await client.openDocument(source, context, intent.snapshot, intent.target, config, controller.signal); }
          catch (error) { view.setNotice('document', `Cannot open ${intent.target.path}: ${(error as Error).message}`); }
          keys = []; break;
        }
      }
      render();
      if (stopped) break;
      if (!keys.length) await new Promise<void>(resolve => {
        const timer = setTimeout(done, config.pane_poll_interval_ms);
        function done() { clearTimeout(timer); wake = undefined; resolve(); }
        wake = done;
      });
    }
  } finally {
    stopped = true;
    controller.abort();
    input.off('data', data); input.off('end', stop); output.off('resize', resize);
    try {
      input.setRawMode(originalRaw); input.pause();
      output.write('\x1b[?2004l\x1b[?7h\x1b[?25h\x1b[?1049l');
      // Herdr can send HUP before acknowledging self-close. Keep handlers installed
      // until the close's workspace lock has settled and unlinked its lockfile.
      if (closeRequested && context) {
        selfClosing = true;
        try { await client.closeDisplay(source, context, config); }
        catch (error) { process.stderr.write(`Streams native card retained: ${plain((error as Error).message)}; inspect ownership before closing manually.\n`); }
      }
    } finally { for (const signal of signals) process.off(signal, stop); }
  }
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  runStreamsPane().catch(error => { process.stderr.write(`${plain((error as Error).message)}\n`); process.exitCode = 1; });
}
