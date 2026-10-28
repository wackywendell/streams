import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdtemp, writeFile, chmod, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const launcher = fileURLToPath(new URL('../bin/open-artifact.sh', import.meta.url));
async function fixture(t, body) {
 const root = await mkdtemp(join(tmpdir(), 'artifact-launch-'));
 t.after(() => rm(root, {recursive:true, force:true}));
 const binary = join(root, 'streams stub');
 await writeFile(binary, '#!/bin/sh\n' + body);
 await chmod(binary, 0o700);
 return {root, env:{HOME:root, PATH:'/usr/bin:/bin', STREAMS_BIN_PATH:binary,
  STREAMS_ARTIFACT_CWD:root, STREAMS_ARTIFACT_CATALOG_SOURCE:'catalog',
  STREAMS_ARTIFACT_REFERENCE:'host/stream/session/assessment.md'}};
}

test('artifact launcher keeps the reference literal and runs in the reader cwd', async t => {
 const f = await fixture(t, 'printf "%s\\n" "$PWD" "$#" "$1" "$2" "$3"\n');
 const child = spawnSync('/bin/sh', [launcher], {encoding:'utf8', env:f.env});
 assert.equal(child.status, 0, child.stderr);
 assert.deepEqual(child.stdout.trimEnd().split('\n'), [f.root, '3', 'artifact', 'view', f.env.STREAMS_ARTIFACT_REFERENCE]);
 assert.doesNotMatch(child.stderr, /Press Enter/);
});

test('artifact launcher preserves setup and reader errors until Enter', async t => {
 const f = await fixture(t, 'printf "reader refused catalog\\n" >&2\nexit 19\n');
 for (const changes of [{}, {STREAMS_ARTIFACT_CWD:join(f.root, 'missing')},
  {STREAMS_BIN_PATH:'relative'}, {STREAMS_ARTIFACT_REFERENCE:''}]) {
  const child = spawnSync('/bin/sh', [launcher], {encoding:'utf8', input:'\n', env:{...f.env, ...changes}});
  assert.notEqual(child.status, 0);
  assert.match(child.stderr, /Press Enter to close this tab/);
  assert.match(child.stderr, /failed/);
 }
});

test('a failed artifact reader stays visible and is not retried', {timeout:5000}, async t => {
 const f = await fixture(t, 'printf "reader invoked\\n" >&2\nexit 19\n');
 const child = spawn('/bin/sh', [launcher], {env:f.env});
 t.after(() => {if (child.exitCode === null) child.kill();});
 let stderr = '';
 const exited = new Promise(resolve => child.once('exit', resolve));
 await new Promise((resolve, reject) => {
  child.once('error', reject);
  child.once('exit', () => reject(Error('Viewer closed before Enter')));
  child.stderr.on('data', bytes => {
   stderr += bytes.toString();
   if (stderr.includes('Press Enter to close this tab')) resolve();
  });
 });
 assert.equal(child.exitCode, null);
 child.stdin.end('\n');
 assert.equal(await exited, 19);
 assert.equal(stderr.match(/reader invoked/g).length, 1);
});
