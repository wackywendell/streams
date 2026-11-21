import assert from 'node:assert/strict';
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { testCli } from './helpers/stream-cli.mjs';

test('format-5 CLI card mode and hc label integration work end to end in isolated home', async (t) => {
  const root = mkdtempSync(join(tmpdir(), 'streams-native-rust-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));

  const binDir = join(root, 'bin');
  const hcLog = join(root, 'hc.log');
  const hcStub = join(binDir, 'hc');
  const workDir = join(root, 'work');
  const { mkdirSync } = await import('node:fs');
  mkdirSync(binDir, { recursive: true });
  mkdirSync(workDir, { recursive: true });
  writeFileSync(
    hcStub,
    `#!/bin/sh\nprintf '%s\\n' "$*" >> "${hcLog}"\nexit 0\n`,
    { mode: 0o755 }
  );
  chmodSync(hcStub, 0o755);

  const cli = testCli(workDir, {
    PATH: `${binDir}:${process.env.PATH ?? ''}`,
    HC_TERMINAL_ID: 'term-native-1',
  });

  const stream = await cli.createStream({ title: 'Native test', titleMode: 'manual' });
  const session = await cli.createSession('sess-native', {
    stream: stream.key,
    title: 'Worker',
    titleMode: 'manual',
    directory: workDir,
  });

  // Extension writes streams.session label via hc.
  await cli.writeSessionLabel(session.key);

  // `card mode` reads and updates card mode cleanly without requiring a running Herdr window.
  const modeDefault = await cli.cardMode('sess-native');
  assert.equal(modeDefault.effective, 'automatic');
  const modeManual = await cli.cardMode('sess-native', 'manual');
  assert.equal(modeManual.effective, 'manual');
  const modeAuto = await cli.cardMode('sess-native', 'automatic');
  assert.equal(modeAuto.effective, 'automatic');

  // Verify hc received the extension's `streams.session` label write.
  const lines = readFileSync(hcLog, 'utf8').trim().split('\n');
  assert.ok(lines.some((line) => line.includes(`label term-native-1 streams.session=${session.key}`)), lines.join('\n'));
});
