import { readFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { join } from 'node:path';
import type { AssistantMessage, Context, ModelThinkingLevel, ModelsSimpleStreamOptions } from '@earendil-works/pi-ai';
import type { WorkCardsConfig } from '../config.js';

export type SummaryRequestOptions = Pick<WorkCardsConfig, 'model' | 'thinking_level' | 'max_output_tokens' | 'request_timeout_ms'>;
export interface SummaryRegistry<TModel> {
  find(provider: string, modelId: string): TModel | undefined;
  streamSimple(model: TModel, context: Context, options?: ModelsSimpleStreamOptions): {
    result(): Promise<Pick<AssistantMessage, 'stopReason' | 'content' | 'errorMessage'>>;
  };
}

function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

async function readHelperProfiles(): Promise<unknown> {
  const path = join(process.env.PI_CODING_AGENT_DIR || join(process.env.HOME || homedir(), '.pi', 'agent'), 'helper-models.json');
  try {
    return JSON.parse(await readFile(path, 'utf8'));
  } catch (error) {
    throw new Error(`Cannot read helper-models.json at ${path}: ${(error as Error).message}. Configure the helper alias and reload Pi.`, { cause: error });
  }
}

function helperThinkingLevel(profiles: unknown, tier: string): ModelThinkingLevel {
  const profile = object(profiles) ? profiles[tier] : undefined;
  const level = object(profile) ? profile.thinkingLevel : undefined;
  if (typeof level !== 'string' || !['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'].includes(level)) {
    throw new Error(`helper-models.json: helpers/${tier}.thinkingLevel must be off, minimal, low, medium, high, xhigh, or max.`);
  }
  return level as ModelThinkingLevel;
}

/** Stop waiting even if a provider ignores its transport cancellation signal. */
function awaitWithAbort<T>(promise: Promise<T>, signal: AbortSignal): Promise<T> {
  return new Promise((resolve, reject) => {
    const onAbort = () => {
      signal.removeEventListener('abort', onAbort);
      reject(signal.reason);
    };
    signal.addEventListener('abort', onAbort, { once: true });
    // Both handlers remain attached after abort, consuming any late rejection.
    promise.then((value) => {
      signal.removeEventListener('abort', onAbort);
      if (signal.aborted) reject(signal.reason);
      else resolve(value);
    }, (error) => {
      signal.removeEventListener('abort', onAbort);
      reject(signal.aborted ? signal.reason : error);
    });
    if (signal.aborted) onAbort();
  });
}

/** Direct registry calls stay outside the agent loop and offer no executable tools. */
export async function requestHelperDecision<T, TModel extends { api?: string }>(
  registry: SummaryRegistry<TModel>,
  context: Context,
  options: SummaryRequestOptions,
  parseDecision: (text: string) => T,
  signal?: AbortSignal,
  loadHelperProfiles: () => Promise<unknown> = readHelperProfiles,
  onRequest?: () => (() => void),
): Promise<T> {
  const requestSignal = AbortSignal.any([
    ...(signal ? [signal] : []), AbortSignal.timeout(options.request_timeout_ms),
  ]);
  requestSignal.throwIfAborted();
  const slash = options.model.indexOf('/');
  if (slash < 1 || slash === options.model.length - 1) throw new Error('Summary model must be provider/model.');
  const provider = options.model.slice(0, slash);
  const modelId = options.model.slice(slash + 1);
  const model = registry.find(provider, modelId);
  if (!model) throw new Error(`Summary model ${options.model} is not registered. Configure that model and reload Pi.`);
  let level: ModelThinkingLevel = options.thinking_level ?? 'off';
  if (provider === 'helpers') {
    if (model.api !== 'pi-virtual') throw new Error(`Summary alias ${options.model} must be a registered helpers virtual model. Load helper-models and reload Pi.`);
    if (options.thinking_level == null) level = helperThinkingLevel(await loadHelperProfiles(), modelId);
  }
  requestSignal.throwIfAborted();
  // Virtual routing needs literal off; physical adapters disable thinking by omission.
  const reasoning = level === 'off' && model.api !== 'pi-virtual' ? undefined : level;
  const requestOptions = {
    maxTokens: options.max_output_tokens, toolChoice: 'none', reasoning, signal: requestSignal,
  } as ModelsSimpleStreamOptions;
  const stopActivity = onRequest?.();
  try {
    const result = await awaitWithAbort(
      registry.streamSimple(model, { ...context, tools: [] }, requestOptions).result(), requestSignal,
    );
    requestSignal.throwIfAborted();
    if (result.stopReason === 'error') {
      const diagnostic = (result.errorMessage ?? '')
        .replace(/\u001b\][^\u0007\u001b]*(?:\u0007|\u001b\\)/g, '')
        .replace(/\u001b\[[0-?]*[ -/]*[@-~]/g, '')
        .replace(/[\u0000-\u001f\u007f-\u009f\u2028-\u202e\u2066-\u2069]/g, ' ')
        .replace(/\bBearer\s+[A-Za-z0-9._~+/-]+=*/gi, 'Bearer [redacted]')
        .replace(/\s+/g, ' ').trim().slice(0, 500);
      throw new Error(`Summary model did not return a complete text decision (error). Model ${options.model}: ${diagnostic || 'No provider diagnostic was returned. Check the provider configuration and availability, then try again.'}`);
    }
    if (result.stopReason !== 'stop' || result.content.some((block) => block.type === 'toolCall')) {
      throw new Error(`Summary model did not return a complete text decision (${result.stopReason}).`);
    }
    return parseDecision(result.content.filter((block) => block.type === 'text').map((block) => block.text).join(''));
  } finally { stopActivity?.(); }
}

export type HelperRequestTail = [signal?: AbortSignal, loadHelperProfiles?: () => Promise<unknown>, onRequest?: () => (() => void)];

