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
  first.workspace_naming = false;
  assert.equal((await loadConfig(path)).workspace_naming, true);
  assert.equal(DEFAULT_CONFIG.workspace_naming, true);
});

test('partial config overlays defaults without changing unrelated settings', async t => {
  const path = await fixture(t);
  const overrides = { pane_height_fraction: 0.35, herdr_timeout_ms: 250, document_command: 'less -R' };
  await writeFile(path, JSON.stringify(overrides));
  assert.deepEqual(await loadConfig(path), { ...DEFAULT_CONFIG, ...overrides });
});

test('workspace startup accepts only automatic or manual', async t => {
  const path = await fixture(t);
  assert.equal(DEFAULT_CONFIG.workspace_startup, 'automatic');
  for (const workspace_startup of ['automatic', 'manual']) {
    await writeFile(path, JSON.stringify({ workspace_startup }));
    assert.equal((await loadConfig(path)).workspace_startup, workspace_startup);
  }
  for (const workspace_startup of [null, true, 'legacy', 'Automatic', 0, [], {}]) {
    await writeFile(path, JSON.stringify({ workspace_startup }));
    await assert.rejects(loadConfig(path), /workspace_startup/);
  }
});

test('naming defaults to true independently of startup and other naming settings', async t => {
  const path = await fixture(t);
  const keys = ['workspace_naming', 'tab_naming', 'session_naming'];
  for (const workspace_startup of ['automatic', 'manual']) {
    await writeFile(path, JSON.stringify({ workspace_startup }));
    const defaults = await loadConfig(path);
    for (const key of keys) {
      assert.equal(DEFAULT_CONFIG[key], true);
      assert.equal(defaults[key], true);
      for (const override of [true, false]) {
        await writeFile(path, JSON.stringify({ workspace_startup, [key]: override }));
        const config = await loadConfig(path);
        assert.equal(config[key], override);
        for (const other of keys.filter(other => other !== key)) assert.equal(config[other], true);
      }
    }
  }
});

test('naming settings reject null and all non-boolean values', async t => {
  const path = await fixture(t);
  for (const key of ['workspace_naming', 'tab_naming', 'session_naming']) {
    for (const value of [null, 'true', 'false', 0, 1, [], {}]) {
      await writeFile(path, JSON.stringify({ [key]: value }));
      await assert.rejects(loadConfig(path), new RegExp(key));
    }
  }
});

test('removed inference and legacy options are rejected rather than silently ignored', async t => {
  const path = await fixture(t);
  for (const key of ['model', 'thinking_level', 'show_refresh_spinner', 'check_every_n_responses',
    'max_output_tokens', 'request_timeout_ms', 'check_every_n_cycles', 'max_context_chars',
    'max_document_bytes', 'update_guidance', 'pane_poll_interval_ms']) {
    await writeFile(path, JSON.stringify({ [key]: 1 }));
    await assert.rejects(loadConfig(path), new RegExp(`unknown field ${key}`));
  }
});

test('Herdr timeout accepts positive integer boundaries and rejects invalid or overflowing delays', async t => {
  const path = await fixture(t);
  for (const value of [1, 2147483647]) {
    await writeFile(path, JSON.stringify({ herdr_timeout_ms: value }));
    assert.equal((await loadConfig(path)).herdr_timeout_ms, value);
  }
  for (const value of [0, -1, 0.5, '100', null, false, [], {}, 2147483648, 1e100]) {
    await writeFile(path, JSON.stringify({ herdr_timeout_ms: value }));
    await assert.rejects(loadConfig(path), /integer between 1 and 2147483647/);
  }
});

test('pane height fraction must be a number strictly between zero and one', async t => {
  const path = await fixture(t);
  for (const value of [0.01, 0.5, 0.99]) {
    await writeFile(path, JSON.stringify({ pane_height_fraction: value }));
    assert.equal((await loadConfig(path)).pane_height_fraction, value);
  }
  for (const value of [0, 1, -0.1, 1.1, '0.2', null, false, [], {}]) {
    await writeFile(path, JSON.stringify({ pane_height_fraction: value }));
    await assert.rejects(loadConfig(path), /pane_height_fraction/);
  }
});

test('malformed config errors include path and never silently use defaults', async t => {
  const path = await fixture(t);
  for (const raw of ['{broken', 'null', '[]', '42', 'true', '"text"', '{"retry":1}', '{"workspace_naming":null}']) {
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
