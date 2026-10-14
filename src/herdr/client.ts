import { execFile } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { lstat, open, realpath, unlink } from 'node:fs/promises';
import { basename, dirname, isAbsolute, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { isDeepStrictEqual } from 'node:util';
import { homedir } from 'node:os';
import { resolveDocument, type CardSnapshot } from '../card.js';
import { configPath, documentCommand, type WorkCardsConfig } from '../config.js';
import { classifyExplicitOwner, liveOwner, workspaceKeys, type Attachment, type Pane, type Workspace } from './ownership.js';
export type { Attachment } from './ownership.js';
type Run = (argv: string[], timeoutMs: number) => Promise<string>;
interface ClientOptions { env?: NodeJS.ProcessEnv; run?: Run; pluginRoot?: string }
const pluginPanes = [
  { id: 'card', title: 'Work card', placement: 'split', command: ['node', 'dist/herdr/pane.js'] },
  { id: 'document', title: 'Document', placement: 'tab', command: ['/bin/sh', 'bin/open-document.sh'] },
];

function object(value: unknown, label: string): Record<string, any> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error(`Unexpected Herdr ${label}`);
  return value as Record<string, any>;
}
function id(value: unknown): string {
  if (typeof value !== 'string' || !value || /[\p{Cc}\p{Cf}]/u.test(value)) throw new Error('Invalid Herdr identifier');
  return value;
}
function tokens(value: unknown): Record<string, string> {
  if (value === undefined) return {};
  const result = object(value, 'tokens');
  if (Object.values(result).some(v => typeof v !== 'string')) throw new Error('Invalid Herdr tokens');
  return result;
}
function parsePane(value: unknown): Pane {
  const p = object(value, 'pane');
  return { pane_id: id(p.pane_id), terminal_id: id(p.terminal_id), workspace_id: id(p.workspace_id), tab_id: id(p.tab_id), tokens: tokens(p.tokens) };
}
export async function cardIdentity(path: string): Promise<string> {
  if (!isAbsolute(path)) throw new Error('Work card path must be absolute');
  return createHash('sha256').update(await realpath(path)).digest('hex');
}

async function cleanupIdentity(path: string): Promise<string> {
  try { return await cardIdentity(path); }
  catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
  // Validated cards are regular files. A dangling final symlink cannot prove its previous target.
  try {
    if ((await lstat(path)).isSymbolicLink()) throw new Error('Cannot prove cleanup identity through a dangling card symlink');
  } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
  const canonical = join(await realpath(dirname(path)), basename(path));
  return createHash('sha256').update(canonical).digest('hex');
}

// All cooperating callers use the socket's directory, not their possibly different TMPDIRs.
export async function withWorkspaceLock<T>(socket: string, workspaceId: string, operation: () => Promise<T>): Promise<T> {
  const key = createHash('sha256').update(`${socket}\0${workspaceId}`).digest('hex');
  const path = join(dirname(socket), `.work-cards-${key}.lock`);
  let lock;
  try { lock = await open(path, 'wx', 0o600); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'EEXIST') {
      throw new Error(`Work-cards lock exists at ${path}. Inspect the owner/process before manually removing the lock; never steal it. Retry on the next poll or explicit open.`);
    }
    throw error;
  }
  try { await lock.writeFile(`pid=${process.pid}\n`); return await operation(); }
  finally { try { await lock.close(); } finally { await unlink(path); } }
}

