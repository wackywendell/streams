import { basename, dirname, isAbsolute } from 'node:path';
import stringWidth from 'string-width';
import { readCard, type CardSnapshot, type CardStatus } from '../card.js';
import { parseStateSections, type StateSections } from '../state.js';

const documentKeys = 'abcdefghijklmnoprstuvwxyz';
export function plain(text: string): string { return text.toWellFormed().replace(/[\p{Cc}\p{Cf}]/gu, ''); }
const graphemes = new Intl.Segmenter(undefined, { granularity: 'grapheme' });
const style = (text: string, code: number): string => text ? `\x1b[${code}m${text}\x1b[0m` : '';

// These helpers measure sanitized text only; trusted terminal sequences are added afterwards.
function prefix(text: string, columns: number): string {
  let result = '', width = 0;
  for (const { segment } of graphemes.segment(text)) {
    const cells = stringWidth(segment);
    if (width + cells > columns) break;
    result += segment; width += cells;
  }
  return result;
}
function graphemeBoundary(text: string, offset: number): number {
  if (offset <= 0) return 0;
  for (const { index, segment } of graphemes.segment(text)) {
    const end = index + segment.length;
    if (end >= offset) return end;
  }
  return text.length;
}
function fit(text: string, columns: number): string {
  if (columns <= 0) return '';
  return stringWidth(text) <= columns ? text : prefix(text, columns - 1) + '…';
}
function clipped(text: string, columns: number): string { return prefix(plain(text), Math.max(0, columns)); }
function wrap(text: string, columns: number): string[] {
  const lines: string[] = [];
  let line = '';
  for (const word of text.trim().split(/\s+/u).filter(Boolean)) {
    if (line && stringWidth(`${line} ${word}`) <= columns) { line += ` ${word}`; continue; }
    if (line) { lines.push(line); line = ''; }
    for (const { segment } of graphemes.segment(word)) {
      const cells = stringWidth(segment);
      if (stringWidth(line) + cells > columns && line) { lines.push(line); line = ''; }
      // A two-cell grapheme cannot be painted in a one-cell pane.
      line += cells > columns ? '…' : segment;
    }
  }
  if (line) lines.push(line);
  return lines;
}
function webUrl(value: string): string | undefined {
  try {
    if (/[\s\\\p{Cc}\p{Cf}]/u.test(value) || /[\p{Cc}\p{Cf}]/u.test(decodeURIComponent(value))) return undefined;
    const url = new URL(value);
    return ['http:', 'https:'].includes(url.protocol) && url.hostname ? url.href : undefined;
  } catch { return undefined; }
}
interface DocumentTarget { cwd: string; path: string }
export interface DisplayCard<T> {
  title: string; workspaceTitle?: string; status: CardStatus;
  state: { sections: StateSections } | { prose: string };
  documents: { label: string; target: T }[];
  issues: { url: string; description: string }[];
  prs: { url: string; description: string }[];
}
export interface RenderedCard<T = DocumentTarget> { text: string; documents: Map<string, T> }
export function renderCard(snapshot: CardSnapshot | undefined, columns: number, rows: number, notice?: string): RenderedCard {
  const card = snapshot?.card;
  const sections = card && parseStateSections(card.state);
  return renderDisplayCard(card ? {
    title: card.title, status: card.status, state: sections ? { sections } : { prose: card.state },
    documents: card.documents.map(document => ({ label: document.path, target: { cwd: card.cwd, path: document.path } })),
    issues: card.issues, prs: card.prs,
  } : undefined, columns, rows, notice);
}
export function renderDisplayCard<T>(card: DisplayCard<T> | undefined, columns: number, rows: number, notice?: string): RenderedCard<T> {
  const height = Math.max(0, Math.floor(rows)), width = Math.max(0, Math.floor(columns));
  const documents = new Map<string, T>();
  if (!height || !width) return { text: '', documents };
  const footer = width >= 9 ? '[q] close' : width >= 3 ? '[q]' : '';
  const contentHeight = height - Number(notice !== undefined);
  type Span = { text: string; code?: number; url?: string };
  type Row = { spans: (width: number) => Span[]; key?: string; target?: T };
  const lines: Row[] = [];
  const textRow = (text: string, code?: number): Row => ({ spans: w => [{ text: clipped(text, w), code }] });
  if (card && contentHeight) {
    const statuses = { active: ['●', 36], waiting: ['○', 33], blocked: ['!', 31], parked: ['○', 2], done: ['✓', 32] } as const;
    const [symbol, colour] = statuses[card.status];
    const status = `${symbol} ${card.status}`, statusWidth = stringWidth(status), title = plain(card.title);
    const sections = 'sections' in card.state ? card.state.sections : undefined;
    if (!sections) lines.push({ spans: w => {
      if (w < statusWidth + 3) return [{ text: fit(title, w), code: 3 }];
      const label = fit(title, w - statusWidth - 2);
      return [{ text: label, code: 3 }, { text: ' '.repeat(w - stringWidth(label) - statusWidth) }, { text: status, code: colour }];
    } });
    const references = [
      ...card.documents.map((document, index) => ({ path: document.label, target: document.target, key: documentKeys[index], label: '', url: undefined as string | undefined })),
      ...card.issues.map((link, index) => ({ label: plain(`[${index + 1}] Issue: ${link.description || link.url}`), url: webUrl(link.url), path: undefined, target: undefined, key: undefined })),
      ...card.prs.map((link, index) => ({ label: plain(`[${card.issues.length + index + 1}] PR: ${link.description || link.url}`), url: webUrl(link.url), path: undefined, target: undefined, key: undefined })),
    ];
    const available = contentHeight - lines.length;
    let stateLimit = references.length ? (available < 2 ? available : Math.min(Math.ceil(available / 2), Math.max(1, available - Math.min(2, references.length)))) : available;
    if (sections) {
      const shared = card.workspaceTitle !== undefined;
      const states = [
        { label: shared ? card.workspaceTitle! : card.title, body: sections.goal, italic: 'title', ownStatus: false },
        { label: shared ? card.title : (sections.now ? 'Now' : ''), body: sections.now, italic: shared ? 'title' : undefined, ownStatus: true },
        { label: '', body: sections.remaining, italic: 'all', ownStatus: false },
      ].map(section => ({ ...section, label: plain(section.label).trim(), body: plain(section.body).trim() }))
        .filter(section => section.label || section.body || section.ownStatus);
      // Keep each nonempty section visible without taking the reserved reference slots.
      stateLimit = Math.max(stateLimit, Math.min(states.length, available - Math.min(2, references.length)));
      let budget = stateLimit;
      for (const [index, section] of states.entries()) {
        if (!budget) break;
        const { label, body, italic, ownStatus } = section;
        const sectionWidth = ownStatus && width >= statusWidth + 3 ? width - statusWidth - 2 : width;
        const heading = label + (label && body ? ':' : '');
        const state = wrap(heading + (heading && body ? ' ' : '') + body, sectionWidth);
        if (!state.length && ownStatus) state.push('');
        const headings = wrap(heading, sectionWidth);
        const reserved = Math.min(states.length - index - 1, budget - 1);
        const visibleState = state.slice(0, budget - reserved);
        if (state.length > visibleState.length || (budget === visibleState.length && index < states.length - 1)) {
          const last = visibleState.length - 1;
          visibleState[last] = prefix(visibleState[last]!, sectionWidth - 1).trimEnd() + '…';
        }
        lines.push(...visibleState.map((line, row) => ({ spans: (w: number): Span[] => {
          const showStatus = ownStatus && row === 0 && w >= statusWidth + 3;
          const text = clipped(line, showStatus ? w - statusWidth - 2 : w);
          const headingText = headings[row] ?? '';
          const titleLength = headingText.length - Number(Boolean(label && body) && row === headings.length - 1);
          const emphasized = italic === 'all' ? text.length : italic === 'title' ? graphemeBoundary(text, titleLength) : 0;
          const spans: Span[] = [{ text: text.slice(0, emphasized), code: 3 }, { text: text.slice(emphasized) }];
          if (showStatus) spans.push({ text: ' '.repeat(w - stringWidth(text) - statusWidth) }, { text: status, code: colour });
          return spans;
        } })));
        budget -= visibleState.length;
      }
    } else {
      const state = wrap(plain('prose' in card.state ? card.state.prose : ''), width), visibleState = state.slice(0, stateLimit);
      if (state.length > visibleState.length && visibleState.length) {
        const last = visibleState.length - 1;
        visibleState[last] = prefix(visibleState[last], width - 1).trimEnd() + '…';
      }
      lines.push(...visibleState.map(line => textRow(line)));
    }
    const slots = contentHeight - lines.length;
    const visible = references.slice(0, Math.max(0, slots - Number(references.length > slots)));
    for (const entry of visible) {
      if (entry.path !== undefined) {
        lines.push({ key: entry.key, target: entry.target, spans: w => {
          const keyLabel = entry.key ? prefix(`[${entry.key}]`, w) : '';
          const space = keyLabel && w > stringWidth(keyLabel) ? ' ' : '';
          const path = fit(plain(entry.path!), w - stringWidth(keyLabel + space));
          const slash = path.lastIndexOf('/') + 1;
          return [{ text: keyLabel, code: 36 }, { text: space }, { text: path.slice(0, slash), code: 2 }, { text: path.slice(slash), code: 1 }];
        } });
      } else lines.push({ spans: w => [{ text: fit(entry.label, w), code: 36, url: entry.url }] });
    }
    if (references.length > visible.length && lines.length < contentHeight) lines.push(textRow(`… ${references.length - visible.length} more; enlarge pane`, 2));
  }
  while (lines.length < contentHeight) lines.push(textRow(''));
  if (notice !== undefined) lines.push(textRow(notice));
  const painted = lines.map((line, index) => {
    const hint = index === height - 1 ? footer : '';
    const room = width - stringWidth(hint), spans = line.spans(room);
    if (line.key && spans[0]?.text === `[${line.key}]`) documents.set(line.key, line.target!);
    // Geometry and action visibility are settled on plain spans before adding trusted sequences.
    if (hint) spans.push({ text: ' '.repeat(room - spans.reduce((n, span) => n + stringWidth(span.text), 0)) }, { text: hint, code: 2 });
    return spans.map(span => {
      const text = span.code === undefined ? span.text : style(span.text, span.code);
      return span.url && text ? `\x1b]8;;${span.url}\x1b\\${text}\x1b]8;;\x1b\\` : text;
    }).join('');
  });
  return { text: painted.join('\n'), documents };
}

