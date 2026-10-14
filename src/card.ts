import { createHash, randomUUID } from 'node:crypto';
import { constants } from 'node:fs';
import { link, mkdir, open, readdir, realpath, rename, stat, unlink } from 'node:fs/promises';
import { homedir } from 'node:os';
import { isAbsolute, join, relative, resolve, sep } from 'node:path';
import { isDeepStrictEqual } from 'node:util';
import { parseDocument, stringify } from 'yaml';

export type CardStatus = 'active' | 'waiting' | 'blocked' | 'parked' | 'done';
export interface CardDocument { path: string; description: string }
export interface CardLink { url: string; description: string }
export interface Card {
  title: string;
  status: CardStatus;
  state: string;
  cwd: string;
  documents: CardDocument[];
  issues: CardLink[];
  prs: CardLink[];
}
export type CardPatch = Partial<Pick<Card, 'status' | 'state' | 'documents' | 'issues' | 'prs'>>;
export interface CardSnapshot { slug: string; path: string; card: Card; revision: string }

const statuses = ['active', 'waiting', 'blocked', 'parked', 'done'];
const patchKeys = ['status', 'state', 'documents', 'issues', 'prs'];
const cardKeys = ['title', 'cwd', ...patchKeys];
const controls = /[\p{Cc}\p{Cf}]/u;

function object(value: unknown, keys: string[], label: string): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value) ||
      ![Object.prototype, null].includes(Object.getPrototypeOf(value))) {
    throw new Error(`${label} must be an object`);
  }
  for (const key of Object.keys(value)) {
    if (!keys.includes(key)) throw new Error(`${label}: unknown field ${key}`);
  }
  return value as Record<string, unknown>;
}

function text(value: unknown, label: string, nonempty = false): string {
  if (typeof value !== 'string' || controls.test(value) || (nonempty && !value.trim())) {
    throw new Error(`${label} must be ${nonempty ? 'a non-empty' : 'a'} single-line string without controls`);
  }
  return value;
}

function documentPath(value: unknown): string {
  const path = text(value, 'document path', true);
  if (isAbsolute(path) || path.includes('\\') || path.split('/').some(p => !p || p === '.' || p === '..')) {
    throw new Error('document path must be repo-relative without traversal or backslashes');
  }
  return path;
}

function cwdPath(value: unknown): string {
  const cwd = text(value, 'cwd', true);
  if (!(isAbsolute(cwd) || cwd.startsWith('~/'))) throw new Error('cwd must be absolute or start with ~/');
  return cwd;
}

function linkUrl(value: unknown): string {
  const url = text(value, 'url', true);
  try {
    if (/\s|\\/u.test(url) || controls.test(decodeURIComponent(url)) || !/^https?:\/\//i.test(url)) throw new Error();
    const parsed = new URL(url);
    if (!parsed.hostname || !['https:', 'http:'].includes(parsed.protocol)) throw new Error();
  } catch {
    throw new Error('url must be an absolute HTTP(S) URL without whitespace, backslashes or encoded controls');
  }
  return url;
}

function entries<T>(value: unknown, label: string, validate: (entry: unknown) => T): T[] {
  if (!Array.isArray(value)) throw new Error(`${label} must be an array`);
  return Array.from(value, validate);
}

function patchField(key: string, value: unknown): unknown {
  switch (key) {
    case 'status':
      if (typeof value !== 'string' || !statuses.includes(value)) throw new Error(`status must be one of ${statuses.join(', ')}`);
      return value;
    case 'state': return text(value, 'state');
    case 'documents': return entries(value, 'documents', item => {
      const doc = object(item, ['path', 'description'], 'document');
      return { path: documentPath(doc.path), description: text(doc.description, 'document description') };
    });
    case 'issues':
    case 'prs': return entries(value, key, item => {
      const entry = object(item, ['url', 'description'], key);
      return { url: linkUrl(entry.url), description: text(entry.description, `${key} description`) };
    });
    default: throw new Error(`Unknown patch field ${key}`);
  }
}

export function validateCardPatch(value: unknown): CardPatch {
  const input = object(value, patchKeys, 'card patch');
  return Object.fromEntries(Object.keys(input).map(key => [key, patchField(key, input[key])])) as CardPatch;
}

function validateCard(value: unknown): Card {
  const input = object(value, cardKeys, 'card');
  for (const key of cardKeys) {
    if (!Object.hasOwn(input, key)) throw new Error(`card: missing field ${key}`);
  }
  return {
    title: text(input.title, 'title', true), cwd: cwdPath(input.cwd),
    ...Object.fromEntries(patchKeys.map(key => [key, patchField(key, input[key])])),
  } as Card;
}

export function cardDirectory(env: NodeJS.ProcessEnv = process.env): string {
  return join(env.XDG_STATE_HOME || join(env.HOME || homedir(), '.local', 'state'), 'herdr', 'cards');
}

export function slugify(title: string): string {
  const slug = text(title, 'title', true).normalize('NFKD').replace(/\p{M}/gu, '').toLowerCase()
    .replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '').slice(0, 120).replace(/-$/g, '');
  assertSlug(slug);
  return slug;
}

function assertSlug(slug: string): void {
  if (typeof slug !== 'string' || slug.length > 120 || !/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(slug)) {
    throw new Error('Invalid card slug: use lowercase ASCII words joined by single hyphens (max 120 chars), never paths');
  }
}

