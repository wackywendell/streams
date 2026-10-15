import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const modulePath = fileURLToPath(new URL('../dist/herdr/pane.js', import.meta.url));
test('pane import does not connect to Herdr or open a terminal', () => {
  const child = spawnSync(process.execPath, ['--input-type=module', '-e', `await import(${JSON.stringify(new URL('../dist/herdr/pane.js', import.meta.url).href)}); console.log('imported')`], { encoding: 'utf8', env: { ...process.env, WORK_CARDS_CARD: '' } });
  assert.equal(child.status, 0); assert.equal(child.stdout.trim(), 'imported'); assert.equal(child.stderr, '');
});
test('standalone pane rejects a noninteractive launch with an actionable error', () => {
  const child = spawnSync(process.execPath, [modulePath], { encoding: 'utf8' });
  assert.notEqual(child.status, 0); assert.match(child.stderr, /interactive terminal/i);
});
