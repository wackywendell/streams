import assert from 'node:assert/strict';
import test from 'node:test';
import { createJiti } from 'jiti';

const jiti = createJiti(import.meta.url);
const { parseNamingEdit, saveNamingEdit, savePiNameEdit, namingCompletions } = await jiti.import(
  new URL('../extensions/stream-checkpoint/naming.ts', import.meta.url).pathname
);
const { editField } = await jiti.import(
  new URL('../extensions/stream-checkpoint/panel.ts', import.meta.url).pathname
);

function context() {
  return {
    stream: {
      host_id: 'host',
      id: 'effort',
      title: { text: 'Effort', manual: false },
      goal: { text: 'Help users', short: 'Help', manual: false },
      status: 'active',
      revision: 1,
      updated_at_ms: 0,
    },
    session: {
      host_id: 'host',
      harness: 'pi',
      id: 'native',
      stream_id: 'effort',
      title: { text: 'Saved title', manual: false },
      status: 'active',
      recap: null,
      state: { text: 'Work', short: null },
      remaining: null,
      directory: null,
      references: {},
      artifacts: {},
      revision: 1,
      updated_at_ms: 0,
    },
    naming: { stream: 'automatic', session: 'automatic' },
  };
}

const ok = (key) => ({ key, not_written: {} });

test('name syntax defaults to Session, preserves title text and pins manual', () => {
  assert.deepEqual(parseNamingEdit('name', '  My  task  '), {
    target: 'session',
    kind: 'name',
    text: 'My  task',
  });
  assert.deepEqual(parseNamingEdit('name', 'session Session task'), {
    target: 'session',
    kind: 'name',
    text: 'Session task',
  });
  assert.deepEqual(parseNamingEdit('name', 'stream Shared effort'), {
    target: 'stream',
    kind: 'name',
    text: 'Shared effort',
  });
  assert.equal(parseNamingEdit('name', 'automatic').text, 'automatic');
  for (const args of ['', ' ', 'session', 'stream ', 'bad\0name']) {
    assert.throws(() => parseNamingEdit('name', args), /usage/i);
  }
});

test('title-mode accepts only automatic or manual (no inherit)', () => {
  for (const mode of ['automatic', 'manual']) {
    assert.deepEqual(parseNamingEdit('title-mode', mode), {
      target: 'session',
      kind: 'mode',
      mode,
    });
    assert.deepEqual(parseNamingEdit('title-mode', `stream ${mode}`), {
      target: 'stream',
      kind: 'mode',
      mode,
    });
  }
  for (const args of ['', 'stream', 'inherit', 'stream inherit', 'manual extra', 'auto']) {
    assert.throws(() => parseNamingEdit('title-mode', args), /usage/i);
  }
  const completions = namingCompletions('title-mode ');
  assert.ok(completions.some((c) => c.label === 'automatic'));
  assert.ok(completions.some((c) => c.label === 'manual'));
  assert.ok(!completions.some((c) => c.label === 'inherit'));
});

test('saveNamingEdit saves format-5 title object with before check', async () => {
  for (const target of ['session', 'stream']) {
    const catalog = context();
    const calls = [];
    const client = {
      context: async (id) => {
        calls.push(['context', id]);
        return catalog;
      },
      editSession: async (key, body) => {
        calls.push(['editSession', key, body]);
        return ok(key);
      },
      editStream: async (key, body) => {
        calls.push(['editStream', key, body]);
        return ok(key);
      },
    };
    const title = catalog[target].title.text;
    const res = await saveNamingEdit(
      client,
      'native',
      parseNamingEdit('name', `${target} ${title}`),
      () => {}
    );
    assert.deepEqual(res.not_written, {});
    assert.equal(calls[0][0], 'context');
    assert.equal(calls[1][0], target === 'session' ? 'editSession' : 'editStream');
    assert.deepEqual(calls[1][2], {
      changes: { title: { text: title, manual: true } },
      before: { title: { text: title, manual: false } },
    });
  }
});

