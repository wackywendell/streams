import assert from 'node:assert/strict';
import test from 'node:test';
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { createJiti } from 'jiti';

const jiti = createJiti(import.meta.url);
const source = readFileSync(new URL('../extensions/stream-checkpoint/streams-bin.ts', import.meta.url), 'utf8');
async function fixture(t) {
  const root = mkdtempSync(join(realpathSync(tmpdir()), 'streams-selector-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const module = join(root, 'extensions/stream-checkpoint/streams-bin.ts');
  mkdirSync(dirname(module), { recursive: true });
  writeFileSync(module, source);
  const { resolveStreamsBin, checkoutSource } = await jiti.import(module);
  const executable = path => {
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, '#!/bin/sh\nexit 99\n', { mode: 0o700 });
    return realpathSync(path);
  };
  return { root, select: resolveStreamsBin, source: checkoutSource, executable };
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

});

test('invalid explicit selectors fail without trying Cargo, .venv or PATH', async t => {
  const f = await fixture(t), binary = f.executable(join(f.root, 'target/debug/streams'));
  const nonExecutable = f.executable(join(f.root, 'not executable'));chmodSync(nonExecutable, 0o600);
  f.executable(join(f.root, '.venv/bin/streams'));
  for (const value of ['', 'streams', resolve(f.root, 'missing'), f.root, nonExecutable]) {
      assert.throws(() => f.select({ STREAMS_BIN_PATH: value, PATH: dirname(binary) }), /STREAMS_BIN_PATH/);
  }
});

test('a packaged extension has no checkout source; a checkout digest follows every Rust source', async t => {
  const f = await fixture(t);
  assert.equal(f.source(), null);
  for (const path of ['Cargo.toml', 'Cargo.lock', 'build.rs', 'src/main.rs']) {
    mkdirSync(dirname(join(f.root, path)), { recursive: true });writeFileSync(join(f.root, path), path);
  }
  const before = f.source();
  assert.match(before, /^[0-9a-f]{64}$/);
  assert.equal(f.source(), before);
  writeFileSync(join(f.root, 'src/main.rs'), 'changed');
  assert.notEqual(f.source(), before);
});
