import stringWidth from 'string-width';
import type { CardStatus } from '../records/schema.js';

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
function artifactNumber(value: string, kind: 'Issue' | 'PR'): string | undefined {
  const safe = webUrl(value);
  if (!safe) return undefined;
  const path = new URL(safe).pathname;
  // Repository issue/PR paths only: CI run IDs and query/fragment numbers are not artifact numbers.
  const pattern = kind === 'PR'
    ? /^(?:\/[^/]+\/[^/]+\/pulls?|\/.+\/-\/merge_requests)\/([1-9]\d*)(?:\/|$)/
    : /^(?:\/[^/]+\/[^/]+\/issues|\/.+\/-\/issues)\/([1-9]\d*)(?:\/|$)/;
  return pattern.exec(path)?.[1];
}
export interface DisplayCard<T> {
  title: string; status: CardStatus;
  goal: string; now: string; remaining: string;
  documents: { label: string; target: T }[];
  issues: { url: string; description: string; label?: string }[];
  prs: { url: string; description: string; label?: string }[];
}
export interface RenderedCard<T> { text: string; documents: Map<string, T> }
export function renderDisplayCard<T>(card: DisplayCard<T> | undefined, columns: number, rows: number, notice?: string): RenderedCard<T> {
  const height = Math.max(0, Math.floor(rows)), width = Math.max(0, Math.floor(columns));
  const documents = new Map<string, T>();
  if (!height || !width) return { text: '', documents };
  const footer = width >= 9 ? '[q] close' : width >= 3 ? '[q]' : '';
  const contentHeight = height - Number(notice !== undefined);
  type Span = { text: string; code?: number; url?: string; key?: string; target?: T };
  type Row = { spans: (width: number) => Span[]; items?: number };
  const lines: Row[] = [];
  const textRow = (text: string, code?: number): Row => ({ spans: w => [{ text: clipped(text, w), code }] });
  if (card && contentHeight) {
    const statuses = { active: ['●', 36], waiting: ['○', 33], blocked: ['!', 31], parked: ['○', 2], done: ['✓', 32] } as const;
    const [symbol, colour] = statuses[card.status];
    const status = `${symbol} ${card.status}`, statusWidth = stringWidth(status);
    const entries = card.documents.map((document, index) => ({ path: plain(document.label), target: document.target, key: documentKeys[index] }));
    const lengths = entries.map(entry => stringWidth((entry.key ? `[${entry.key}] ` : '') + entry.path));
    let gridColumns = 1, columnWidths = [width];
    // Try complete labels first; only the one-column fallback needs truncation.
    for (let count = Math.min(entries.length, Math.floor((width + 2) / 2)); count >= 2; count--) {
      const widths = Array<number>(count).fill(0);
      lengths.forEach((length, index) => { widths[index % count] = Math.max(widths[index % count]!, length); });
      if (widths.reduce((sum, cells) => sum + cells, 0) + 2 * (count - 1) <= width) {
        gridColumns = count; columnWidths = widths; break;
      }
    }
    const references: Row[] = [];
    for (let index = 0; index < entries.length; index += gridColumns) {
      const cells = entries.slice(index, index + gridColumns);
      references.push({ items: cells.length, spans: w => {
        const spans: Span[] = [];
        let offset = 0, paintedWidth = 0;
        for (const [column, entry] of cells.entries()) {
          if (offset >= w) break;
          if (offset > paintedWidth) spans.push({ text: ' '.repeat(offset - paintedWidth) });
          const room = Math.min(columnWidths[column]!, w - offset);
          const keyLabel = entry.key ? prefix(`[${entry.key}]`, room) : '';
          const space = keyLabel && room > stringWidth(keyLabel) ? ' ' : '';
          const path = fit(entry.path, room - stringWidth(keyLabel + space));
          const slash = graphemeBoundary(path, path.lastIndexOf('/') + 1);
          spans.push({ text: keyLabel, code: 36, key: entry.key, target: entry.target }, { text: space },
            { text: path.slice(0, slash), code: 2 }, { text: path.slice(slash) });
          paintedWidth = offset + stringWidth(keyLabel + space + path);
          offset += columnWidths[column]! + 2;
        }
        return spans;
      } });
    }
    const linkRow = (link: DisplayCard<T>['issues'][number], index: number, kind: 'Issue' | 'PR') => {
      const number = artifactNumber(link.url, kind);
      return { label: plain(`[${index + 1}] ${kind}${number ? ` #${number}` : ''}: ${link.label || link.description || link.url}`), url: webUrl(link.url) };
    };
    const links = [
      ...card.issues.map((link, index) => linkRow(link, index, 'Issue')),
      ...card.prs.map((link, index) => linkRow(link, card.issues.length + index, 'PR')),
    ];
    for (const link of links) references.push({ items: 1, spans: w => [{ text: fit(link.label, w), code: 36, url: link.url }] });
    const available = contentHeight - lines.length;
    let stateLimit = references.length ? (available < 2 ? available : Math.min(Math.ceil(available / 2), Math.max(1, available - Math.min(2, references.length)))) : available;
    const states = [
      { label: '', body: card.goal, ownStatus: false, goal: true },
      { label: card.title, body: card.now, ownStatus: true, goal: false },
      { label: plain(card.remaining).trim() ? 'Remaining' : '', body: card.remaining, ownStatus: false, goal: false },
    ].map(section => ({ ...section, label: plain(section.label).trim(), body: plain(section.body).trim() }))
      .filter(section => section.label || section.body || section.ownStatus);
    // Keep each nonempty section visible without taking the reserved reference slots.
    stateLimit = Math.max(stateLimit, Math.min(states.length, available - Math.min(2, references.length)));
    let budget = stateLimit, goalEnd = 0, workEnd = 0;
    for (const [index, section] of states.entries()) {
      if (!budget) break;
      const { label, body, ownStatus } = section;
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
        const emphasized = graphemeBoundary(text, headingText.length);
        const spans: Span[] = section.goal ? [{ text, code: 2 }]
          : [{ text: text.slice(0, emphasized), code: 1 }, { text: text.slice(emphasized) }];
        if (showStatus) spans.push({ text: ' '.repeat(w - stringWidth(text) - statusWidth) }, { text: status, code: colour });
        return spans;
      } })));
      budget -= visibleState.length;
      if (section.goal) goalEnd = lines.length;
      else workEnd = lines.length;
    }
    const slots = contentHeight - lines.length;
    const visible = references.slice(0, Math.max(0, slots - Number(references.length > slots)));
    lines.push(...visible);
    const hidden = references.slice(visible.length).reduce((sum, row) => sum + row.items!, 0);
    if (hidden && lines.length < contentHeight) lines.push(textRow(`… ${hidden} more; enlarge pane`, 2));
    // Separators consume only surplus padding, never visible content or an empty footer row.
    const boundaries = [];
    if (goalEnd && workEnd > goalEnd) boundaries.push(goalEnd);
    if (workEnd > goalEnd && lines.length > workEnd) boundaries.push(workEnd);
    const spare = contentHeight - lines.length - Number(notice === undefined && !!footer && lines.length < contentHeight);
    for (const boundary of boundaries.slice(0, Math.max(0, spare)).reverse()) lines.splice(boundary, 0, textRow(''));
  }
  while (lines.length < contentHeight) lines.push(textRow(''));
  if (notice !== undefined) lines.push(textRow(notice));
  const painted = lines.map((line, index) => {
    const hint = index === height - 1 ? footer : '';
    const room = width - stringWidth(hint), spans = line.spans(room);
    for (const span of spans) if (span.key && span.text === `[${span.key}]`) documents.set(span.key, span.target!);
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