test('mode-only saves keep current title text and toggle manual boolean', async () => {
  for (const [args, expectedManual, target] of [
    ['manual', true, 'session'],
    ['stream automatic', false, 'stream'],
  ]) {
    const writes = [];
    const catalog = context();
    const client = {
      context: async () => catalog,
      editSession: async (key, body) => {
        writes.push(['session', key, body]);
        return ok(key);
      },
      editStream: async (key, body) => {
        writes.push(['stream', key, body]);
        return ok(key);
      },
    };
    await saveNamingEdit(client, 'native', parseNamingEdit('title-mode', args), () => {});
    assert.equal(writes[0][0], target);
    assert.deepEqual(writes[0][2].changes, {
      title: { text: catalog[target].title.text, manual: expectedManual },
    });
  }
});

test('panel title and goal edits set manual: true on save', async () => {
  for (const [field, entered, expected] of [
    ['title', 'Renamed effort', { text: 'Renamed effort', manual: true }],
    ['goal', 'New purpose', { text: 'New purpose', short: null, manual: true }],
    ['goal', '   ', null],
  ]) {
    const writes = [];
    const ctx = {
      ui: {
        notify: assert.fail,
        editor: async () => entered,
        select: async () => 'Save field',
      },
    };
    const catalog = context();
    await editField(
      ctx,
      {
        editStream: async (key, body) => {
          writes.push({ key, body });
          return ok(key);
        },
        editSession: async () => assert.fail('wrong target'),
      },
      catalog,
      'stream',
      field,
      { edits: new Map() },
      () => {}
    );
    assert.deepEqual(writes[0].body.changes, { [field]: expected });
  }
});

test('stale identity fences refuse before reading or saving', async () => {
  let reads = 0;
  let saves = 0;
  let checks = 0;
  const client = {
    context: async () => {
      reads++;
      return context();
    },
    editSession: async (key) => {
      saves++;
      return ok(key);
    },
    editStream: async (key) => {
      saves++;
      return ok(key);
    },
  };
  await assert.rejects(
    saveNamingEdit(client, 'native', parseNamingEdit('name', 'Task'), () => {
      throw Error('stale');
    }),
    /stale/
  );
  assert.equal(reads, 0);
  assert.equal(saves, 0);

  await assert.rejects(
    saveNamingEdit(client, 'native', parseNamingEdit('name', 'Task'), () => {
      if (++checks === 2) throw Error('stale');
    }),
    /stale/
  );
  assert.equal(reads, 1);
  assert.equal(saves, 0);
});

test('Pi names save only changed bound titles, without pinning matching echoes', async () => {
  for (const name of ['Saved title', 'Changed title']) {
    const catalog = context();
    const writes = [];
    const client = {
      context: async (id) => {
        assert.equal(id, 'native');
        return catalog;
      },
      editSession: async (key, body) => {
        writes.push({ key, body });
        return ok(key);
      },
    };
    const value = await savePiNameEdit(client, 'native', name, () => {});
    if (name === 'Saved title') {
      assert.equal(value, undefined);
      assert.deepEqual(writes, []);
    } else {
      assert.deepEqual(value, ok('host/pi/native'));
      assert.deepEqual(writes[0].body, {
        changes: { title: { text: name, manual: true } },
        before: { title: { text: 'Saved title', manual: false } },
      });
    }
  }
});

test('empty names and unbound Pi conversations do not save', async () => {
  let reads = 0;
  const client = {
    context: async () => {
      reads++;
      return { ...context(), session: null };
    },
    editSession: assert.fail,
  };
  for (const name of [undefined, '', '  ', 'bad\0name']) {
    assert.equal(await savePiNameEdit(client, 'native', name, assert.fail), undefined);
  }
  assert.equal(reads, 0);
  assert.equal(await savePiNameEdit(client, 'native', 'Native only', () => {}), undefined);
  assert.equal(reads, 1);
});
