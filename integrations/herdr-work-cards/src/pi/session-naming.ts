import type { ExtensionAPI, ExtensionContext } from '@earendil-works/pi-coding-agent';

export const sessionNamingEntry = 'work-cards.session-naming';
type NamingAPI = Pick<ExtensionAPI, 'getSessionName' | 'setSessionName' | 'appendEntry'>;
type Session = ExtensionContext['sessionManager'];
type Provenance = { version: 1; sessionId: string } & (
  | { mode: 'auto'; name: string; sessionInfoId: string }
  | { mode: 'manual'; name: string; sessionInfoId?: string }
);
type State = { kind: 'absent' | 'malformed' } | { kind: 'valid' | 'foreign'; value: Provenance };
function provenance(value: unknown): Provenance | undefined {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return;
  const p = value as Record<string, unknown>;
  if (p.version !== 1 || typeof p.sessionId !== 'string' || !p.sessionId) return;
  if (p.sessionInfoId !== undefined && (typeof p.sessionInfoId !== 'string' || !p.sessionInfoId)) return;
  if (typeof p.name !== 'string') return;
  if (p.mode === 'auto' && typeof p.sessionInfoId === 'string' && p.sessionInfoId) return { version: 1, sessionId: p.sessionId, mode: 'auto', name: p.name, sessionInfoId: p.sessionInfoId };
  if (p.mode === 'manual') return { version: 1, sessionId: p.sessionId, mode: 'manual', name: p.name, ...(typeof p.sessionInfoId === 'string' ? { sessionInfoId: p.sessionInfoId } : {}) };
}
function latestState(manager: Session, sessionId: string): State {
  const entry = manager.getEntries().filter(entry => entry.type === 'custom' && entry.customType === sessionNamingEntry).at(-1);
  if (!entry) return { kind: 'absent' };
  const value = entry.type === 'custom' ? provenance(entry.data) : undefined;
  if (!value) return { kind: 'malformed' };
  return { kind: value.sessionId === sessionId ? 'valid' : 'foreign', value };
}
function checkedState(manager: Session, sessionId: string): State {
  const state = latestState(manager, sessionId);
  if (state.kind === 'malformed') throw new Error('Malformed Pi naming state; automatic naming suspended');
  return state;
}
/** Session names are global across the entry tree, so their provenance follows file order too. */
export function projectSessionName(pi: NamingAPI, manager: Session, sessionId: string, title: string, assertCurrent: () => void, intent: 'automatic' | 'explicit' = 'automatic'): 'renamed' | 'unchanged' | 'manual' {
  if (!title.trim()) throw new Error('Catalog session title must be nonempty');
  const entries = manager.getEntries();
  const observedInfo = entries.filter(entry => entry.type === 'session_info').at(-1);
  const state = checkedState(manager, sessionId);
  const previous = state.kind === 'valid' ? state.value : undefined;
  // No asynchronous work is allowed between observing the name, checking identity, and applying it.
  const observed = pi.getSessionName() ?? '';
  assertCurrent();
  if (manager.getSessionId() !== sessionId) throw new Error('Stale Pi naming session');
  if (intent !== 'explicit' && previous?.mode === 'manual') return 'manual';
  if (intent !== 'explicit' && ((previous?.mode === 'auto' && (previous.name !== observed || previous.sessionInfoId !== observedInfo?.id)) || (!previous && (observed !== '' || observedInfo !== undefined || state.kind === 'foreign')))) {
    pi.appendEntry(sessionNamingEntry, { ...previous, version: 1, sessionId, mode: 'manual', name: observed } satisfies Provenance);
    return 'manual';
  }
  if (previous?.mode === 'auto' && observed === title && previous.name === observed && previous.sessionInfoId === observedInfo?.id) return 'unchanged';
  pi.setSessionName(title);
  const applied = manager.getEntries().filter(entry => entry.type === 'session_info').at(-1);
  assertCurrent();
  if (manager.getSessionId() !== sessionId || pi.getSessionName() !== title || !applied?.id || applied.name !== title || applied.id === observedInfo?.id) throw new Error('Pi session name application was not acknowledged');
  // A failed name write must never grant authority. A failed provenance write leaves an unproven name.
  pi.appendEntry(sessionNamingEntry, { ...previous, version: 1, sessionId, mode: 'auto', name: title, sessionInfoId: applied.id } satisfies Provenance);
  return 'renamed';
}
