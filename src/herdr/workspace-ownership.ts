import type { Pane, Workspace } from './ownership.js';
export interface NativeTab { tab_id: string; workspace_id: string; label: string; number: number }
export interface NativeLayout { tab_id: string; workspace_id: string; panes: { pane_id: string }[] }
export interface NativeSnapshot { workspaces: Workspace[]; tabs: NativeTab[]; panes: Pane[]; layouts: NativeLayout[] }
export function nativeObject(value: unknown): Record<string, any> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Unexpected Herdr object');
  return value as Record<string, any>;
}
export function nativeId(value: unknown): string {
  if (typeof value !== 'string' || !value || /[\p{Cc}\p{Cf}]/u.test(value)) throw new Error('Invalid Herdr identity');
  return value;
}
export function nativeTokens(value: unknown): Record<string, string> {
  if (value === undefined) return {};
  const tokens = nativeObject(value);
  if (Object.values(tokens).some(v => typeof v !== 'string')) throw new Error('Invalid Herdr tokens');
  return tokens;
}
export function nativePane(value: unknown): Pane {
  const p = nativeObject(value);
  return { pane_id: nativeId(p.pane_id), terminal_id: nativeId(p.terminal_id), workspace_id: nativeId(p.workspace_id), tab_id: nativeId(p.tab_id), tokens: nativeTokens(p.tokens) };
}
export function parseNativeSnapshot(value: unknown): NativeSnapshot {
  const s = nativeObject(value);
  const list = (key: string): any[] => { if (!Array.isArray(s[key])) throw new Error(`Missing Herdr ${key}`); return s[key]; };
  const workspaces = list('workspaces').map(w => ({ workspace_id: nativeId(w.workspace_id), tokens: nativeTokens(w.tokens) }));
  const tabs = list('tabs').map(t => {
    if (!Number.isSafeInteger(t.number) || t.number < 1 || typeof t.label !== 'string') throw new Error('Invalid Herdr tab label/number');
    return { tab_id: nativeId(t.tab_id), workspace_id: nativeId(t.workspace_id), label: t.label, number: t.number };
  });
  const panes = list('panes').map(nativePane);
  const layouts = list('layouts').map(l => {
    if (!Array.isArray(l.panes)) throw new Error('Missing layout panes');
    return { tab_id: nativeId(l.tab_id), workspace_id: nativeId(l.workspace_id), panes: l.panes.map((p: any) => ({ pane_id: nativeId(p.pane_id) })) };
  });
  const unique = (ids: string[]) => { if (new Set(ids).size !== ids.length) throw new Error('Duplicate Herdr identity'); };
  unique(workspaces.map(w => w.workspace_id)); unique(tabs.map(t => t.tab_id)); unique(panes.map(p => p.pane_id)); unique(panes.map(p => p.terminal_id)); unique(layouts.map(l => l.tab_id));
  for (const t of tabs) if (!workspaces.some(w => w.workspace_id === t.workspace_id)) throw new Error('Missing tab workspace');
  for (const p of panes) if (!tabs.some(t => t.tab_id === p.tab_id && t.workspace_id === p.workspace_id)) throw new Error('Missing pane tab');
  for (const t of tabs) {
    const l = layouts.find(l => l.tab_id === t.tab_id);
    if (!l || l.workspace_id !== t.workspace_id) throw new Error('Missing tab layout');
    unique(l.panes.map((p: { pane_id: string }) => p.pane_id));
    const actual = panes.filter(p => p.tab_id === t.tab_id);
    if (actual.length !== l.panes.length || actual.some(p => !l.panes.some((q: { pane_id: string }) => p.pane_id === q.pane_id))) throw new Error('Layout inventory mismatch');
  }
  if (layouts.some(l => !tabs.some(t => t.tab_id === l.tab_id))) throw new Error('Foreign layout');
  return { workspaces, tabs, panes, layouts };
}
export function locateTerminal(snapshot: NativeSnapshot, terminal: string): Pane {
  const matches = snapshot.panes.filter(p => p.terminal_id === terminal);
  if (matches.length !== 1) throw new Error('Missing or duplicate live terminal identity');
  return matches[0];
}
export const pairKeys = ['wca_agent', 'wca_nonce', 'wca_pi', 'wca_card', 'wca_role'];
export function pairMarkers(identity: string, nonce: string, agentTerminal: string, cardTerminal: string): Record<string, string> {
  return { wca_agent: identity, wca_nonce: nonce, wca_pi: agentTerminal, wca_card: cardTerminal };
}
export interface LivePair { kind: 'paired' | 'separated'; agent: Pane; card: Pane; identity: string; nonce: string }
export type PairState = LivePair | { kind: 'empty' } | { kind: 'absent'; agent: Pane; identity: string; nonce: string };
export function classifyPair(s: NativeSnapshot, identity: string, terminal: string): PairState {
  const caller = locateTerminal(s, terminal);
  if (s.panes.some(p => p.tokens.wca_pi === terminal && p.tokens.wca_agent !== identity)) throw new Error('Foreign or incomplete agent owner markers');
  const marked = s.panes.filter(p => p.tokens.wca_agent === identity);
  if (!marked.length) {
    if (pairKeys.some(k => Object.hasOwn(caller.tokens, k))) throw new Error('Foreign or incomplete pair markers');
    return { kind: 'empty' };
  }
  for (const p of marked) {
    if (!pairKeys.every(k => p.tokens[k]) || !['agent', 'card'].includes(p.tokens.wca_role)) throw new Error('Incomplete pair markers');
  }
  const agents = marked.filter(p => p.tokens.wca_role === 'agent');
  const cards = marked.filter(p => p.tokens.wca_role === 'card');
  if (agents.length !== 1 || cards.length > 1) throw new Error('Conflicting duplicate live owners');
  const agent = agents[0], a = agent.tokens;
  if (agent.terminal_id !== a.wca_pi || (terminal !== a.wca_pi && terminal !== a.wca_card)) throw new Error('Foreign agent terminal');
  const referenced = s.panes.find(p => p.terminal_id === a.wca_card);
  if (!cards.length) {
    if (referenced) throw new Error('Foreign or incomplete live card markers');
    return { kind: 'absent', agent, identity, nonce: a.wca_nonce };
  }
  const card = cards[0];
  if (card.terminal_id !== a.wca_card || ['wca_agent', 'wca_nonce', 'wca_pi', 'wca_card'].some(k => card.tokens[k] !== a[k])) throw new Error('Contradictory pair markers');
  return { kind: agent.workspace_id === card.workspace_id && agent.tab_id === card.tab_id ? 'paired' : 'separated', agent, card, identity, nonce: a.wca_nonce };
}
export function firstPane(s: NativeSnapshot, p: Pane): boolean { return s.layouts.find(l => l.tab_id === p.tab_id)?.panes[0]?.pane_id === p.pane_id; }