function cardPath(slug: string, directory: string): string {
  assertSlug(slug);
  return resolve(directory, `${slug}.yaml`);
}

function snapshot(slug: string, path: string, card: Card, bytes: Buffer): CardSnapshot {
  return { slug, path, card, revision: createHash('sha256').update(bytes).digest('hex') };
}

function errorCode(error: unknown): string | undefined {
  return (error as NodeJS.ErrnoException)?.code;
}

export async function readCard(slug: string, directory = cardDirectory()): Promise<CardSnapshot> {
  const path = cardPath(slug, directory);
  let file;
  try {
    file = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  } catch (error) {
    if (errorCode(error) === 'ELOOP') throw new Error(`Card ${path} must be a regular file, not a symlink`);
    throw error;
  }
  try {
    if (!(await file.stat()).isFile()) throw new Error(`Card ${path} must be a regular file`);
    const bytes = await file.readFile();
    try {
      const yaml = parseDocument(new TextDecoder('utf-8', { fatal: true }).decode(bytes), { uniqueKeys: true });
      if (yaml.errors.length) throw new Error(yaml.errors.map(error => error.message).join('; '));
      return snapshot(slug, path, validateCard(yaml.toJS({ maxAliasCount: 0 })), bytes);
    } catch (error) {
      throw new Error(`Invalid card ${path}: ${(error as Error).message}`, { cause: error });
    }
  } finally {
    await file.close();
  }
}

export async function listCards(directory = cardDirectory()): Promise<CardSnapshot[]> {
  let names: string[];
  try { names = await readdir(directory); }
  catch (error) { if (errorCode(error) === 'ENOENT') return []; throw error; }
  return Promise.all(names.filter(name => name.endsWith('.yaml')).sort().map(name => readCard(name.slice(0, -5), directory)));
}

async function removeTemp(path: string): Promise<void> {
  try { await unlink(path); }
  catch (error) { if (errorCode(error) !== 'ENOENT') throw error; }
}

async function writeTemp(path: string, bytes: Buffer): Promise<string> {
  const temporary = `${path}.${randomUUID()}.tmp`;
  const file = await open(temporary, 'wx', 0o600);
  try {
    await file.writeFile(bytes);
    await file.sync();
  } catch (error) {
    await removeTemp(temporary);
    throw error;
  } finally {
    await file.close();
  }
  return temporary;
}

export async function createCard(card: Card, directory = cardDirectory()): Promise<CardSnapshot> {
  const validated = validateCard(card);
  const slug = slugify(validated.title);
  const path = cardPath(slug, directory);
  await mkdir(directory, { recursive: true, mode: 0o700 });
  const bytes = Buffer.from(stringify(validated));
  const temporary = await writeTemp(path, bytes);
  try {
    try { await link(temporary, path); }
    catch (error) {
      if (errorCode(error) === 'EEXIST') throw new Error(`Card already exists at ${path}; choose a different initial title (slug collision)`);
      throw error;
    }
    return snapshot(slug, path, validated, bytes);
  } finally { await removeTemp(temporary); }
}

export async function updateCard(slug: string, patch: CardPatch, expectedRevision: string, directory = cardDirectory(), signal?: AbortSignal): Promise<CardSnapshot> {
  signal?.throwIfAborted();
  const path = cardPath(slug, directory);
  const validated = validateCardPatch(patch);
  const lockPath = `${path}.lock`;
  let lock;
  try { lock = await open(lockPath, 'wx', 0o600); }
  catch (error) {
    if (errorCode(error) === 'EEXIST') {
      throw new Error(`Card lock exists at ${lockPath}. Inspect the owner/process before manually removing it; locks are never stolen. Reread the card before retrying.`);
    }
    throw error;
  }
  try {
    await lock.writeFile(`pid=${process.pid}\n`);
    const current = await readCard(slug, directory);
    signal?.throwIfAborted();
    if (current.revision !== expectedRevision) throw new Error(`Stale revision conflict for ${path}; reread the card and reconsider the patch`);
    const next = validateCard({ ...current.card, ...validated });
    if (isDeepStrictEqual(next, current.card)) return current;
    const bytes = Buffer.from(stringify(next));
    const temporary = await writeTemp(path, bytes);
    try {
      // Cancellation must be checked at the commit boundary, with no intervening await.
      signal?.throwIfAborted();
      await rename(temporary, path);
    } finally { await removeTemp(temporary); }
    return snapshot(slug, path, next, bytes);
  } finally {
    try { await lock.close(); }
    finally { await unlink(lockPath); }
  }
}

export async function resolveDocument(card: Card, path: string): Promise<string> {
  const cwd = cwdPath(card.cwd);
  const root = await realpath(cwd.startsWith('~/') ? join(homedir(), cwd.slice(2)) : cwd);
  const target = await realpath(resolve(root, documentPath(path)));
  const local = relative(root, target);
  if (local === '..' || local.startsWith(`..${sep}`) || isAbsolute(local)) {
    throw new Error(`Document ${path} resolves outside the checkout ${root}`);
  }
  if (!(await stat(target)).isFile()) throw new Error(`Document ${path} must be a regular file`);
  return target;
}
