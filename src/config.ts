import { readFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { join } from 'node:path';

export type { CardStatus, CardDocument, CardLink, Card, CardPatch, CardSnapshot } from './card.js';

export interface WorkCardsConfig {
  model: string;
  thinking_level: null | 'off' | 'minimal' | 'low' | 'medium' | 'high' | 'xhigh' | 'max';
  check_every_n_responses: number;
  check_every_n_cycles: number;
  max_context_chars: number;
  max_document_bytes: number;
  document_command: string | null;
  max_output_tokens: number;
  request_timeout_ms: number;
  pane_poll_interval_ms: number;
  herdr_timeout_ms: number;
  pane_height_fraction: number;
  update_guidance: string;
}

export const DEFAULT_CONFIG: Readonly<WorkCardsConfig> = Object.freeze({
  model: 'helpers/simple',
  thinking_level: null,
  check_every_n_responses: 1,
  check_every_n_cycles: 10,
  max_context_chars: 24000,
  max_document_bytes: 262144,
  document_command: null,
  max_output_tokens: 2048,
  request_timeout_ms: 30000,
  pane_poll_interval_ms: 1000,
  herdr_timeout_ms: 3000,
  pane_height_fraction: 0.2,
  update_guidance: 'Update only for significant, evidenced changes. Preserve stable facts. Do not infer progress, invent links or documents, or rewrite wording without a substantive change. Return no_change when nothing important changed.',
});

function validateDocumentCommand(value: unknown, label: string): asserts value is string {
  if (typeof value !== 'string' || !value.trim() || /[\p{Cc}\p{Cf}]/u.test(value)) {
    throw new Error(`${label} must be a nonblank command string without controls`);
  }
}

export function documentCommand(config: WorkCardsConfig, env: NodeJS.ProcessEnv = process.env): string {
  const command = config.document_command ?? (env.EDITOR?.trim() ? env.EDITOR : 'less');
  validateDocumentCommand(command, config.document_command !== null ? 'document_command' : 'EDITOR');
  return command;
}

export function configPath(env: NodeJS.ProcessEnv = process.env): string {
  return join(env.PI_CODING_AGENT_DIR || join(env.HOME || homedir(), '.pi', 'agent'), 'work-cards.json');
}

export async function loadConfig(path = configPath()): Promise<WorkCardsConfig> {
  let raw: string;
  try { raw = await readFile(path, 'utf8'); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return { ...DEFAULT_CONFIG };
    throw new Error(`Cannot read work-cards config ${path}: ${(error as Error).message}`, { cause: error });
  }
  try {
    const input: unknown = JSON.parse(raw);
    if (!input || typeof input !== 'object' || Array.isArray(input)) throw new Error('expected a JSON object');
    for (const [key, value] of Object.entries(input)) {
      if (!Object.hasOwn(DEFAULT_CONFIG, key)) throw new Error(`unknown field ${key}`);
      if (key === 'model') {
        if (typeof value !== 'string' || !/^[^\s/\p{Cc}\p{Cf}]+\/[^\s\p{Cc}\p{Cf}]+$/u.test(value)) {
          throw new Error('model must be provider/model or a configured helpers/alias');
        }
      } else if (key === 'document_command') {
        if (value !== null) validateDocumentCommand(value, key);
      } else if (key === 'thinking_level') {
        if (value !== null && (typeof value !== 'string' || !['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'].includes(value))) {
          throw new Error('thinking_level must be null or off, minimal, low, medium, high, xhigh, max');
        }
      } else if (key === 'update_guidance') {
        if (typeof value !== 'string' || !value.trim() || /[\p{Cc}\p{Cf}]/u.test(value.replace(/\n/g, ''))) {
          throw new Error('update_guidance must be a non-empty string without controls (newlines allowed)');
        }
      } else if (key === 'pane_height_fraction') {
        if (typeof value !== 'number' || !Number.isFinite(value) || value <= 0 || value >= 1) {
          throw new Error('pane_height_fraction must be a finite number strictly between 0 and 1');
        }
      } else {
        const minimum = key === 'check_every_n_responses' || key === 'check_every_n_cycles' ? 0 : 1;
        if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < minimum) {
          throw new Error(`${key} must be an integer >= ${minimum}`);
        }
        if (['pane_poll_interval_ms', 'request_timeout_ms', 'herdr_timeout_ms'].includes(key) && value > 2147483647) {
          throw new Error(`${key} must be <= 2147483647 (Node timer maximum)`);
        }
      }
    }
    return { ...DEFAULT_CONFIG, ...input };
  } catch (error) {
    throw new Error(`Invalid work-cards config ${path}: ${(error as Error).message}`, { cause: error });
  }
}
