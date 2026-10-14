import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createCard } from '../dist/card.js';

const modulePath = fileURLToPath(new URL('../dist/herdr/pane.js', import.meta.url));
test('pane import does not connect to Herdr or open a terminal', () => {
  const child = spawnSync(process.execPath, ['--input-type=module', '-e', `await import(${JSON.stringify(new URL('../dist/herdr/pane.js', import.meta.url).href)}); console.log('imported')`], { encoding: 'utf8', env: { ...process.env, WORK_CARDS_CARD: '' } });
  assert.equal(child.status, 0); assert.equal(child.stdout.trim(), 'imported'); assert.equal(child.stderr, '');
});
test('standalone pane rejects a noninteractive launch with an actionable error', () => {
  const child = spawnSync(process.execPath, [modulePath], { encoding: 'utf8' });
  assert.notEqual(child.status, 0); assert.match(child.stderr, /interactive terminal/i);
});
test('publication clears only projection notices independently of message wording', async t => {
  const root = await mkdtemp(join(tmpdir(), 'work-cards-runtime-')); t.after(() => rm(root, { recursive: true, force: true }));
  const saved = await createCard({ title: 'Title', status: 'active', state: '', cwd: root, documents: [], issues: [], prs: [] }, root);
  const script = `
    import { PassThrough, Writable } from 'node:stream';
    import { runPane } from ${JSON.stringify(new URL('../dist/herdr/pane.js', import.meta.url).href)};
    import { HerdrClient } from ${JSON.stringify(new URL('../dist/herdr/client.js', import.meta.url).href)};
    import { CardView } from ${JSON.stringify(new URL('../dist/herdr/view.js', import.meta.url).href)};
    const originalOutput = process.stdout;
    const input = new PassThrough(); input.isTTY = true; input.isRaw = false;
    input.setRawMode = value => { input.isRaw = value; };
    const output = new Writable({ write(chunk, encoding, done) { done(); } });
    output.isTTY = true; output.columns = 80; output.rows = 10;
    Object.defineProperty(process, 'stdin', { value: input });
    Object.defineProperty(process, 'stdout', { value: output });
    let view, publications = 0; const notices = [];
    HerdrClient.prototype.paneAttachment = async () => ({});
    HerdrClient.prototype.projectCard = async () => {
      publications++;
      if (publications === 1) throw new Error('Publishing failed');
      if (publications === 4) view.setNotice('card', 'Herdr projection unavailable: card error');
    };
    const render = CardView.prototype.render;
    CardView.prototype.render = function(columns, rows) {
      view = this; notices.push(this.notice ?? null);
      const text = render.call(this, columns, rows);
      if (publications === 1) this.setNotice('projection', 'Arbitrary publishing failure wording');
      if (publications === 2) this.setNotice('document', 'Herdr projection unavailable: document error');
      setImmediate(() => publications === 4 ? process.emit('SIGTERM') : output.emit('resize'));
      return text;
    };
    await runPane({ WORK_CARDS_CARD: ${JSON.stringify(saved.path)}, WORK_CARDS_CONFIG: ${JSON.stringify(join(root, 'missing.json'))} });
    originalOutput.write(JSON.stringify({ publications, notices }));
  `;
  const child = spawnSync(process.execPath, ['--input-type=module', '-e', script], { encoding: 'utf8', timeout: 5000 });
  assert.equal(child.status, 0, child.stderr); const result = JSON.parse(child.stdout);
  assert.equal(result.publications, 4);
  assert.deepEqual(result.notices, [
    { source: 'projection', text: 'Herdr projection unavailable: Publishing failed' }, null,
    { source: 'document', text: 'Herdr projection unavailable: document error' },
    { source: 'card', text: 'Herdr projection unavailable: card error' },
  ]);
});
test('SIGTERM restores raw mode, cursor, wrapping and alternate screen without a real pane', async t => {
  const root = await mkdtemp(join(tmpdir(), 'work-cards-runtime-')); t.after(() => rm(root, { recursive: true, force: true }));
  const saved = await createCard({ title: 'Title', status: 'active', state: '', cwd: root, documents: [], issues: [], prs: [] }, root);
  const script = `
    import { PassThrough, Writable } from 'node:stream';
    import { runPane } from ${JSON.stringify(new URL('../dist/herdr/pane.js', import.meta.url).href)};
    const originalOutput = process.stdout;
    const input = new PassThrough(); input.isTTY = true; input.isRaw = false;
    const raw = []; input.setRawMode = value => { raw.push(value); input.isRaw = value; };
    let painted = ''; let sent = false;
    const output = new Writable({ write(chunk, encoding, done) {
      painted += chunk.toString();
      if (!sent && painted.includes('Title')) { sent = true; setImmediate(() => process.emit('SIGTERM')); }
      done();
    } });
    output.isTTY = true; output.columns = 80; output.rows = 10;
    Object.defineProperty(process, 'stdin', { value: input });
    Object.defineProperty(process, 'stdout', { value: output });
    await runPane({ WORK_CARDS_CARD: ${JSON.stringify(saved.path)}, WORK_CARDS_CONFIG: ${JSON.stringify(join(root, 'missing.json'))} });
    originalOutput.write(JSON.stringify({ raw, painted }));
  `;
  const child = spawnSync(process.execPath, ['--input-type=module', '-e', script], { encoding: 'utf8', timeout: 5000 });
  assert.equal(child.status, 0, child.stderr); const result = JSON.parse(child.stdout);
  assert.deepEqual(result.raw, [true, false]); assert.match(result.painted, /Title/);
  assert.ok(result.painted.endsWith('\x1b[?2004l\x1b[?7h\x1b[?25h\x1b[?1049l'));
});

