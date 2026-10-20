import { isDeepStrictEqual } from 'node:util';
import { checkoutLabels } from '../presentation.js';
import { renderDisplayCard, type RenderedCard } from './view.js';
import { sameStreamsScope, type StreamsCardScope, type StreamsCardSnapshot, type StreamsCardSource, type StreamsDocumentLocation } from './streams-source.js';

export type StreamsDocumentTarget = StreamsDocumentLocation & {
  scope: StreamsCardScope; repositoryUrl: string; ref: string;
  commit?: string; source_session_id?: string;
};
export function renderStreamsCard(snapshot: StreamsCardSnapshot | undefined, columns: number, rows: number, notice?: string): RenderedCard<StreamsDocumentTarget> {
  if (!snapshot) return renderDisplayCard<StreamsDocumentTarget>(undefined, columns, rows, notice);
  const { stream, session, scope, checkouts } = snapshot;
  const roots = [...new Set(checkouts.map(binding => binding.root))];
  // Feed only checkout directory data to the existing distinguishing-label helper.
  const labels = checkoutLabels(Object.fromEntries(roots.map((root, index) => [String(index), { directory: root }])));
  const repositories = Object.keys(stream.repositories);
  const documents = stream.documents.map(document => {
    const location = snapshot.documentLocations.find(l => l.repository === document.repository && l.path === document.path)!;
    const root = location.root ?? undefined;
    let qualifier = '';
    if (repositories.length > 1 || roots.length > 1) {
      qualifier = root === undefined ? document.repository : labels[String(roots.indexOf(root))];
      if (root !== undefined && checkouts.some(binding => binding.repository !== document.repository && binding.root === root)) {
        qualifier = `${document.repository}:${qualifier}`;
      }
    }
    const revSuffix = location.kind === 'revision' ? ` (revision ${location.revision.slice(0, 8)})`
      : document.commit ? ` (commit ${document.commit.slice(0, 8)})` : '';
    const label = `${qualifier ? `${qualifier === '/' ? '' : qualifier}/` : ''}${document.path}${revSuffix}`;
    return { label, target: {
      scope: { ...scope }, repositoryUrl: stream.repositories[document.repository].url,
      ref: document.ref,
      ...(document.commit ? { commit: document.commit } : {}),
      ...(document.source_session_id ? { source_session_id: document.source_session_id } : {}),
      ...location,
    } };
  });
  return renderDisplayCard({ title: session.title, status: session.status, goal: stream.goal ?? '', now: session.state,
    remaining: session.remaining, documents, issues: stream.issues, prs: stream.prs }, columns, rows, notice);
}
interface Notice { source: 'card' | 'projection' | 'document'; text: string }
export type StreamsKeyResult = { kind: 'exit' } | { kind: 'none' }
  | { kind: 'open-document'; snapshot: StreamsCardSnapshot; target: StreamsDocumentTarget };
function sameTarget(left: StreamsDocumentTarget, right: StreamsDocumentTarget): boolean {
  return sameStreamsScope(left.scope, right.scope) && isDeepStrictEqual(left, right);
}
export class StreamsCardView {
  current?: StreamsCardSnapshot;
  private activeNotice?: Readonly<Notice>;
  private documents = new Map<string, StreamsDocumentTarget>();
  private chooser?: { key: string; options: { label: string; target: StreamsDocumentTarget }[]; notice?: string };
  private columns = 80;
  private rows = 0;
  private paintedNotice?: string;
  private paint = 0;
  constructor(readonly source: Pick<StreamsCardSource, 'read'>) {}
  get notice(): Readonly<Notice> | undefined { return this.activeNotice; }
  setNotice(source: Notice['source'], text: string): void { this.activeNotice = { source, text }; }
  clearNotice(source?: Notice['source']): void {
    if (!source || this.activeNotice?.source === source) this.activeNotice = undefined;
  }
  async refresh(): Promise<StreamsCardSnapshot | undefined> {
    try {
      this.current = await this.source.read();
      this.clearNotice('card');
      return this.current;
    } catch (error) {
      this.current = undefined;
      this.documents.clear();
      this.chooser = undefined;
      this.setNotice('card', `Card unavailable: ${error instanceof Error ? error.message : String(error)}. Refresh to retry.`);
      return undefined;
    }
  }
  render(columns: number, rows: number): string {
    this.paint++;
    this.columns = columns; this.rows = rows; this.paintedNotice = this.notice?.text;
    const rendered = renderStreamsCard(this.current, columns, rows, this.paintedNotice);
    this.documents = rendered.documents;
    return rendered.text;
  }
  async key(key: string): Promise<StreamsKeyResult> {
    if (key === 'q' || key === '\x03') { this.chooser = undefined; return { kind: 'exit' }; }
    if (this.chooser) {
      const active = this.chooser;
      const notice = this.paintedNotice, paint = this.paint;
      this.chooser = undefined;
      this.clearNotice('document');
      if (key === '\x1b' || !/^[1-9]$/.test(key)) return { kind: 'none' };
      const chosen = active.options[Number(key) - 1];
      if (!chosen) return { kind: 'none' };
      const columns = this.columns, rows = this.rows;
      await this.refresh();
      if (!this.current) return { kind: 'none' };
      const next = renderStreamsCard(this.current, columns, rows, notice).documents.get(active.key);
      const validTargets = next ? [
        next,
        ...(next.alternates ?? []).map(alt => ({ ...next, ...alt, error: null } as StreamsDocumentTarget)),
      ] : [];
      if (paint !== this.paint || !next || !validTargets.some(cand => sameTarget(cand, chosen.target))) {
        this.documents.clear();
        this.setNotice('document', 'Document scope/binding/target changed; redraw and press the displayed shortcut again.');
        return { kind: 'none' };
      }
      return { kind: 'open-document', snapshot: this.current, target: chosen.target };
    }
    if (/^[A-Z]$/.test(key) && key !== 'Q') {
      const lower = key.toLowerCase();
      const displayed = this.documents.get(lower);
      if (!displayed) return { kind: 'none' };
      const options = displayed.alternates?.length
        ? displayed.alternates.map(alt => ({ label: alt.label, target: { ...displayed, ...alt, error: null } as StreamsDocumentTarget }))
        : displayed.kind !== null ? [{ label: displayed.kind === 'revision' ? `revision ${displayed.revision}` : `${displayed.kind} (${displayed.root.split('/').at(-1) ?? displayed.repository})`, target: displayed }] : [];
      if (!options.length) return { kind: 'none' };
      this.chooser = { key: lower, options, notice: this.paintedNotice };
      this.setNotice('document', `Open ${displayed.path}: ${options.map((o, i) => `[${i + 1}] ${o.label}`).join('  ')}  [Esc] cancel`);
      return { kind: 'none' };
    }
    const displayed = this.documents.get(key);
    if (!displayed) return { kind: 'none' };
    const columns = this.columns, rows = this.rows, notice = this.paintedNotice, paint = this.paint;
    await this.refresh();
    if (!this.current) return { kind: 'none' };
    const next = renderStreamsCard(this.current, columns, rows, notice).documents.get(key);
    if (paint !== this.paint || !next || !sameTarget(next, displayed)) {
      this.documents.clear();
      this.setNotice('document', 'Document scope/binding/target changed; redraw and press the displayed shortcut again.');
      return { kind: 'none' };
    }
    return { kind: 'open-document', snapshot: this.current, target: next };
  }
}
