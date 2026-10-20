import { runProcess } from './subprocess.js';
import { isAbsolute } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { WorkCardsConfig } from '../config.js';
import { nativeId as id, nativeObject as object, nativePane as parsePane, nativeTokens as tokens, type Pane } from './workspace-ownership.js';
export type Run = (argv: string[], timeoutMs: number, signal?: AbortSignal) => Promise<string>;
export interface ClientOptions { env?: NodeJS.ProcessEnv; run?: Run; pluginRoot?: string }
export { id, object, parsePane, tokens };

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
    const p = parsePane((await this.result(['pane', 'current', '--current'], config, 'pane_current', signal)).pane);
    delete p.label;
    return p;
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
}
