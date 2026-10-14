export interface Attachment {
  socket: string;
  workspaceId: string;
  paneId: string;
  terminalId: string;
  cardIdentity: string;
  nonce: string;
}
export interface Pane { pane_id: string; terminal_id: string; workspace_id: string; tab_id: string; tokens: Readonly<Record<string, string>> }
export interface Workspace { workspace_id: string; tokens: Readonly<Record<string, string>> }
export const markerKeys = ['wc_card', 'wc_nonce', 'wc_pane', 'wc_terminal'];
export const workspaceKeys = [...markerKeys, 'wc_summary', 'wc_status'];
type OwnerTuple = { card: string; nonce: string; pane: string; terminal: string };
function ownerTuple(tokens: Readonly<Record<string, string>>): OwnerTuple {
  return { card: tokens.wc_card, nonce: tokens.wc_nonce, pane: tokens.wc_pane, terminal: tokens.wc_terminal };
}
function markedPanes(panes: readonly Pane[]): Pane[] {
  return panes.filter(p => markerKeys.some(key => Object.hasOwn(p.tokens, key)) || p.tokens.wc_role === 'card');
}

/** Live ownership deliberately does not require the canonical orphan-recovery formats. */
export function liveOwner(socket: string, identity: string, workspace: Workspace, panes: readonly Pane[]): Attachment | undefined {
  const candidates = markedPanes(panes);
  const marked = workspaceKeys.some(key => Object.hasOwn(workspace.tokens, key));
  if (!marked && !candidates.length) return undefined;
  if (candidates.length !== 1) throw new Error('Work-card ownership conflict: missing or multiple marked panes; inspect before explicit detach/open');
  const p = candidates[0], w = ownerTuple(workspace.tokens), owner = ownerTuple(p.tokens);
  if (w.card !== identity || owner.card !== identity || !w.nonce || w.nonce !== owner.nonce ||
      p.tokens.wc_role !== 'card' || w.pane !== p.pane_id || w.terminal !== p.terminal_id ||
      owner.pane !== p.pane_id || owner.terminal !== p.terminal_id) {
    throw new Error('Work-card ownership conflict: different card or inconsistent live pane/terminal markers; nothing was replaced');
  }
  return { socket, workspaceId: workspace.workspace_id, paneId: p.pane_id, terminalId: p.terminal_id, cardIdentity: identity, nonce: w.nonce };
}

export type ExplicitOwnership = { kind: 'empty' } | { kind: 'live'; attachment: Attachment } | { kind: 'recoverable-orphan' };

/** Classify explicit operations from a complete inventory without changing any markers. */
export function classifyExplicitOwner(socket: string, identity: string, workspace: Workspace, panes: readonly Pane[], caller: Pane): ExplicitOwnership {
  if (new Set(panes.map(p => p.pane_id)).size !== panes.length || new Set(panes.map(p => p.terminal_id)).size !== panes.length ||
      !panes.some(p => p.pane_id === caller.pane_id && p.terminal_id === caller.terminal_id && p.tab_id === caller.tab_id)) {
    throw new Error('Ambiguous Herdr inventory: duplicate identities or missing live caller; nothing was recovered');
  }
  const marked = workspaceKeys.some(key => Object.hasOwn(workspace.tokens, key));
  if (!marked || markedPanes(panes).length) {
    const attachment = liveOwner(socket, identity, workspace, panes);
    return attachment ? { kind: 'live', attachment } : { kind: 'empty' };
  }
  const w = ownerTuple(workspace.tokens), panePrefix = `${workspace.workspace_id}:p`;
  if (!markerKeys.every(key => Object.hasOwn(workspace.tokens, key)) || w.card !== identity ||
      !/^[0-9a-f]{64}$/.test(w.card) || !/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(w.nonce) ||
      !w.pane.startsWith(panePrefix) || w.pane.length > 80 || !/^[A-Za-z0-9]+$/.test(w.pane.slice(panePrefix.length)) ||
      !/^[A-Za-z0-9_.:-]{1,80}$/.test(w.terminal)) {
    throw new Error('Work-card ownership conflict: absent-owner markers are incomplete, malformed or belong to a different card');
  }
  if (panes.some(p => p.pane_id === w.pane || p.terminal_id === w.terminal)) {
    throw new Error('Work-card ownership conflict: the referenced pane or terminal is still live; restored shells are never reclaimed');
  }
  return { kind: 'recoverable-orphan' };
}
