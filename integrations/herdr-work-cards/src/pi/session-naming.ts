import type { ExtensionAPI, ExtensionContext } from '@earendil-works/pi-coding-agent';

type NamingAPI = Pick<ExtensionAPI, 'getSessionName' | 'setSessionName'>;
type Session = Pick<ExtensionContext['sessionManager'], 'getSessionId'>;
export function projectSessionName(pi: NamingAPI, manager: Session, sessionId: string, title: string, assertCurrent: () => void): 'renamed' | 'unchanged' {
  if (!title.trim()) throw new Error('Catalog session title must be nonempty');
  const observed = pi.getSessionName();
  assertCurrent();
  if (manager.getSessionId() !== sessionId) throw new Error('Stale Pi naming session');
  if (observed === title) return 'unchanged';
  pi.setSessionName(title);
  assertCurrent();
  if (manager.getSessionId() !== sessionId || pi.getSessionName() !== title) throw new Error('Pi session name application was not acknowledged');
  return 'renamed';
}