test('document focus discards buffered autorepeat and q while keeping card publication alive; failures are document notices', async t => {
  for (const fails of [false, true]) await t.test(fails ? 'failure' : 'success', async sub => {
    const root = await mkdtemp(join(tmpdir(), 'work-cards-document-input-'));
    sub.after(() => rm(root, { recursive: true, force: true }));
    const saved = await createCard({ title: 'Title', status: 'active', state: '', cwd: root,
      documents: [{ path: 'notes.md', description: '' }], issues: [], prs: [] }, root);
    const script = `
      import { PassThrough, Writable } from 'node:stream';
      import { runPane } from ${JSON.stringify(new URL('../dist/herdr/pane.js', import.meta.url).href)};
      import { HerdrClient } from ${JSON.stringify(new URL('../dist/herdr/client.js', import.meta.url).href)};
      import { CardView } from ${JSON.stringify(new URL('../dist/herdr/view.js', import.meta.url).href)};
      const originalOutput = process.stdout;
      const input = new PassThrough(); input.isTTY = true; input.isRaw = false;
      input.setRawMode = value => { input.isRaw = value; };
      const output = new Writable({ write(chunk, encoding, done) { done(); } });
      output.isTTY = true; output.columns = 80; output.rows = 10;
      Object.defineProperty(process, 'stdin', { value: input });
      Object.defineProperty(process, 'stdout', { value: output });
      let publications = 0, opens = 0; const notices = [];
      HerdrClient.prototype.paneAttachment = async () => ({});
      HerdrClient.prototype.projectCard = async () => { publications++; };
      HerdrClient.prototype.openDocument = async (snapshot, path) => {
        if (path !== 'notes.md' || snapshot.path !== ${JSON.stringify(saved.path)}) throw new Error('wrong intent');
        opens++;
        if (${fails}) throw new Error('Document tab unavailable (known tab w1:t3)');
        input.emit('data', Buffer.from('aaaaaq')); // Buffered while focus was in flight.
      };
      const render = CardView.prototype.render;
      CardView.prototype.render = function(columns, rows) {
        notices.push(this.notice ?? null);
        const text = render.call(this, columns, rows);
        setImmediate(() => {
          if (publications === 1) input.emit('data', Buffer.from(${JSON.stringify('aaaaaq')}));
          else if (publications >= 3) process.emit('SIGTERM');
          else output.emit('resize');
        });
        return text;
      };
      await runPane({ WORK_CARDS_CARD: ${JSON.stringify(saved.path)}, WORK_CARDS_CONFIG: ${JSON.stringify(join(root, 'missing.json'))} });
      originalOutput.write(JSON.stringify({ publications, opens, notices }));
    `;
    const child = spawnSync(process.execPath, ['--input-type=module', '-e', script], { encoding: 'utf8', timeout: 5000 });
    assert.equal(child.status, 0, child.stderr);
    const result = JSON.parse(child.stdout);
    assert.equal(result.opens, 1); assert.ok(result.publications >= 3);
    if (fails) {
      assert.equal(result.notices.at(-1).source, 'document');
      assert.match(result.notices.at(-1).text, /Cannot open notes.md.*w1:t3/);
    } else assert.ok(result.notices.every(notice => notice === null));
  });
});