// Recognize standalone keys only, never keys embedded in terminal strings or paste payloads.
export class KeyInput {
  private state = 'text';
  private parameters = '';
  private paste = false;
  feed(data: Buffer): string[] {
    const keys: string[] = [];
    for (const byte of data) {
      if (this.state === 'escape') {
        this.state = ({ 91: 'csi', 79: 'ss3', 27: 'escape' } as Record<number, string>)[byte] ?? 'text';
        if ([93, 80, 88, 94, 95].includes(byte)) this.state = 'string';
        this.parameters = '';
      } else if (this.state === 'csi' || this.state === 'ss3') {
        if (byte === 27) this.state = 'escape';
        else if (byte >= 64 && byte <= 126) {
          if (this.state === 'csi' && byte === 126) {
            if (this.parameters === '200') this.paste = true;
            if (this.parameters === '201') this.paste = false;
          }
          this.state = 'text';
        } else if (this.parameters.length < 16) this.parameters += String.fromCharCode(byte);
      } else if (this.state === 'string') {
        if (byte === 7) this.state = 'text';
        if (byte === 27) this.state = 'string-escape';
      } else if (this.state === 'string-escape') {
        this.state = byte === 92 ? 'text' : 'string';
      } else if (byte === 27) this.state = 'escape';
      else if (!this.paste && (byte === 3 || documentKeys.includes(String.fromCharCode(byte)) || byte === 113)) keys.push(String.fromCharCode(byte));
    }
    return keys;
  }
}

