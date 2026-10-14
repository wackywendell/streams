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
