import type { RecordStore, Snapshot, WorkspaceRecord, AgentRecord, DocumentIdentity } from '../records.js';
import { composeView } from '../presentation.js';
import { renderDisplayCard, type RenderedCard } from './view.js';

export interface WorkspaceCardSnapshot {
  workspace: Snapshot<WorkspaceRecord>;
  agent: Snapshot<AgentRecord>;
}
export interface WorkspaceDocumentTarget extends DocumentIdentity {
  workspaceId: string; membershipToken: string; directory: string;
}
export function renderWorkspaceCard(snapshot: WorkspaceCardSnapshot | undefined, columns: number, rows: number, notice?: string): RenderedCard<WorkspaceDocumentTarget> {
  if (!snapshot) return renderDisplayCard<WorkspaceDocumentTarget>(undefined, columns, rows, notice);
  const view = composeView(snapshot.workspace, snapshot.agent);
  return renderDisplayCard({
    title: view.title, status: view.status,
    state: { sections: { goal: view.goal, now: view.now, remaining: view.remaining } },
    documents: view.documents.map(document => ({ label: document.label, target: {
      workspaceId: snapshot.workspace.record.id, membershipToken: snapshot.agent.record.membershipToken,
      checkout: document.checkout, path: document.path, directory: snapshot.workspace.record.checkouts[document.checkout].directory,
    } })),
    issues: view.issues, prs: view.prs,
  }, columns, rows, notice);
}

interface Notice { source: 'card' | 'projection' | 'document'; text: string }
export type WorkspaceKeyResult = { kind: 'exit' } | { kind: 'none' }
  | ({ kind: 'open-document'; identity: DocumentIdentity } & WorkspaceCardSnapshot);
export class WorkspaceCardView {
  current?: WorkspaceCardSnapshot;
  private activeNotice?: Readonly<Notice>;
  private documents = new Map<string, WorkspaceDocumentTarget>();
  private columns = 80;
  private rows = 0;
  private paintedNotice?: string;
  private paintedCheckout?: string;
  constructor(readonly store: RecordStore, readonly sessionId: string) {}
  get notice(): Readonly<Notice> | undefined { return this.activeNotice; }
  setNotice(source: Notice['source'], text: string): void { this.activeNotice = { source, text }; }
  clearNotice(source?: Notice['source']): void {
    if (!source || this.activeNotice?.source === source) this.activeNotice = undefined;
  }
  async refresh(): Promise<WorkspaceCardSnapshot | undefined> {
    try {
      const before = await this.store.readAgent(this.sessionId);
      const workspace = await this.store.readWorkspace(before.record.workspaceId);
      const agent = await this.store.readAgent(this.sessionId);
      if (before.record.workspaceId !== agent.record.workspaceId || before.record.membershipToken !== agent.record.membershipToken
        || before.record.checkout !== agent.record.checkout || before.record.cwd !== agent.record.cwd) {
        throw new Error('Agent membership changed during read; refresh to load current context');
      }
      composeView(workspace, agent);
      this.current = { workspace, agent };
      this.clearNotice('card');
      return this.current;
    } catch (error) {
      this.current = undefined; this.documents.clear();
      this.setNotice('card', `Card unavailable (${this.sessionId}): ${(error as Error).message}. Refresh to retry.`);
      return undefined;
    }
  }
  render(columns: number, rows: number): string {
    this.columns = columns; this.rows = rows; this.paintedNotice = this.notice?.text;
    this.paintedCheckout = this.current?.agent.record.checkout;
    const rendered = renderWorkspaceCard(this.current, columns, rows, this.paintedNotice);
    this.documents = rendered.documents;
    return rendered.text;
  }
  async key(key: string): Promise<WorkspaceKeyResult> {
    if (key === '\x03' || key === 'q') return { kind: 'exit' };
    const displayed = this.documents.get(key);
    if (!displayed) return { kind: 'none' };
    // Retain the painted notice reservation while checking a fresh positional shortcut.
    const rows = this.rows, notice = this.paintedNotice, checkout = this.paintedCheckout;
    this.clearNotice();
    await this.refresh();
    if (!this.current) return { kind: 'none' };
    const next = renderWorkspaceCard(this.current, this.columns, rows, notice).documents.get(key);
    if (!next || this.current.agent.record.checkout !== checkout
      || next.workspaceId !== displayed.workspaceId || next.membershipToken !== displayed.membershipToken
      || next.checkout !== displayed.checkout || next.path !== displayed.path || next.directory !== displayed.directory) {
      this.setNotice('document', 'Document checkout/target changed; redraw and press the displayed shortcut again.');
      return { kind: 'none' };
    }
    return { kind: 'open-document', ...this.current, identity: { checkout: next.checkout, path: next.path } };
  }
}
