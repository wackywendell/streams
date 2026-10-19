import { runProcess } from './subprocess.js';
import { realpath } from 'node:fs/promises';
import { isAbsolute, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { isDeepStrictEqual } from 'node:util';
import type { WorkCardsConfig } from '../config.js';
import type { Pane } from './workspace-ownership.js';
export type Run = (argv: string[], timeoutMs: number, signal?: AbortSignal) => Promise<string>;
export interface ClientOptions { env?: NodeJS.ProcessEnv; run?: Run; pluginRoot?: string }
const pluginPanes = [
  { id: 'card', title: 'Work card', placement: 'split', command: ['node', 'dist/herdr/pane.js'] },
  { id: 'document', title: 'Document', placement: 'tab', command: ['/bin/sh', 'bin/open-document.sh'] },
];

export function object(value: unknown, label: string): Record<string, any> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error(`Unexpected Herdr ${label}`);
  return value as Record<string, any>;
}
export function id(value: unknown): string {
  if (typeof value !== 'string' || !value || /[\p{Cc}\p{Cf}]/u.test(value)) throw new Error('Invalid Herdr identifier');
  return value;
}
export function tokens(value: unknown): Record<string, string> {
  if (value === undefined) return {};
  const result = object(value, 'tokens');
  if (Object.values(result).some(v => typeof v !== 'string')) throw new Error('Invalid Herdr tokens');
  return result;
}
export function parsePane(value: unknown): Pane {
  const p = object(value, 'pane');
  return { pane_id: id(p.pane_id), terminal_id: id(p.terminal_id), workspace_id: id(p.workspace_id), tab_id: id(p.tab_id), tokens: tokens(p.tokens) };
}
export class HerdrTransport {
  readonly env: NodeJS.ProcessEnv;
  protected readonly run: Run;
  protected readonly pluginRoot: string;
  constructor(options: ClientOptions = {}) {
    this.env = { ...(options.env ?? process.env) };
    this.pluginRoot = options.pluginRoot ?? fileURLToPath(new URL('../../', import.meta.url));
    this.run = options.run ?? ((argv, timeoutMs, signal) => runProcess(this.env.HERDR_BIN_PATH || 'herdr', argv, { env: this.env, timeoutMs, signal }));
  }
  protected async result(argv: string[], config: WorkCardsConfig, expected: string, signal?: AbortSignal): Promise<Record<string, any>> {
    let response;
    try { response = object(JSON.parse(await this.run(argv, config.herdr_timeout_ms, signal)), 'response'); }
    catch (error) { throw new Error(`Invalid Herdr response: ${(error as Error).message}`, { cause: error }); }
    if (response.error) throw new Error(`Herdr error: ${JSON.stringify(response.error)}`);
    const result = object(response.result, 'result');
    if (result.type !== expected) throw new Error(`Unexpected Herdr result; expected ${expected}`);
    return result;
  }
  protected async current(config: WorkCardsConfig, signal?: AbortSignal): Promise<Pane> {
    if (this.env.HERDR_ENV !== '1' || !this.env.HERDR_PANE_ID || !this.env.HERDR_SOCKET_PATH) {
      throw new Error('Not in a Herdr-managed pane. Run explicit work-card open inside Herdr; GUI focus is never used.');
    }
    return parsePane((await this.result(['pane', 'current', '--current'], config, 'pane_current', signal)).pane);
  }
  protected async socket(config: WorkCardsConfig, signal?: AbortSignal): Promise<string> {
    const response = object(JSON.parse(await this.run(['status', '--json'], config.herdr_timeout_ms, signal)), 'status');
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
  protected async report(kind: 'pane' | 'workspace', target: string, values: Record<string, string | null>, config: WorkCardsConfig): Promise<void> {
    const argv = [kind, 'report-metadata', target, '--source', 'work-cards'];
    for (const [key, value] of Object.entries(values)) argv.push(value === null ? '--clear-token' : '--token', value === null ? key : `${key}=${value}`);
    await this.run(argv, config.herdr_timeout_ms);
  }
  protected async ensurePlugin(config: WorkCardsConfig, beforeLink?: () => Promise<unknown>): Promise<void> {
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
    else {
      await beforeLink?.();
      check((await this.result(['plugin', 'link', join(root, 'herdr-plugin.toml')], config, 'plugin_linked')).plugin);
    }
  }
}
