import assert from 'node:assert/strict';
import test from 'node:test';
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { createJiti } from 'jiti';

const jiti = createJiti(import.meta.url);
const source = readFileSync(new URL('../src/streams-bin.ts', import.meta.url), 'utf8');
async function fixture(t) {
  const root = mkdtempSync(join(realpathSync(tmpdir()), 'streams-selector-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const module = join(root, 'integrations/herdr-work-cards/src/streams-bin.ts');
  mkdirSync(dirname(module), { recursive: true });
  writeFileSync(module, source);
  const { resolveStreamsBin } = await jiti.import(module);
  const executable = path => {
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, '#!/bin/sh\nexit 99\n', { mode: 0o700 });
    return realpathSync(path);
  };
  return { root, select: resolveStreamsBin, executable };
}

test('selector import is inert; default ignores .venv, PATH and stale release binaries', async t => {
  const f = await fixture(t);
  const python = f.executable(join(f.root, '.venv/bin/streams'));
  const pathBinary = f.executable(join(f.root, 'path/streams'));
  f.executable(join(f.root, 'target/release/streams'));
  assert.throws(() => f.select({ PATH: dirname(pathBinary) }), error => {
    assert.ok(error.message.includes(join(f.root, 'target/debug/streams')));
    assert.match(error.message, /cargo build --locked --bin streams/);
    assert.ok(!error.message.includes(python));
    return true;
  });
  const debug = f.executable(join(f.root, 'target/debug/streams'));
  assert.equal(f.select({ PATH: dirname(pathBinary) }), debug);
});

test('Cargo target requests resolve against package root, never the caller cwd or default target', async t => {
  const f = await fixture(t);
  f.executable(join(f.root, 'target/debug/streams'));
  const requested = f.executable(join(f.root, 'custom target/debug/streams'));
  const cwd = process.cwd();
  process.chdir(dirname(f.root));
  try {
    assert.equal(f.select({ CARGO_TARGET_DIR: 'custom target' }), requested);
    assert.equal(f.select({ CARGO_TARGET_DIR: join(f.root, 'custom target') }), requested);
    assert.throws(() => f.select({ CARGO_TARGET_DIR: 'absent-target' }), /absent-target\/debug\/streams/);
  } finally { process.chdir(cwd); }
});

test('explicit executable overrides are canonical and take priority over Cargo selection', async t => {
  const f = await fixture(t), binary = f.executable(join(f.root, 'installed streams'));
  const alias = join(f.root, 'symlink');symlinkSync(binary, alias);
  assert.equal(f.select({ STREAMS_BIN_PATH: alias, CARGO_TARGET_DIR: 'absent' }), binary);
  assert.equal(f.select({ STREAMS_RUST_BIN: alias }), binary);
  assert.equal(f.select({ STREAMS_BIN_PATH: binary, STREAMS_RUST_BIN: alias }), binary);
  const other = f.executable(join(f.root, 'other streams'));
  assert.throws(() => f.select({ STREAMS_BIN_PATH: binary, STREAMS_RUST_BIN: other }), /select different executables/);
});

test('invalid explicit selectors fail without trying valid aliases, Cargo, .venv or PATH', async t => {
  const f = await fixture(t), binary = f.executable(join(f.root, 'target/debug/streams'));
  const nonExecutable = f.executable(join(f.root, 'not executable'));chmodSync(nonExecutable, 0o600);
  f.executable(join(f.root, '.venv/bin/streams'));
  for (const selector of ['STREAMS_BIN_PATH', 'STREAMS_RUST_BIN']) {
    for (const value of ['', 'streams', resolve(f.root, 'missing'), f.root, nonExecutable]) {
      assert.throws(() => f.select({ [selector]: value, PATH: dirname(binary) }), new RegExp(selector));
    }
  }
  assert.throws(() => f.select({ STREAMS_BIN_PATH: '', STREAMS_RUST_BIN: binary }), /STREAMS_BIN_PATH/);
  assert.throws(() => f.select({ STREAMS_BIN_PATH: binary, STREAMS_RUST_BIN: '' }), /STREAMS_RUST_BIN/);
});
