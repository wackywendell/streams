import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, writeFile, rm } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { homedir } from 'node:os';
import { DEFAULT_CONFIG, configPath, loadConfig, documentCommand } from '../dist/config.js';

async function fixture(t) {
  await mkdir(resolve('.test-tmp'), { recursive: true });
  const dir = await mkdtemp(join(resolve('.test-tmp'), 'config-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  return join(dir, 'work-cards.json');
}

test('config location is global and respects PI_CODING_AGENT_DIR', () => {
  assert.equal(configPath({ PI_CODING_AGENT_DIR: '/custom', HOME: '/home/me' }), '/custom/work-cards.json');
  assert.equal(configPath({ HOME: '/home/me' }), '/home/me/.pi/agent/work-cards.json');
  assert.equal(configPath({}), join(homedir(), '.pi/agent/work-cards.json'));
});

test('missing config uses frozen defaults without shared mutable results', async t => {
  const path = await fixture(t);
  assert.equal(Object.isFrozen(DEFAULT_CONFIG), true);
  const first = await loadConfig(path);
  assert.deepEqual(first, DEFAULT_CONFIG);
  first.model = 'provider/changed';
  assert.equal((await loadConfig(path)).model, 'helpers/simple');
  assert.equal(DEFAULT_CONFIG.thinking_level, null);
  assert.equal(DEFAULT_CONFIG.max_document_bytes, 262144);
  assert.equal(DEFAULT_CONFIG.max_context_chars, 24000);
  assert.equal(DEFAULT_CONFIG.max_output_tokens, 2048);
  assert.equal(DEFAULT_CONFIG.request_timeout_ms, 30000);
});

test('default guidance matches the no_change summarizer decision', () => {
  assert.ok(DEFAULT_CONFIG.update_guidance.includes('return no_change for routine activity.'));
  assert.ok(DEFAULT_CONFIG.update_guidance.includes('even without new progress.'));
  assert.ok(!DEFAULT_CONFIG.update_guidance.includes('empty patch'));
});

test('partial config overlays defaults and zero disables automatic input checks', async t => {
  const path = await fixture(t);
  await writeFile(path, JSON.stringify({ check_every_n_responses: 0, model: 'provider/model', pane_height_fraction: 0.35 }));
  assert.deepEqual(await loadConfig(path), { ...DEFAULT_CONFIG, check_every_n_responses: 0, model: 'provider/model', pane_height_fraction: 0.35 });
  await writeFile(path, JSON.stringify({ check_every_n_cycles: 0 }));
  await assert.rejects(loadConfig(path), /unknown field check_every_n_cycles/);
});

test('deprecated context budget remains accepted and validated for strict config compatibility', async t => {
  const path = await fixture(t);
  for (const max_context_chars of [1, 24000, 1000000]) {
    await writeFile(path, JSON.stringify({ max_context_chars }));
    assert.equal((await loadConfig(path)).max_context_chars, max_context_chars);
  }
  await writeFile(path, JSON.stringify({ max_context_chars: 0 }));
  await assert.rejects(loadConfig(path), /max_context_chars.*integer >= 1/);
});

test('thinking level accepts explicit overrides and null for profile inheritance', async t => {
  const path = await fixture(t);
  for (const thinking_level of [null, 'off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max']) {
    await writeFile(path, JSON.stringify({ thinking_level }));
    assert.equal((await loadConfig(path)).thinking_level, thinking_level);
  }
});

test('timer settings reject platform-overflow values and accept the maximum delay', async t => {
  const path = await fixture(t);
  for (const key of ['pane_poll_interval_ms', 'request_timeout_ms', 'herdr_timeout_ms']) {
    await writeFile(path, JSON.stringify({ [key]: 2147483647 }));
    assert.equal((await loadConfig(path))[key], 2147483647);
    await writeFile(path, JSON.stringify({ [key]: 2147483648 }));
    await assert.rejects(loadConfig(path), /2147483647/);
  }
});

test('malformed config errors include path and never silently use defaults', async t => {
  const path = await fixture(t);
  const invalid = ['{broken', 'null', '[]', '42', JSON.stringify({ retry: 1 }), JSON.stringify({ model: '' }), JSON.stringify({ model: 'no-provider' }), JSON.stringify({ model: 'provider/\nmodel' }), JSON.stringify({ update_guidance: '' })];
  for (const field of ['check_every_n_responses', 'check_every_n_cycles']) {
    for (const value of [-1, 0.5, '1', null]) invalid.push(JSON.stringify({ [field]: value }));
  }
  for (const field of ['max_context_chars', 'max_document_bytes', 'max_output_tokens', 'request_timeout_ms', 'pane_poll_interval_ms', 'herdr_timeout_ms']) {
    for (const value of [0, -1, 0.5, '100']) invalid.push(JSON.stringify({ [field]: value }));
  }
  for (const value of [0, 1, -0.1, '0.2']) invalid.push(JSON.stringify({ pane_height_fraction: value }));
  for (const value of ['unknown', 'HIGH', '', 0, false, [], {}]) invalid.push(JSON.stringify({ thinking_level: value }));
  for (const raw of invalid) {
    await writeFile(path, raw);
    await assert.rejects(loadConfig(path), error => error.message.includes(path));
  }
});

test('document command is global, validated and resolves config > EDITOR > less', async t => {
  const path = await fixture(t);
  assert.equal(DEFAULT_CONFIG.document_command, null);
  for (const command of [null, 'vim', 'less -R', '"/path with spaces/editor" -f']) {
    await writeFile(path, JSON.stringify({ document_command: command }));
    assert.equal((await loadConfig(path)).document_command, command);
  }
  for (const command of ['', '   ', 42, false, [], {}, 'vim\n', 'vim\u202e']) {
    await writeFile(path, JSON.stringify({ document_command: command }));
    await assert.rejects(loadConfig(path), /document_command/);
  }
  assert.equal(documentCommand(DEFAULT_CONFIG, {}), 'less');
  assert.equal(documentCommand(DEFAULT_CONFIG, { EDITOR: '   ', VISUAL: 'vim' }), 'less');
  assert.equal(documentCommand(DEFAULT_CONFIG, { EDITOR: 'nano -w' }), 'nano -w');
  assert.equal(documentCommand({ ...DEFAULT_CONFIG, document_command: 'less -R' }, { EDITOR: 'bad\n' }), 'less -R');
  assert.throws(() => documentCommand(DEFAULT_CONFIG, { EDITOR: 'vim\n' }), /EDITOR/);
});