export class HerdrClient {
  readonly env: NodeJS.ProcessEnv;
  private readonly run: Run;
  private readonly pluginRoot: string;
  constructor(options: ClientOptions = {}) {
    this.env = { ...(options.env ?? process.env) };
    this.pluginRoot = options.pluginRoot ?? fileURLToPath(new URL('../../', import.meta.url));
    this.run = options.run ?? ((argv, timeoutMs) => new Promise((resolve, reject) => {
      execFile(this.env.HERDR_BIN_PATH || 'herdr', argv, { env: this.env, encoding: 'utf8', timeout: timeoutMs, shell: false }, (error, stdout, stderr) => {
        if (error) reject(new Error(`Herdr ${argv.slice(0, 2).join(' ')} failed; a mutation may be partial, inspect before retrying: ${stderr || error.message}`));
        else resolve(stdout);
      });
    }));
  }
  private async result(argv: string[], config: WorkCardsConfig, expected: string): Promise<Record<string, any>> {
    let response;
    try { response = object(JSON.parse(await this.run(argv, config.herdr_timeout_ms)), 'response'); }
    catch (error) { throw new Error(`Invalid Herdr response: ${(error as Error).message}`, { cause: error }); }
    if (response.error) throw new Error(`Herdr error: ${JSON.stringify(response.error)}`);
    const result = object(response.result, 'result');
    if (result.type !== expected) throw new Error(`Unexpected Herdr result; expected ${expected}`);
    return result;
  }
  private async current(config: WorkCardsConfig): Promise<Pane> {
    if (this.env.HERDR_ENV !== '1' || !this.env.HERDR_PANE_ID || !this.env.HERDR_SOCKET_PATH) {
      throw new Error('Not in a Herdr-managed pane. Run explicit work-card open inside Herdr; GUI focus is never used.');
    }
    return parsePane((await this.result(['pane', 'current', '--current'], config, 'pane_current')).pane);
  }
  private async socket(config: WorkCardsConfig): Promise<string> {
    const response = object(JSON.parse(await this.run(['status', '--json'], config.herdr_timeout_ms)), 'status');
    const server = object(response.server, 'server');
    const version = typeof server.version === 'string' ? /^(\d+)\.(\d+)\.(\d+)(?:[-+].*)?$/.exec(server.version) : null;
    if (server.running !== true || server.compatible !== true || server.endpoint_compatible !== true || !version ||
        (Number(version[1]) === 0 && (Number(version[2]) < 9 || (Number(version[2]) === 9 && Number(version[3]) < 3)))) {
      throw new Error('Work cards require a running compatible Herdr server >=0.9.3; no automatic restart/update is attempted.');
    }
    const socket = id(server.socket);
    if (!isAbsolute(socket) || socket !== this.env.HERDR_SOCKET_PATH) throw new Error('Herdr endpoint mismatch; refusing focused/default session fallback');
    return socket;
  }
  private async workspace(workspaceId: string, config: WorkCardsConfig): Promise<Workspace> {
    const w = object((await this.result(['workspace', 'get', workspaceId], config, 'workspace_info')).workspace, 'workspace');
    if (w.workspace_id !== workspaceId) throw new Error('Herdr workspace identity mismatch');
    return { workspace_id: workspaceId, tokens: tokens(w.tokens) };
  }
  private async inventory(workspaceId: string, config: WorkCardsConfig): Promise<{ workspace: Workspace; panes: Pane[] }> {
    const workspace = await this.workspace(workspaceId, config);
    const list = (await this.result(['pane', 'list', '--workspace', workspaceId], config, 'pane_list')).panes;
    if (!Array.isArray(list)) throw new Error('Unexpected Herdr pane list');
    const panes = list.map(parsePane);
    if (panes.some(p => p.workspace_id !== workspaceId)) throw new Error('Herdr pane workspace mismatch');
    return { workspace, panes };
  }
  private async report(kind: 'pane' | 'workspace', target: string, values: Record<string, string | null>, config: WorkCardsConfig): Promise<void> {
    const argv = [kind, 'report-metadata', target, '--source', 'work-cards'];
    for (const [key, value] of Object.entries(values)) argv.push(value === null ? '--clear-token' : '--token', value === null ? key : `${key}=${value}`);
    await this.run(argv, config.herdr_timeout_ms);
  }
  private async ensurePlugin(config: WorkCardsConfig): Promise<void> {
    const root = await realpath(this.pluginRoot);
    const check = (value: unknown) => {
      const p = object(value, 'plugin registration');
      if (p.plugin_id !== 'work-cards' || p.enabled !== true || p.plugin_root !== root || p.manifest_path !== join(root, 'herdr-plugin.toml') ||
          p.source?.kind !== 'local' || p.version !== '0.1.0' || p.min_herdr_version !== '0.9.3' ||
          !isDeepStrictEqual(p.platforms, ['macos', 'linux']) || !isDeepStrictEqual(p.panes, pluginPanes)) {
        throw new Error(`Conflicting or disabled work-cards plugin registration; inspect it and explicitly relink with herdr plugin link ${join(root, 'herdr-plugin.toml')}. No registration was replaced; linked manifests may require explicit relink for new entrypoints.`);
      }
    };
    const plugins = (await this.result(['plugin', 'list', '--plugin', 'work-cards', '--json'], config, 'plugin_list')).plugins;
    if (!Array.isArray(plugins) || plugins.length > 1) throw new Error('Unexpected Herdr plugin registration list');
    if (plugins.length) check(plugins[0]);
    else check((await this.result(['plugin', 'link', join(root, 'herdr-plugin.toml')], config, 'plugin_linked')).plugin);
  }
  async attachCard(snapshot: CardSnapshot, config: WorkCardsConfig): Promise<Attachment> {
    const caller = await this.current(config), socket = await this.socket(config), identity = await cardIdentity(snapshot.path);
    return withWorkspaceLock(socket, caller.workspace_id, async () => {
      const current = await this.current(config);
      if (current.workspace_id !== caller.workspace_id || current.terminal_id !== caller.terminal_id || current.pane_id !== caller.pane_id) throw new Error('Caller moved while attaching; retry explicitly');
      const { workspace, panes } = await this.inventory(caller.workspace_id, config);
      const ownership = classifyExplicitOwner(socket, identity, workspace, panes, current);
      if (ownership.kind === 'live') return ownership.attachment;
      if (ownership.kind === 'recoverable-orphan') {
        await this.report('workspace', workspace.workspace_id, Object.fromEntries(workspaceKeys.map(key => [key, null])), config);
      }
      await this.ensurePlugin(config);
      const nonce = randomUUID(), root = await realpath(this.pluginRoot);
      const opened = await this.result(['plugin', 'pane', 'open', '--plugin', 'work-cards', '--entrypoint', 'card', '--placement', 'split',
        '--target-pane', caller.pane_id, '--direction', 'down', '--cwd', root, '--no-focus',
        '--env', `WORK_CARDS_CARD=${await realpath(snapshot.path)}`, '--env', `WORK_CARDS_CONFIG=${configPath(this.env)}`,
        '--env', `WORK_CARDS_STATE_DIR=${dirname(await realpath(snapshot.path))}`, '--env', `WORK_CARDS_NONCE=${nonce}`,
        '--env', `EDITOR=${this.env.EDITOR ?? ''}`, ...(this.env.PATH === undefined ? [] : ['--env', `PATH=${this.env.PATH}`])], config, 'plugin_pane_opened');
      const info = object(opened.plugin_pane, 'plugin pane'), p = parsePane(info.pane);
      if (info.plugin_id !== 'work-cards' || info.entrypoint !== 'card' || p.workspace_id !== caller.workspace_id || p.tab_id !== caller.tab_id || p.pane_id === caller.pane_id) {
        throw new Error('Herdr opened unexpected plugin pane; opening may be partial, inspect before retrying');
      }
      const values = { wc_card: identity, wc_nonce: nonce, wc_pane: p.pane_id, wc_terminal: p.terminal_id };
      await this.report('pane', p.pane_id, { ...values, wc_role: 'card' }, config);
      await this.report('workspace', caller.workspace_id, values, config);
      const renamed = object((await this.result(['workspace', 'rename', caller.workspace_id, snapshot.card.title], config, 'workspace_info')).workspace, 'renamed workspace');
      if (renamed.workspace_id !== caller.workspace_id) throw new Error('Herdr renamed workspace identity mismatch');
      // Plugin splits start balanced. Resize affects only the caller split, with no focus request.
      if (config.pane_height_fraction !== 0.5) {
        try {
          const resize = object((await this.result(['pane', 'resize', '--pane', caller.pane_id, '--direction', config.pane_height_fraction < 0.5 ? 'down' : 'up',
            '--amount', String(Math.abs(0.5 - config.pane_height_fraction))], config, 'pane_resize')).resize, 'resize');
          if (resize.pane_id !== caller.pane_id) throw new Error('Resize identity mismatch');
        } catch { /* Best-effort geometry must not relaunch a successfully owned pane. */ }
      }
      return { socket, workspaceId: caller.workspace_id, paneId: p.pane_id, terminalId: p.terminal_id, cardIdentity: identity, nonce };
    });
  }
  async openDocument(snapshot: CardSnapshot, relativePath: string, config: WorkCardsConfig): Promise<void> {
    if (!snapshot.card.documents.some(document => document.path === relativePath)) throw new Error('Document is not listed on this card');
    const command = documentCommand(config, this.env);
    const document = await resolveDocument(snapshot.card, relativePath);
    const cwd = await realpath(snapshot.card.cwd.startsWith('~/') ? join(homedir(), snapshot.card.cwd.slice(2)) : snapshot.card.cwd);
    const caller = await this.current(config), socket = await this.socket(config), identity = await cardIdentity(snapshot.path);
    await withWorkspaceLock(socket, caller.workspace_id, async () => {
      const current = await this.current(config);
      if (current.pane_id !== caller.pane_id || current.terminal_id !== caller.terminal_id ||
          current.workspace_id !== caller.workspace_id || current.tab_id !== caller.tab_id || current.pane_id !== this.env.HERDR_PANE_ID) {
        throw new Error('Work-card caller moved; nothing opened, retry explicitly');
      }
      const { workspace, panes } = await this.inventory(caller.workspace_id, config);
      const ownership = classifyExplicitOwner(socket, identity, workspace, panes, current);
      if (ownership.kind !== 'live' || ownership.attachment.paneId !== current.pane_id ||
          ownership.attachment.terminalId !== current.terminal_id || ownership.attachment.nonce !== this.env.WORK_CARDS_NONCE) {
        throw new Error('Work-card ownership conflict: only the verified live card pane may open documents');
      }
      const documentKey = createHash('sha256').update(JSON.stringify([cwd, document])).digest('hex');
      const matching = (inventory: Pane[]): Pane | undefined => {
        const records = inventory.filter(p => p.tokens.wcd_key === documentKey);
        if (!records.length) return undefined;
        const p = records[0]!;
        if (records.length !== 1 || p.tokens.wcd_pane !== p.pane_id || p.tokens.wcd_terminal !== p.terminal_id || p.tokens.wcd_tab !== p.tab_id) {
          throw new Error('Conflicting or incomplete document tab record; inspect the workspace before retrying. Nothing launched or focused.');
        }
        return p;
      };
      const focus = async (tab: string): Promise<void> => {
        // Focus is documented; do not invent a result type for its response.
        const focused = object(JSON.parse(await this.run(['tab', 'focus', tab], config.herdr_timeout_ms)), 'focus response');
        if (focused.error) throw new Error(`Herdr focus error: ${JSON.stringify(focused.error)}`);
      };
      const existing = matching(panes);
      if (existing) {
        try { await focus(existing.tab_id); }
        catch (error) { throw new Error(`Document tab focus may be partial (pane ${existing.pane_id}, terminal ${existing.terminal_id}, tab ${existing.tab_id}); inspect before retrying: ${(error as Error).message}`, { cause: error }); }
        return;
      }
      await this.ensurePlugin(config);
      let tabId: string | undefined, paneId: string | undefined, terminalId: string | undefined;
      try {
        const opened = await this.result(['plugin', 'pane', 'open', '--plugin', 'work-cards', '--entrypoint', 'document',
          '--placement', 'tab', '--workspace', caller.workspace_id, '--cwd', await realpath(this.pluginRoot), '--no-focus',
          '--env', `WORK_CARDS_DOCUMENT_COMMAND=${command}`, '--env', `WORK_CARDS_DOCUMENT_CWD=${cwd}`,
          '--env', `WORK_CARDS_DOCUMENT_PATH=${document}`, ...(this.env.PATH === undefined ? [] : ['--env', `PATH=${this.env.PATH}`])], config, 'plugin_pane_opened');
        const info = object(opened.plugin_pane, 'plugin pane');
        if (typeof info.pane?.tab_id === 'string') tabId = id(info.pane.tab_id);
        const p = parsePane(info.pane);
        paneId = p.pane_id; terminalId = p.terminal_id;
        if (info.plugin_id !== 'work-cards' || info.entrypoint !== 'document' || p.workspace_id !== caller.workspace_id ||
            panes.some(existing => existing.pane_id === p.pane_id || existing.terminal_id === p.terminal_id || existing.tab_id === p.tab_id)) {
          throw new Error('Herdr returned an unexpected or reused document pane/tab/terminal');
        }
        await this.report('pane', p.pane_id, { wcd_key: documentKey, wcd_pane: p.pane_id, wcd_terminal: p.terminal_id, wcd_tab: p.tab_id }, config);
        const stamped = matching((await this.inventory(caller.workspace_id, config)).panes);
        if (!stamped || stamped.pane_id !== p.pane_id || stamped.terminal_id !== p.terminal_id || stamped.tab_id !== p.tab_id) {
          throw new Error('Document tab metadata did not round-trip to the returned live tuple');
        }
        const label = basename(relativePath);
        const tab = object((await this.result(['tab', 'rename', p.tab_id, label], config, 'tab_info')).tab, 'tab');
        if (tab.tab_id !== p.tab_id || tab.workspace_id !== caller.workspace_id || tab.label !== label) throw new Error('Herdr renamed tab identity/label mismatch');
        await focus(p.tab_id);
      } catch (error) {
        throw new Error(`Document tab opening may be partial${tabId ? ` (returned pane ${paneId ?? 'unknown'}, terminal ${terminalId ?? 'unknown'}, tab ${tabId})` : ''}. Inspect the workspace/tab before retrying; no tab was closed: ${(error as Error).message}`, { cause: error });
      }
    });
  }
  async projectCard(snapshot: CardSnapshot, config: WorkCardsConfig, expected: Attachment): Promise<void> {
    const identity = await cardIdentity(snapshot.path);
    await withWorkspaceLock(expected.socket, expected.workspaceId, async () => {
      const current = await this.current(config);
      if (current.pane_id !== expected.paneId || current.terminal_id !== expected.terminalId || current.workspace_id !== expected.workspaceId ||
          this.env.WORK_CARDS_NONCE !== expected.nonce || this.env.HERDR_SOCKET_PATH !== expected.socket) {
        throw new Error('Work-card ownership conflict: only the verified card pane may publish workspace summaries');
      }
      // Attach/detach scan all panes under this same lock. Polling needs only the live owner and workspace markers.
      const workspace = await this.workspace(expected.workspaceId, config);
      const live = liveOwner(expected.socket, identity, workspace, [current]);
      if (!live || !isDeepStrictEqual(live, expected)) throw new Error('Work-card ownership conflict: publisher is no longer the live owner');
      const summary = Array.from(`${snapshot.card.status} · ${snapshot.card.state}`.trim()).slice(0, 80).join('');
      if (workspace.tokens.wc_status === snapshot.card.status && workspace.tokens.wc_summary === summary) return;
      await this.report('workspace', expected.workspaceId, { wc_status: snapshot.card.status, wc_summary: summary }, config);
    });
  }
  async paneAttachment(snapshot: CardSnapshot, config: WorkCardsConfig): Promise<Attachment> {
    const p = await this.current(config), socket = await this.socket(config);
    return { socket, workspaceId: p.workspace_id, paneId: p.pane_id, terminalId: p.terminal_id,
      cardIdentity: await cardIdentity(snapshot.path), nonce: id(this.env.WORK_CARDS_NONCE) };
  }
  async detachCard(snapshot: Pick<CardSnapshot, 'path'>, config: WorkCardsConfig): Promise<void> {
    const caller = await this.current(config), socket = await this.socket(config), identity = await cleanupIdentity(snapshot.path);
    await withWorkspaceLock(socket, caller.workspace_id, async () => {
      const current = await this.current(config);
      if (current.workspace_id !== caller.workspace_id || current.terminal_id !== caller.terminal_id) throw new Error('Caller moved while detaching; retry explicitly');
      const { workspace, panes } = await this.inventory(caller.workspace_id, config);
      const ownership = classifyExplicitOwner(socket, identity, workspace, panes, current);
      if (ownership.kind === 'empty') return;
      if (ownership.kind === 'recoverable-orphan') {
        await this.report('workspace', workspace.workspace_id, Object.fromEntries(workspaceKeys.map(key => [key, null])), config);
        return;
      }
      const owned = ownership.attachment;
      const closed = await this.result(['plugin', 'pane', 'close', owned.paneId], config, 'plugin_pane_closed');
      if (closed.pane_id !== owned.paneId) throw new Error('Herdr closed pane identity mismatch');
      await this.report('workspace', caller.workspace_id, Object.fromEntries(workspaceKeys.map(key => [key, null])), config);
    });
  }
}

export function attachCard(snapshot: CardSnapshot, config: WorkCardsConfig): Promise<Attachment> {
  return new HerdrClient().attachCard(snapshot, config);
}
export function detachCard(snapshot: Pick<CardSnapshot, 'path'>, config: WorkCardsConfig): Promise<void> {
  return new HerdrClient().detachCard(snapshot, config);
}
