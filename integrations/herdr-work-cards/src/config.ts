
export interface WorkCardsConfig {
  workspace_startup: 'automatic' | 'manual';
  workspace_naming: boolean;
  tab_naming: boolean;
  session_naming: boolean;
  document_command: string | null;
  herdr_timeout_ms: number;
  pane_height_fraction: number;
}

export const DEFAULT_CONFIG: Readonly<WorkCardsConfig> = Object.freeze({
  workspace_startup: 'automatic',
  workspace_naming: true,
  tab_naming: true,
  session_naming: true,
  document_command: null,
  herdr_timeout_ms: 3000,
  pane_height_fraction: 0.2,
});

function validateDocumentCommand(value: unknown, label: string): asserts value is string {
  if (typeof value !== 'string' || !value.trim() || /[\p{Cc}\p{Cf}]/u.test(value)) {
    throw new Error(`${label} must be a nonblank command string without controls`);
  }
}


export function parseConfig(input: unknown): WorkCardsConfig {
  if (!input || typeof input !== 'object' || Array.isArray(input)) throw new Error('expected a JSON object');
  for (const [key, value] of Object.entries(input)) {
    if (!Object.hasOwn(DEFAULT_CONFIG, key)) throw new Error(`unknown field ${key}`);
    if (key === 'workspace_startup') {
      if (value !== 'automatic' && value !== 'manual') throw new Error('workspace_startup must be automatic or manual');
    } else if (key === 'workspace_naming' || key === 'tab_naming' || key === 'session_naming') {
      if (typeof value !== 'boolean') throw new Error(`${key} must be a boolean`);
    } else if (key === 'document_command') {
      if (value !== null) validateDocumentCommand(value, key);
    } else if (key === 'pane_height_fraction') {
      if (typeof value !== 'number' || !Number.isFinite(value) || value <= 0 || value >= 1) {
        throw new Error('pane_height_fraction must be a finite number strictly between 0 and 1');
      }
    } else {
      if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 1 || value > 2147483647) {
        throw new Error(`${key} must be an integer between 1 and 2147483647 (Node timer maximum)`);
      }
    }
  }
  return { ...DEFAULT_CONFIG, ...input };
}

