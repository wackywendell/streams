import test from 'node:test';
import assert from 'node:assert/strict';
import { createJiti } from 'jiti';

const jiti = createJiti(import.meta.url);
const { registerArtifactTool } = await jiti.import('../extensions/stream-checkpoint/artifacts.ts');
const { StreamCli } = await jiti.import('../extensions/stream-checkpoint/streams.ts');

function fixture(respond = () => ({ reference: 'host/pi/sess-1/assessment.md' })) {
  const calls = [];
  const cli = new StreamCli('/working', async (program, args, options) => {
    calls.push({ program, args, options });
    return { code: 0, stdout: JSON.stringify(respond(args)), stderr: '' };
  });
  let tool;
  registerArtifactTool(
    {
      registerTool: (value) => {
        tool = value;
      },
    },
    () => cli
  );
  const ctx = { cwd: '/working', sessionManager: { getSessionId: () => 'sess-1' } };
  const invoke = (args) => tool.execute('call', args, new AbortController().signal, undefined, ctx);
  return { tool, calls, invoke, cli };
}

test('artifact tool routes write text, file paths, and description-only edits through public CLI', async () => {
  const f = fixture();
  assert.equal(f.tool.name, 'artifact');
  await f.invoke({
    action: 'write',
    name: 'assessment.md',
    description: 'Findings',
    text: 'useful output',
  });
  await f.invoke({
    action: 'write',
    name: 'assessment.md',
    description: 'Findings',
    file: '/not-readable-by-pi/report.md',
  });
  await f.invoke({
    action: 'write',
    name: 'assessment.md',
    description: 'New description',
  });

  assert.deepEqual(
    f.calls.map((c) => ({ program: c.program, args: c.args, stdin: c.options?.stdin })),
    [
      {
        program: 'streams',
        args: ['artifact', 'write', 'assessment.md', '--session', 'pi/sess-1', '--description', 'Findings', '--json'],
        stdin: 'useful output',
      },
      {
        program: 'streams',
        args: [
          'artifact',
          'write',
          'assessment.md',
          '--session',
          'pi/sess-1',
          '--description',
          'Findings',
          '--file',
          '/not-readable-by-pi/report.md',
          '--json',
        ],
        stdin: undefined,
      },
      {
        program: 'streams',
        args: [
          'artifact',
          'write',
          'assessment.md',
          '--session',
          'pi/sess-1',
          '--description',
          'New description',
          '--description-only',
          '--json',
        ],
        stdin: undefined,
      },
    ]
  );
  assert.match(f.tool.description, /not instructions/);
  assert.match(f.tool.description, /Do not use for design docs, PR descriptions, progress reports/);
});

test('artifact filename and description guidance stay clear without rewriting literal names', async () => {
  const f = fixture();
  const nameGuidance = f.tool.parameters.properties.name.description;
  const descGuidance = f.tool.parameters.properties.description.description;
  assert.match(nameGuidance, /short filename naming the subject or result/);
  assert.match(nameGuidance, /Do not add artifact- merely to label the storage kind/);
  assert.match(descGuidance, /one short line, at most 240 characters/);
  assert.match(descGuidance, /teammate who knows the project but has not read this conversation/);
  assert.match(descGuidance, /Prefer meaning over provenance/);

  await f.invoke({
    action: 'write',
    name: 'artifact-review.md',
    description: 'Review of artifact storage',
    text: 'Findings',
  });
  assert.ok(f.calls[0].args.includes('artifact-review.md'));
});

test('artifact reads use bounded byte pages and preserve complete format-5 references', async () => {
  const calls = [];
  const cli = new StreamCli('/working', async (program, args) => {
    calls.push({ program, args });
    return { code: 0, stdout: '0123456789abcdefghij', stderr: '' };
  });
  let tool;
  registerArtifactTool({ registerTool: (v) => (tool = v) }, () => cli);
  const ctx = { cwd: '/working', sessionManager: { getSessionId: () => 'sess-1' } };
  const result = await tool.execute(
    'call',
    { action: 'read', reference: 'foreign/pi/sess-2/assessment.md', offset: 0, limit: 10 },
    new AbortController().signal,
    undefined,
    ctx
  );
  assert.deepEqual(calls[0].args, ['artifact', 'read', 'foreign/pi/sess-2/assessment.md']);
  assert.match(result.content[0].text, /0123456789/);
  assert.equal(result.details.next_offset, 10);
  assert.equal(result.details.total_bytes, 20);

  const page2 = await tool.execute(
    'call',
    { action: 'read', reference: 'foreign/pi/sess-2/assessment.md', offset: 10, limit: 10 },
    new AbortController().signal,
    undefined,
    ctx
  );
  assert.match(page2.content[0].text, /abcdefghij/);
  assert.equal(page2.details.next_offset, undefined);
});

test('artifact lists stay scoped and invalid tool inputs never reach the CLI', async () => {
  const f = fixture(() => ({ items: [] }));
  await f.invoke({ action: 'list' });
  await f.invoke({ action: 'list', scope: 'stream', stream: 'foreign/work' });
  await f.invoke({ action: 'list', session: 'host/pi/other' });
  await f.invoke({ action: 'delete', reference: 'host/pi/sess-1/assessment.md' });

  assert.deepEqual(
    f.calls.map((c) => c.args),
    [
      ['artifact', 'list', '--session', 'pi/sess-1', '--json'],
      ['artifact', 'list', '--stream', 'foreign/work', '--json'],
      ['artifact', 'list', '--session', 'host/pi/other', '--json'],
      ['artifact', 'delete', 'host/pi/sess-1/assessment.md', '--confirm', '--json'],
    ]
  );

  for (const args of [
    { action: 'write', name: 'assessment.md' },
    { action: 'write', name: 'assessment.md', description: 'Findings', text: 'one', file: 'two' },
    { action: 'write', name: 'assessment.md', description: 'Findings', text: 'one', session: 'other' },
    { action: 'list', scope: 'stream' },
    { action: 'list', scope: 'stream', stream: 'work', session: 'other' },
    { action: 'read' },
    { action: 'delete' },
  ]) {
    await assert.rejects(f.invoke(args));
  }
  assert.equal(f.calls.length, 4);
});

test('artifact CLI errors throw while exit code 3 returns parsed result', async () => {
  let code = 1;
  const cli = new StreamCli('/working', async () =>
    code === 1
      ? { code: 1, stdout: '', stderr: 'refused' }
      : { code: 3, stdout: JSON.stringify({ deleted: 'host/pi/sess/name', warnings: ['partial'] }), stderr: '' }
  );
  await assert.rejects(
    cli.artifact('sess', { action: 'delete', reference: 'host/pi/sess/name' }),
    /refused/
  );
  code = 3;
  const res = await cli.artifact('sess', { action: 'delete', reference: 'host/pi/sess/name' });
  assert.deepEqual(res.warnings, ['partial']);
});
