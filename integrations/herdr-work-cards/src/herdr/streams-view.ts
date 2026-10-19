import { isDeepStrictEqual } from 'node:util';
import { checkoutLabels } from '../presentation.js';
import { renderDisplayCard, type RenderedCard } from './view.js';
import { sameStreamsScope, type StreamsCardScope, type StreamsCardSnapshot, type StreamsCardSource } from './streams-source.js';

export interface StreamsDocumentTarget {
  scope: StreamsCardScope; repository: string; repositoryUrl: string; ref: string; added_at?: string; path: string;
  checkout_id: string | null; root: string | null; file: string | null; error: string | null;
}
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
    const label = `${qualifier ? `${qualifier === '/' ? '' : qualifier}/` : ''}${document.path}${document.added_at ? ` (added ${document.added_at.slice(0, 8)})` : ''}`;
    return { label, target: {
      scope: { ...scope }, repositoryUrl: stream.repositories[document.repository].url,
      ref: document.ref, ...(document.added_at ? { added_at: document.added_at } : {}), ...location,
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
    if (key === 'q' || key === '\x03') return { kind: 'exit' };
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
