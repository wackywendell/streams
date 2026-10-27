import { test } from 'node:test';
import assert from 'node:assert/strict';
import { DEFAULT_CONFIG, parseConfig } from '../dist/config.js';

test('empty config uses frozen defaults without shared mutable results', () => {
  assert.equal(Object.isFrozen(DEFAULT_CONFIG), true);
  const first = parseConfig({});
  assert.deepEqual(first, DEFAULT_CONFIG);
  first.workspace_naming = false;
  assert.equal(parseConfig({}).workspace_naming, true);
  assert.equal(DEFAULT_CONFIG.workspace_naming, true);
});

test('partial config overlays defaults without changing unrelated settings', () => {
  const overrides = { pane_height_fraction: 0.35, herdr_timeout_ms: 250, document_command: 'less -R' };
  assert.deepEqual(parseConfig(overrides), { ...DEFAULT_CONFIG, ...overrides });
});

test('workspace startup accepts only automatic or manual', () => {
  assert.equal(DEFAULT_CONFIG.workspace_startup, 'automatic');
  for (const workspace_startup of ['automatic', 'manual']) {
    assert.equal(parseConfig({ workspace_startup }).workspace_startup, workspace_startup);
  }
  for (const workspace_startup of [null, true, 'legacy', 'Automatic', 0, [], {}]) {
    assert.throws(() => parseConfig({ workspace_startup }), /workspace_startup/);
  }
});

test('naming defaults to true independently of startup and other naming settings', () => {
  const keys = ['workspace_naming', 'tab_naming', 'session_naming'];
  for (const workspace_startup of ['automatic', 'manual']) {
    const defaults = parseConfig({ workspace_startup });
    for (const key of keys) {
      assert.equal(DEFAULT_CONFIG[key], true);
      assert.equal(defaults[key], true);
      for (const override of [true, false]) {
        const config = parseConfig({ workspace_startup, [key]: override });
        assert.equal(config[key], override);
        for (const other of keys.filter(other => other !== key)) assert.equal(config[other], true);
      }
    }
  }
});

test('naming settings reject null and all non-boolean values', () => {
  for (const key of ['workspace_naming', 'tab_naming', 'session_naming']) {
    for (const value of [null, 'true', 'false', 0, 1, [], {}]) {
      assert.throws(() => parseConfig({ [key]: value }), new RegExp(key));
    }
  }
});

test('removed inference and legacy options are rejected rather than silently ignored', () => {
  for (const key of ['model', 'thinking_level', 'show_refresh_spinner', 'check_every_n_responses',
    'max_output_tokens', 'request_timeout_ms', 'check_every_n_cycles', 'max_context_chars',
    'max_document_bytes', 'update_guidance', 'pane_poll_interval_ms']) {
    assert.throws(() => parseConfig({ [key]: 1 }), new RegExp(`unknown field ${key}`));
  }
});

test('Herdr timeout accepts positive integer boundaries and rejects invalid or overflowing delays', () => {
  for (const value of [1, 2147483647]) {
    assert.equal(parseConfig({ herdr_timeout_ms: value }).herdr_timeout_ms, value);
  }
  for (const value of [0, -1, 0.5, '100', null, false, [], {}, 2147483648, 1e100]) {
    assert.throws(() => parseConfig({ herdr_timeout_ms: value }), /integer between 1 and 2147483647/);
  }
});

test('pane height fraction must be a number strictly between zero and one', () => {
  for (const value of [0.01, 0.5, 0.99]) {
    assert.equal(parseConfig({ pane_height_fraction: value }).pane_height_fraction, value);
  }
  for (const value of [0, 1, -0.1, 1.1, '0.2', null, false, [], {}, NaN, Infinity]) {
    assert.throws(() => parseConfig({ pane_height_fraction: value }), /pane_height_fraction/);
  }
});

test('non-object config is rejected rather than silently using defaults', () => {
  for (const input of [null, [], 42, true, 'text']) {
    assert.throws(() => parseConfig(input), /expected a JSON object/);
  }
});

test('document command accepts null or a nonblank command without controls', () => {
  assert.equal(DEFAULT_CONFIG.document_command, null);
  for (const document_command of [null, 'vim', 'less -R', '"/path with spaces/editor" -f']) {
    assert.equal(parseConfig({ document_command }).document_command, document_command);
  }
  for (const document_command of ['', '   ', 42, false, [], {}, 'vim\n', 'vim\u202e']) {
    assert.throws(() => parseConfig({ document_command }), /document_command/);
  }
});
