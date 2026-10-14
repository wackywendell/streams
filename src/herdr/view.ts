import { constants } from 'node:fs';
import { open } from 'node:fs/promises';
import { basename, dirname, isAbsolute } from 'node:path';
import { readCard, resolveDocument, type CardSnapshot } from '../card.js';
import type { WorkCardsConfig } from '../config.js';

const documentKeys = '123456789abcdefghijklmnoprstuvwxyz';
export function plain(text: string): string { return text.replace(/[\p{Cc}\p{Cf}]/gu, ''); }
function clipped(text: string, columns: number): string { return Array.from(plain(text)).slice(0, columns).join(''); }
function webUrl(value: string): string | undefined {
  try {
    if (/[\s\\\p{Cc}\p{Cf}]/u.test(value) || /[\p{Cc}\p{Cf}]/u.test(decodeURIComponent(value))) return undefined;
    const url = new URL(value);
    return ['http:', 'https:'].includes(url.protocol) && url.hostname ? url.href : undefined;
  } catch { return undefined; }
}
interface DocumentTarget { cwd: string; path: string }
export interface RenderedCard { text: string; documents: Map<string, DocumentTarget> }
export function renderCard(snapshot: CardSnapshot, columns: number, rows: number): RenderedCard {
  const card = snapshot.card;
  const entries: { label: string; url?: string; key?: string; path?: string }[] = [
    { label: `${card.title} · ${card.status}` }, { label: card.state },
    ...card.documents.map((document, index) => ({ label: `${documentKeys[index] ? `[${documentKeys[index]}] ` : ''}${document.description || document.path}`,
      key: documentKeys[index], path: document.path })),
    ...card.issues.map(link => ({ label: `Issue: ${link.description || link.url}`, url: webUrl(link.url) })),
    ...card.prs.map(link => ({ label: `PR: ${link.description || link.url}`, url: webUrl(link.url) })),
  ];
  const height = Math.max(0, rows), width = Math.max(0, columns);
  const visible = entries.slice(0, Math.max(0, height - Number(entries.length > height)));
  const documents = new Map<string, DocumentTarget>();
  const lines = visible.map(entry => {
    const label = clipped(entry.label, width);
    // A clipped-away key is not an actionable document.
    if (entry.key && entry.path && label.startsWith(`[${entry.key}]`)) documents.set(entry.key, { cwd: card.cwd, path: entry.path });
    return entry.url && label ? `\x1b]8;;${entry.url}\x1b\\${label}\x1b]8;;\x1b\\` : label;
  });
  if (entries.length > height && height) lines.push(clipped(`… ${entries.length - visible.length} more; enlarge pane`, width));
  return { text: lines.join('\n'), documents };
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
interface Preview { path: string; text: string; truncated: boolean; bytesRead: number; offset: number }
export class CardView {
  current?: CardSnapshot;
  preview?: Preview;
  private activeNotice?: Readonly<Notice>;
  get notice(): Readonly<Notice> | undefined { return this.activeNotice; }
  setNotice(source: Notice['source'], text: string): void { this.activeNotice = { source, text }; }
  clearNotice(source?: Notice['source']): void {
    if (!source || this.activeNotice?.source === source) this.activeNotice = undefined;
  }
  private documents = new Map<string, DocumentTarget>();
  private columns = 80;
  private rows = 24;
  constructor(readonly path: string, readonly config: WorkCardsConfig) {
    if (!isAbsolute(path) || !path.endsWith('.yaml')) throw new Error('WORK_CARDS_CARD must be an absolute card YAML path');
  }
  async refresh(): Promise<CardSnapshot | undefined> {
    try {
      const next = await readCard(basename(this.path, '.yaml'), dirname(this.path));
      if (this.current && next.revision !== this.current.revision) this.preview = undefined;
      this.current = next;
      this.clearNotice('card');
      return next;
    } catch (error) {
      this.current = undefined; this.preview = undefined; this.documents.clear();
      this.setNotice('card', `Card unavailable (${this.path}): ${(error as Error).message}. Fix the saved card and retry.`);
      return undefined;
    }
  }
  render(columns: number, rows: number): string {
    this.columns = columns; this.rows = rows;
    const height = Math.max(0, rows - Number(Boolean(this.notice)));
    let text: string;
    if (this.preview) {
      const lines = this.preview.text.split('\n'), visibleRows = Math.max(0, height - 1);
      this.preview.offset = Math.min(this.preview.offset, Math.max(0, lines.length - Math.max(1, visibleRows)));
      const end = Math.min(lines.length, this.preview.offset + visibleRows);
      const position = `${visibleRows ? this.preview.offset + 1 : 0}-${visibleRows ? end : 0}/${lines.length}`;
      const header = `${this.preview.truncated ? 'byte-capped · ' : ''}${position} [q] back j/k · ${this.preview.path}`;
      text = [header, ...lines.slice(this.preview.offset, end)].slice(0, height).map(line => clipped(line, columns)).join('\n');
      this.documents.clear();
    } else if (this.current) {
      const rendered = renderCard(this.current, columns, height); this.documents = rendered.documents; text = rendered.text;
    } else { this.documents.clear(); text = ''; }
    if (this.notice && rows) text += `${text ? '\n' : ''}${clipped(this.notice.text, columns)}`;
    return text;
  }
  async key(key: string): Promise<boolean> {
    if (key === '\x03') return true;
    if (key === 'q') {
      if (!this.preview) return true;
      this.preview = undefined; this.clearNotice(); return false;
    }
    if (this.preview) {
      const visibleRows = Math.max(1, this.rows - Number(Boolean(this.notice)) - 1);
      const maximum = Math.max(0, this.preview.text.split('\n').length - visibleRows);
      if (key === 'j') this.preview.offset = Math.min(maximum, this.preview.offset + 1);
      if (key === 'k') this.preview.offset = Math.max(0, this.preview.offset - 1);
      return false;
    }
    const displayed = this.documents.get(key);
    if (!displayed) return false;
    this.clearNotice();
    await this.refresh();
    if (!this.current) return false;
    const next = renderCard(this.current, this.columns, this.rows).documents.get(key);
    if (!next || next.cwd !== displayed.cwd || next.path !== displayed.path) {
      this.setNotice('document', 'Document checkout/target changed; redraw and press the displayed shortcut again.');
      return false;
    }
    try {
      const path = await resolveDocument(this.current.card, displayed.path);
      const file = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
      try {
        const info = await file.stat();
        if (!info.isFile()) throw new Error('Preview target must be a regular file');
        const bytes = Buffer.alloc(Math.min(info.size, this.config.max_document_bytes));
        const { bytesRead } = await file.read(bytes, 0, bytes.length, 0);
        this.preview = { path: displayed.path, text: new TextDecoder().decode(bytes.subarray(0, bytesRead)), truncated: info.size > bytesRead, bytesRead, offset: 0 };
        this.documents.clear();
      } finally { await file.close(); }
    } catch (error) { this.setNotice('document', `Cannot preview ${displayed.path}: ${(error as Error).message}`); }
    return false;
  }
}