interface Notice { source: 'card' | 'projection' | 'document'; text: string }
export type KeyResult = { kind: 'exit' } | { kind: 'none' } | { kind: 'open-document'; snapshot: CardSnapshot; relativePath: string };
export class CardView {
  current?: CardSnapshot;
  private activeNotice?: Readonly<Notice>;
  get notice(): Readonly<Notice> | undefined { return this.activeNotice; }
  setNotice(source: Notice['source'], text: string): void { this.activeNotice = { source, text }; }
  clearNotice(source?: Notice['source']): void {
    if (!source || this.activeNotice?.source === source) this.activeNotice = undefined;
  }
  private documents = new Map<string, DocumentTarget>();
  private columns = 80;
  private rows = 0;
  private paintedNotice?: string;
  constructor(readonly path: string) {
    if (!isAbsolute(path) || !path.endsWith('.yaml')) throw new Error('WORK_CARDS_CARD must be an absolute card YAML path');
  }
  async refresh(): Promise<CardSnapshot | undefined> {
    try {
      const next = await readCard(basename(this.path, '.yaml'), dirname(this.path));
      this.current = next;
      this.clearNotice('card');
      return next;
    } catch (error) {
      this.current = undefined; this.documents.clear();
      this.setNotice('card', `Card unavailable (${this.path}): ${(error as Error).message}. Fix the saved card and retry.`);
      return undefined;
    }
  }
  render(columns: number, rows: number): string {
    this.columns = columns;
    this.rows = rows;
    this.paintedNotice = this.notice?.text;
    const rendered = renderCard(this.current, columns, rows, this.paintedNotice);
    this.documents = rendered.documents;
    return rendered.text;
  }
  async key(key: string): Promise<KeyResult> {
    if (key === '\x03' || key === 'q') return { kind: 'exit' };
    const displayed = this.documents.get(key);
    if (!displayed) return { kind: 'none' };
    // Validate against the layout actually painted, including its reserved notice row.
    const rows = this.rows, notice = this.paintedNotice;
    this.clearNotice();
    await this.refresh();
    if (!this.current) return { kind: 'none' };
    const next = renderCard(this.current, this.columns, rows, notice).documents.get(key);
    if (!next || next.cwd !== displayed.cwd || next.path !== displayed.path) {
      this.setNotice('document', 'Document checkout/target changed; redraw and press the displayed shortcut again.');
      return { kind: 'none' };
    }
    return { kind: 'open-document', snapshot: this.current, relativePath: displayed.path };
  }
}
