import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdtemp, writeFile, chmod, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const launcher = fileURLToPath(new URL('../bin/open-document.sh', import.meta.url));
test('POSIX launcher keeps trusted quoted command args and hostile filenames literal in checkout cwd', async t => {
  const root = await mkdtemp(join(tmpdir(), 'work-cards-launch-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const stub = join(root, 'editor stub.sh');
  await writeFile(stub, '#!/bin/sh\nprintf "%s\\n" "$PWD" "$#" "$1" "$2"\n');
  await chmod(stub, 0o700);
  for (const command of ['/bin/sh "' + stub + '" "quoted arg"', '"' + stub + '" "quoted arg"']) for (const name of ['space name.md', 'quote\'".md', '-dash.md', '$HOME;`touch nope`.md']) {
    const path = join(root, name);
    const child = spawnSync('/bin/sh', [launcher], { encoding: 'utf8', env: {
      ...process.env, WORK_CARDS_DOCUMENT_COMMAND: command,
      WORK_CARDS_DOCUMENT_CWD: root, WORK_CARDS_DOCUMENT_PATH: path,
    } });
    assert.equal(child.status, 0, child.stderr);
    assert.deepEqual(child.stdout.trimEnd().split('\n'), [root, '2', 'quoted arg', path]);
  }
});
test('launcher exposes command and cwd failures and waits for Enter before closing', () => {
  for (const [command, cwd] of [['/bin/sh -c "exit 19"', '/'], ['command-that-does-not-exist', '/'], ['"unclosed', '/'], ['less', '/missing-work-cards-checkout']]) {
    const child = spawnSync('/bin/sh', [launcher], { encoding: 'utf8', input: '\n', env: {
      ...process.env, WORK_CARDS_DOCUMENT_COMMAND: command, WORK_CARDS_DOCUMENT_CWD: cwd, WORK_CARDS_DOCUMENT_PATH: '/literal file',
    } });
    assert.notEqual(child.status, 0);
    assert.match(child.stderr, /failed|cannot/i);
    assert.match(child.stderr, /Press Enter to close/);
    assert.match(child.stderr, /document_command|checkout/i);
  }
});

test('a failed launcher stays visible until Enter arrives on its input pipe', { timeout: 5000 }, async t => {
  const child = spawn('/bin/sh', [launcher], { env: {
    ...process.env, WORK_CARDS_DOCUMENT_COMMAND: '/bin/sh -c "exit 19"', WORK_CARDS_DOCUMENT_CWD: '/', WORK_CARDS_DOCUMENT_PATH: '/literal document',
  } });
  t.after(() => { if (child.exitCode === null) child.kill(); });
  let stderr = '';
  const exited = new Promise(resolve => child.on('exit', code => resolve(code)));
  await new Promise((resolve, reject) => {
    child.on('error', reject);
    child.on('exit', () => reject(new Error('Exited before Enter')));
    child.stderr.on('data', bytes => {
      stderr += bytes.toString();
      if (stderr.includes('Press Enter to close')) resolve();
    });
  });
  assert.equal(await Promise.race([exited.then(() => 'exited'), new Promise(resolve => setTimeout(() => resolve('waiting'), 40))]), 'waiting');
  child.stdin.end('\n');
  assert.equal(await exited, 19);
});
