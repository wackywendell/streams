import assert from 'node:assert/strict';
import test from 'node:test';
import { createJiti } from 'jiti';

const jiti = createJiti(import.meta.url);
const {
  StreamCli,
  formatNotWritten,
  referenceText,
  safeReferenceJson,
  syncNeedsAttention,
} = await jiti.import('../extensions/stream-checkpoint/streams.ts');
const { FieldEdit } = await jiti.import('../extensions/stream-checkpoint/core.ts');

test('syncNeedsAttention flags repair-required and sharing failures independently', () => {
  for (const sharing of ['local-only', 'pending', 'confirmed', 'failed', 'blocked', 'uncertain']) {
    const expected = ['failed', 'blocked', 'uncertain'].includes(sharing);
    assert.equal(syncNeedsAttention({ local: 'unchanged', sharing }), expected);
  }
  assert.equal(syncNeedsAttention({ local: 'repair-required', sharing: 'confirmed' }), true);
  assert.equal(syncNeedsAttention({ local: 'committed', needs_attention: true }), true);
});

test('editSession parses exit 0 and exit 3 partial results and rejects invalid shapes', async () => {
  const partial = {
    key: 'host/pi/sess',
    not_written: {
      title: { text: 'Other', manual: true },
      references: {
        'docs/plan.md': {
          kind: 'document',
          name: 'Plan',
          purpose: 'Design',
          role: 'deliverable',
          status: 'dropped',
          manual: true,
        },
      },
    },
  };
  let status = 3;
  const cli = new StreamCli('/unused', async () => ({
    code: status,
    stdout: JSON.stringify(partial),
    stderr: '',
  }));
  const res = await cli.editSession('host/pi/sess', {
    changes: { title: { text: 'Mine', manual: false } },
  });
  assert.deepEqual(res, { ...partial, partial: true });

  const formatted = formatNotWritten(res.not_written);
  assert.equal(formatted.length, 2);
  assert.match(formatted[0], /^title not written/);
  assert.match(formatted[1], /^reference .*docs\/plan\.md.*not written/);

  const badCli = new StreamCli('/unused', async () => ({
    code: 0,
    stdout: JSON.stringify({ key: 'host/pi/sess' }),
    stderr: '',
  }));
  await assert.rejects(
    badCli.editSession('host/pi/sess', { changes: { status: 'done' } }),
    /Invalid session edit response/
  );
});

test('safeReferenceJson escapes control/format chars while referenceText preserves safe text', () => {
  const key = { path: 'docs/\nplan.md', note: 'changed\u001b[31m' };
  assert.deepEqual(JSON.parse(safeReferenceJson(key)), key);
  assert.equal(referenceText('ordinary 🐟'), 'ordinary 🐟');
  assert.equal(referenceText('a\u{e0001}b'), '"a\\udb40\\udc01b"');
});

test('FieldEdit retains input when not_written refuses the edited field', async () => {
  const context = {
    stream: null,
    session: {
      host_id: 'laptop',
      harness: 'pi',
      id: 'sess',
      stream_id: 'effort',
      title: { text: 'Before', manual: false },
      status: 'active',
      recap: null,
      next: null,
      directory: null,
      references: {},
      artifacts: {},
      revision: 1,
      updated_at_ms: 0,
    },
    naming: { stream: 'automatic', session: 'automatic' },
  };
  const edit = new FieldEdit('session', context, 'title');
  edit.value = { text: 'After', manual: true };
  const results = [];
  await assert.rejects(
    edit.save(
      {
        editSession: async () => ({
          key: 'laptop/pi/sess',
          not_written: { title: { text: 'Concurrent', manual: true } },
        }),
        editStream: async () => assert.fail('wrong target'),
      },
      (r) => results.push(r)
    ),
    /changed concurrently/
  );
  assert.equal(edit.dirty, true);
  assert.deepEqual(edit.value, { text: 'After', manual: true });
  assert.equal(results.length, 1);
});
