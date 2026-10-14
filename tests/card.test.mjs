import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, open, readFile, writeFile, stat, rm, symlink, readdir } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { createHash } from 'node:crypto';
import { spawn, spawnSync } from 'node:child_process';
import { homedir } from 'node:os';
import { stringify } from 'yaml';
import { cardDirectory, slugify, createCard, readCard, listCards, updateCard, resolveDocument, validateCardPatch } from '../dist/card.js';

async function fixture(t) {
  const base = resolve('.test-tmp');
  await mkdir(base, { recursive: true });
  const dir = await mkdtemp(join(base, 'card-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  return dir;
}
function card(cwd, overrides = {}) {
  return { title: 'New project', status: 'active', state: 'Implementing shared core', cwd, documents: [], issues: [], prs: [], ...overrides };
}

test('validateCardPatch validates unknown helper output without accepting identity changes', () => {
  assert.deepEqual(validateCardPatch({ status: 'waiting', state: 'Review needed' }), { status: 'waiting', state: 'Review needed' });
  assert.deepEqual(validateCardPatch({}), {});
  for (const value of [null, [], { title: 'No' }, { cwd: '/no' }, { state: undefined }, { status: 'bad' }, { documents: [{ path: '../escape', description: '' }] }, { documents: Array(1) }]) {
    assert.throws(() => validateCardPatch(value));
  }
});

test('state directory and slug generation are deterministic', () => {
  assert.equal(cardDirectory({ XDG_STATE_HOME: '/state', HOME: '/home/me' }), '/state/herdr/cards');
  assert.equal(cardDirectory({ HOME: '/home/me' }), '/home/me/.local/state/herdr/cards');
  assert.equal(cardDirectory({}), join(homedir(), '.local/state/herdr/cards'));
  assert.equal(slugify('  Café / New Project! '), 'cafe-new-project');
  assert.throws(() => slugify('!!!'), /slug/i);
});

test('create/read/list return byte-hashed snapshots and never overwrite a collision', async t => {
  const dir = await fixture(t);
  const input = card(dir);
  const first = await createCard(input, dir);
  assert.equal(first.slug, 'new-project');
  assert.equal(first.path, join(dir, 'new-project.yaml'));
  assert.deepEqual(first.card, input);
  assert.equal(first.revision, createHash('sha256').update(await readFile(first.path)).digest('hex'));
  await assert.rejects(createCard({ ...input, state: 'Overwrite' }, dir), /exist|collision/i);
  assert.deepEqual(await readCard(first.slug, dir), first);
  await createCard(card(dir, { title: 'A project' }), dir);
  assert.deepEqual((await listCards(dir)).map(c => c.slug), ['a-project', 'new-project']);
  assert.deepEqual(await listCards(join(dir, 'absent')), []);
  assert.deepEqual((await readdir(dir)).sort(), ['a-project.yaml', 'new-project.yaml']);
});

test('strict schema rejects invalid cards before any card file is created', async t => {
  const dir = await fixture(t);
  const invalid = [
    { extra: 'no' }, { title: '' }, { title: 'bad\nline' }, { status: 'unknown' },
    { state: 12 }, { cwd: 'relative' }, { cwd: '~other/repo' }, { cwd: '/bad\u001bpath' },
    { documents: [{ path: '../outside', description: '' }] },
    { documents: [{ path: '/absolute', description: '' }] },
    { documents: [{ path: 'docs/../file', description: '' }] },
    { documents: [{ path: 'docs\\file', description: '' }] },
    { documents: [{ path: 'docs/file', description: '', extra: true }] },
    { issues: [{ url: 'javascript:alert(1)', description: '' }] },
    { prs: [{ url: 'https://example.com/\nunsafe', description: '' }] },
    { prs: [{ url: 'https://example.com/%0dunsafe', description: '' }] },
    { prs: [{ url: 'https://example.com/white space', description: '' }] },
    { issues: [{ url: 'https://', description: '' }] }, { documents: null },
  ];
  for (const override of invalid) await assert.rejects(createCard(card(dir, override), dir));
  const missing = card(dir); delete missing.state;
  await assert.rejects(createCard(missing, dir));
  assert.deepEqual(await listCards(dir), []);
});

test('YAML duplicate keys, unknown fields and malformed files are visible failures', async t => {
  const dir = await fixture(t);
  const path = join(dir, 'bad.yaml');
  for (const bytes of [stringify(card(dir)) + 'title: Duplicate\n', stringify({ ...card(dir), extra: true }), '[unterminated']) {
    await writeFile(path, bytes);
    await assert.rejects(readCard('bad', dir));
    await assert.rejects(listCards(dir));
  }
});

test('strict slugs reject filesystem paths and card files cannot be symlinks', async t => {
  const dir = await fixture(t);
  for (const slug of ['../evil', '/evil', 'a/b', 'a\\b', 'UPPER', '', 'a--b', 'a'.repeat(121)]) {
    await assert.rejects(readCard(slug, dir), /slug/i);
    await assert.rejects(updateCard(slug, {}, 'revision', dir), /slug/i);
  }
  const target = join(dir, 'target');
  await writeFile(target, stringify(card(dir)));
  await symlink(target, join(dir, 'linked.yaml'));
  await assert.rejects(readCard('linked', dir), /regular|symlink/i);
});

for (const operation of ['readCard', 'listCards']) {
  test(`${operation} promptly rejects a FIFO without a writer`, async t => {
    const dir = await fixture(t);
    const fifo = spawnSync('mkfifo', [join(dir, 'pipe.yaml')], { timeout: 3000, encoding: 'utf8' });
    assert.equal(fifo.status, 0, fifo.error?.message || fifo.stderr);
    const moduleUrl = new URL('../dist/card.js', import.meta.url).href;
    const args = operation === 'readCard' ? ['pipe', dir] : [dir];
    const code = `import assert from 'node:assert/strict';
      import { ${operation} } from ${JSON.stringify(moduleUrl)};
      await assert.rejects(${operation}(...${JSON.stringify(args)}), /regular file/);`;
    // Isolate blocked filesystem work so a regression cannot hang the test runner.
    const child = spawnSync(process.execPath, ['--input-type=module', '-e', code], {
      timeout: 3000, killSignal: 'SIGKILL', encoding: 'utf8',
    });
    assert.equal(child.status, 0, child.error?.message || child.stderr);
  });
}

test('updates preserve identity after manual title editing and enforce optimistic revisions', async t => {
  const dir = await fixture(t);
  const first = await createCard(card(dir), dir);
  await writeFile(first.path, stringify(card(dir, { title: 'Manually renamed' })));
  const latest = await readCard(first.slug, dir);
  await assert.rejects(updateCard(first.slug, { state: 'stale' }, first.revision, dir), /stale|conflict/i);
  const updated = await updateCard(first.slug, { status: 'waiting', state: 'Needs review', issues: [{ url: 'https://example.com/issue/1', description: 'Tracking' }] }, latest.revision, dir);
  assert.equal(updated.slug, first.slug);
  assert.equal(updated.card.title, 'Manually renamed');
  assert.equal(updated.card.status, 'waiting');
  assert.notEqual(updated.revision, latest.revision);
  for (const patch of [{ title: 'automatic rename' }, { cwd: '/other' }, { state: undefined }, { status: 'invalid' }, { body: 'extra' }]) {
    await assert.rejects(updateCard(first.slug, patch, updated.revision, dir));
  }
  assert.deepEqual(await readCard(first.slug, dir), updated);
  assert.deepEqual(await readdir(dir), ['new-project.yaml']);
});

test('semantic no-change preserves YAML bytes, revision and mtime', async t => {
  const dir = await fixture(t);
  const first = await createCard(card(dir), dir);
  await writeFile(first.path, '# user comment\n' + stringify(card(dir)));
  const before = await readCard(first.slug, dir);
  const bytes = await readFile(first.path);
  const metadata = await stat(first.path, { bigint: true });
  const result = await updateCard(first.slug, { state: before.card.state, documents: [] }, before.revision, dir);
  assert.deepEqual(result, before);
  assert.deepEqual(await readFile(first.path), bytes);
  assert.equal((await stat(first.path, { bigint: true })).mtimeNs, metadata.mtimeNs);
});

test('already aborted updates preserve bytes and never acquire or disturb a lock', async t => {
  const dir = await fixture(t);
  const first = await createCard(card(dir), dir);
  const bytes = await readFile(first.path);
  const controller = new AbortController();
  controller.abort();
  await assert.rejects(updateCard(first.slug, { state: 'Cancelled' }, first.revision, dir, controller.signal), { name: 'AbortError' });
  assert.deepEqual(await readFile(first.path), bytes);
  assert.deepEqual(await readdir(dir), ['new-project.yaml']);
  await writeFile(first.path + '.lock', 'owner');
  await assert.rejects(updateCard(first.slug, {}, first.revision, dir, controller.signal), { name: 'AbortError' });
  assert.equal(await readFile(first.path + '.lock', 'utf8'), 'owner');
});

test('abort during temporary file sync preserves bytes and cleans temporary file and lock', async t => {
  const dir = await fixture(t);
  const first = await createCard(card(dir), dir);
  const bytes = await readFile(first.path);
  const handle = await open(first.path, 'r');
  const prototype = Object.getPrototypeOf(handle);
  const sync = prototype.sync;
  await handle.close();
  let entered, release;
  const syncing = new Promise(resolve => { entered = resolve; });
  const suspended = new Promise(resolve => { release = resolve; });
  t.mock.method(prototype, 'sync', async function () {
    entered();
    await suspended;
    return sync.call(this);
  });
  const controller = new AbortController();
  const pending = updateCard(first.slug, { state: 'Cancelled' }, first.revision, dir, controller.signal);
  const rejected = assert.rejects(pending, { name: 'AbortError' });
  try {
    await syncing;
    controller.abort();
  } finally { release(); }
  await rejected;
  assert.deepEqual(await readFile(first.path), bytes);
  assert.deepEqual(await readdir(dir), ['new-project.yaml']);
});

test('held locks fail closed, including no-op updates, without removing the lock', async t => {
  const dir = await fixture(t);
  const first = await createCard(card(dir), dir);
  const lock = first.path + '.lock';
  await writeFile(lock, 'owner');
  await assert.rejects(updateCard(first.slug, {}, first.revision, dir), /lock.*inspect|inspect.*lock/i);
  assert.equal(await readFile(lock, 'utf8'), 'owner');
  assert.deepEqual(await readCard(first.slug, dir), first);
});

test('concurrent creators/updaters cannot overwrite or lose writes', async t => {
  const dir = await fixture(t);
  const creations = await Promise.allSettled(Array.from({ length: 8 }, () => createCard(card(dir), dir)));
  assert.equal(creations.filter(r => r.status === 'fulfilled').length, 1);
  const first = await readCard('new-project', dir);
  const results = await Promise.allSettled(Array.from({ length: 8 }, (_, i) => updateCard(first.slug, { state: `Update ${i}` }, first.revision, dir)));
  const successful = results.filter(r => r.status === 'fulfilled');
  assert.equal(successful.length, 1);
  assert.deepEqual(await readCard(first.slug, dir), successful[0].value);
  assert.deepEqual(await readdir(dir), ['new-project.yaml']);
});

test('separate processes updating one revision produce exactly one winner', async t => {
  const dir = await fixture(t);
  const first = await createCard(card(dir), dir);
  const moduleUrl = new URL('../dist/card.js', import.meta.url).href;
  const workers = Array.from({ length: 4 }, (_, i) => new Promise((resolve, reject) => {
    const code = `import { updateCard } from ${JSON.stringify(moduleUrl)};
      try { await updateCard('new-project', {state: 'Process ${i}'}, ${JSON.stringify(first.revision)}, ${JSON.stringify(dir)}); }
      catch (error) { if (/lock|[Ss]tale/.test(error.message)) process.exitCode = 2; else throw error; }`;
    const child = spawn(process.execPath, ['--input-type=module', '-e', code], { stdio: ['ignore', 'ignore', 'pipe'] });
    let stderr = '';
    child.stderr.on('data', bytes => { stderr += bytes; });
    child.on('error', reject);
    child.on('exit', status => {
      if (status === 0 || status === 2) resolve({ status, i });
      else reject(new Error(stderr || `worker exited ${status}`));
    });
  }));
  const results = await Promise.all(workers);
  const winners = results.filter(result => result.status === 0);
  assert.equal(winners.length, 1);
  assert.equal((await readCard(first.slug, dir)).card.state, `Process ${winners[0].i}`);
  assert.deepEqual(await readdir(dir), ['new-project.yaml']);
});

test('document resolution stays inside the checkout, including symlinks', async t => {
  const dir = await fixture(t);
  const repo = join(dir, 'repo');
  await mkdir(join(repo, 'docs'), { recursive: true });
  await writeFile(join(repo, 'docs/design.md'), 'design');
  await writeFile(join(dir, 'outside'), 'outside');
  await symlink(join(dir, 'outside'), join(repo, 'escape'));
  await symlink(join(repo, 'docs/design.md'), join(repo, 'inside'));
  const input = card(repo);
  assert.equal(await resolveDocument(input, 'inside'), await resolveDocument(input, 'docs/design.md'));
  await assert.rejects(resolveDocument(input, 'escape'), /outside|checkout/i);
  await assert.rejects(resolveDocument(input, '../outside'), /path/i);
  await assert.rejects(resolveDocument(input, 'docs'), /file/i);
  await assert.rejects(resolveDocument(input, 'missing'));
  const relativeHome = resolve(repo).slice(homedir().length + 1);
  assert.equal(await resolveDocument(card(`~/${relativeHome}`), 'inside'), await resolveDocument(input, 'inside'));
});
