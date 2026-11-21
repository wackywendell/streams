import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { testCli } from './helpers/stream-cli.mjs';

test('format-5 CLI stream, session, edit, artifact, and catalog sync round-trip in isolated home', async (t) => {
  const root = mkdtempSync(join(tmpdir(), 'streams-sync-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));

  const workDir = join(root, 'work');
  const remoteDir = join(root, 'remote.git');
  mkdirSync(workDir, { recursive: true });
  execFileSync('git', ['init', '--bare', remoteDir], {
    env: {
      ...process.env,
      GIT_CONFIG_GLOBAL: '/dev/null',
      GIT_CONFIG_SYSTEM: '/dev/null',
      GIT_CONFIG_NOSYSTEM: '1',
    },
  });

  const cli = testCli(workDir);

  // Create Stream and Session.
  const createdStream = await cli.createStream({
    title: 'Sync Effort',
    titleMode: 'manual',
    goal: 'Verify format-5 CLI sync',
    goalShort: 'Verify sync',
    goalMode: 'automatic',
  });
  const createdSession = await cli.createSession('pi-sync-1', {
    stream: createdStream.key,
    title: 'Sync Lane',
    titleMode: 'automatic',
    directory: workDir,
  });

  // Read context back.
  const ctx = await cli.context('pi-sync-1');
  assert.equal(ctx.stream.id, createdStream.key.split('/')[1]);
  assert.equal(ctx.session.id, 'pi-sync-1');
  assert.equal(ctx.session.title.text, 'Sync Lane');
  assert.equal(ctx.session.directory.path, realpathSync(workDir));

  // Edit Session fields + reference with `before` check.
  const editRes = await cli.editSession(createdSession.key, {
    changes: {
      recap: { text: 'Exercise format-5 sync', short: 'Format-5 sync' },
      references: {
        'https://example.test/pr/1': {
          kind: 'pr',
          name: 'PR #1',
          purpose: 'Track change',
          role: 'deliverable',
          status: 'active',
          manual: false,
        },
      },
    },
    before: {
      recap: null,
      references: { 'https://example.test/pr/1': null },
    },
  });
  assert.deepEqual(editRes.not_written, {});

  // Stale `before` check returns exit 3 and populates `not_written` without overwriting.
  const staleRes = await cli.editSession(createdSession.key, {
    changes: { recap: { text: 'Stale write', short: null } },
    before: { recap: null },
  });
  assert.equal(staleRes.partial, true);
  assert.equal(staleRes.not_written.recap.text, 'Exercise format-5 sync');

  // Artifact write / read / list / delete.
  const written = await cli.artifact('pi-sync-1', {
    action: 'write',
    name: 'notes.md',
    description: 'Sync check notes',
    text: 'hello format-5 artifact',
  });
  assert.ok(written.key.endsWith('/pi/pi-sync-1/notes.md'));

  const read = await cli.artifact('pi-sync-1', {
    action: 'read',
    reference: written.key,
    offset: 0,
    limit: 5,
  });
  assert.equal(read.content, 'hello');
  assert.equal(read.next_offset, 5);

  const listed = await cli.artifact('pi-sync-1', { action: 'list_session' });
  assert.equal(listed.items.length, 1);

  await cli.artifact('pi-sync-1', { action: 'delete', reference: written.key });
  const afterDelete = await cli.artifact('pi-sync-1', { action: 'list_session' });
  assert.equal(afterDelete.items.length, 0);

  // Catalog sync (local-only, no remote configured).
  const syncResult = await cli.sync();
  assert.ok(['unchanged', 'persisted', 'committed'].includes(syncResult.local));
});
