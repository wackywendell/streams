import { dirname, isAbsolute } from 'node:path';
import { pathToFileURL } from 'node:url';
import { configPath, loadConfig } from '../config.js';
import { HerdrClient, type Attachment } from './client.js';
import { CardView, KeyInput, plain } from './view.js';

export async function runPane(env: NodeJS.ProcessEnv = process.env): Promise<void> {
  const input = process.stdin, output = process.stdout;
  if (!input.isTTY || !output.isTTY) throw new Error('Work-card pane requires an interactive terminal; open it through explicit /work-card open inside Herdr.');
  const path = env.WORK_CARDS_CARD;
  if (!path || !isAbsolute(path)) throw new Error('WORK_CARDS_CARD must identify an absolute YAML card on this machine.');
  if (env.WORK_CARDS_STATE_DIR && dirname(path) !== env.WORK_CARDS_STATE_DIR) throw new Error('Card/state directory mismatch; reopen explicitly.');
  const config = await loadConfig(env.WORK_CARDS_CONFIG || configPath(env));
  const view = new CardView(path, config), client = new HerdrClient({ env }), parser = new KeyInput();
  let attachment: Attachment | undefined;
  let stopped = false, wake: (() => void) | undefined;
  let keys: string[] = [];
  let previous = '';
  const originalRaw = input.isRaw;
  const stop = () => { stopped = true; wake?.(); };
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
      if (saved) {
        try {
          attachment ??= await client.paneAttachment(saved, config);
          await client.projectCard(saved, config, attachment);
          // Clear only a transient publishing notice, not document/read errors.
          view.clearNotice('projection');
        } catch (error) { view.setNotice('projection', `Herdr projection unavailable: ${(error as Error).message}`); }
      }
      const pending = keys; keys = [];
      for (const key of pending) { if (await view.key(key)) { stopped = true; break; } }
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
    input.off('data', data); input.off('end', stop); output.off('resize', resize);
    for (const signal of signals) process.off(signal, stop);
    input.setRawMode(originalRaw); input.pause();
    output.write('\x1b[?2004l\x1b[?7h\x1b[?25h\x1b[?1049l');
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  runPane().catch(error => { process.stderr.write(`${plain((error as Error).message)}\n`); process.exitCode = 1; });
}
