import test from 'node:test';
import assert from 'node:assert/strict';
import { SessionManager } from '@earendil-works/pi-coding-agent';
import { normalizeEvidence, normalizeReferenceEvidence, referenceEvidenceContinues } from '../dist/pi/evidence.js';

test('projection normalization copies only compaction summary and user/assistant text', () => {
  const messages = [
    null, [], { role: 'system', content: 'SYSTEM_SECRET' },
    { role: 'compactionSummary', summary: 'Earlier durable milestone', content: 'WRONG_SECRET' },
    { role: 'user', content: ' User request ' },
    { role: 'assistant', content: [
      { type: 'text', text: 'Visible response', textSignature: 'SIGNATURE_SECRET' },
      { type: 'text', text: 'Second block' },
      { type: 'thinking', thinking: 'THINKING_SECRET' },
      { type: 'toolCall', name: 'bash', arguments: { command: 'ARG_SECRET' } },
      { type: 'image', data: 'ASSISTANT_IMAGE_SECRET' },
      { type: 'text', text: 123 }, null,
    ] },
    { role: 'toolResult', details: { secret: 'DETAIL_SECRET' }, content: [
      { type: 'text', text: 'TOOL_SECRET' }, { type: 'image', data: 'IMAGE_SECRET' },
    ] },
    { role: 'custom', content: 'CUSTOM_SECRET' },
    { role: 'branchSummary', summary: 'BRANCH_SECRET' },
    { role: 'user', content: ' \n ' }, { role: 'assistant', content: {} },
  ];
  const before = structuredClone(messages);
  const evidence = normalizeEvidence(messages);
  assert.deepEqual(evidence, {
    compactionSummary: 'Earlier durable milestone',
    transcript: [
      { role: 'user', text: ' User request ' },
      { role: 'assistant', text: 'Visible response\nSecond block' },
    ],
  });
  assert.doesNotMatch(JSON.stringify(evidence), /SECRET/);
  assert.deepEqual(messages, before);
  evidence.transcript[0].text = 'Changed normalized record';
  assert.deepEqual(messages, before);
  messages[5].content[0].text = 'Changed host record';
  assert.equal(evidence.transcript[1].text, 'Visible response\nSecond block');
});

test('reference evidence preserves inline block order and repeated paths without changing own evidence', () => {
  const messages = [
    { role: 'compactionSummary', summary: 'Old' },
    { role: 'user', content: 'Start' },
    { role: 'assistant', content: [
      { type: 'text', text: 'Before' },
      { type: 'toolCall', name: 'read', arguments: { path: './notes.md', offset: 1 } },
      { type: 'text', text: 'After' },
      { type: 'text', text: 'Adjacent' },
      { type: 'toolCall', name: 'write', arguments: { path: './notes.md', content: 'BODY_SECRET' } },
      { type: 'toolCall', name: 'edit', arguments: { path: './notes.md', edits: [{ oldText: 'OLD_SECRET', newText: 'NEW_SECRET' }] } },
    ] },
    { role: 'user', content: 'Again' },
    { role: 'assistant', content: [{ type: 'toolCall', name: 'read', arguments: { path: './notes.md' } }] },
    { role: 'compactionSummary', summary: 'Latest' },
  ];
  assert.deepEqual(normalizeReferenceEvidence(messages), {
    compactionSummary: 'Latest',
    transcript: [
      { role: 'user', text: 'Start' },
      { role: 'assistant', text: 'Before' },
      { role: 'tool_activity', operation: 'read', path: './notes.md' },
      { role: 'assistant', text: 'After' },
      { role: 'assistant', text: 'Adjacent' },
      { role: 'tool_activity', operation: 'write', path: './notes.md' },
      { role: 'tool_activity', operation: 'edit', path: './notes.md' },
      { role: 'user', text: 'Again' },
      { role: 'tool_activity', operation: 'read', path: './notes.md' },
    ],
  });
  assert.deepEqual(normalizeEvidence(messages), {
    compactionSummary: 'Latest',
    transcript: [
      { role: 'user', text: 'Start' },
      { role: 'assistant', text: 'Before\nAfter\nAdjacent' },
      { role: 'user', text: 'Again' },
    ],
  });
});

test('reference activity admits only well-formed control-free nonblank path strings and exact tools', () => {
  const invalidPaths = [undefined, null, 123, {}, [], '', ' \t ', 'a\nb', 'a\u0000b', 'a\u007fb', 'a\u0085b', 'a\u202eb', 'a\u2028b', 'a\u2029b', '\ud800', '\udc00'];
  const messages = [
    null, [], { role: 'system', content: 'SYSTEM_SECRET' },
    { role: 'user', content: [{ type: 'toolCall', name: 'read', arguments: { path: 'USER_SECRET' } }] },
    { role: 'assistant', content: [
      ...invalidPaths.map(path => ({ type: 'toolCall', name: 'read', arguments: { path } })),
      ...[null, [], 'RAW_SECRET', 12].map(args => ({ type: 'toolCall', name: 'read', arguments: args })),
      ...['bash', 'Read', 'tools.read', 'grep', 'unknown'].map(name => ({ type: 'toolCall', name, arguments: { path: 'UNKNOWN_SECRET', command: 'SHELL_SECRET' } })),
      { type: 'thinking', thinking: 'THINKING_SECRET' },
      { type: 'image', data: 'IMAGE_SECRET' },
      { type: 'text', text: ' \n ' },
      { type: 'text', text: 123 },
      { type: 'toolCall', name: 'read', arguments: { path: ' ../a file/😀.md ', secret: 'ARG_SECRET' } },
      { type: 'toolCall', name: 'write', arguments: { path: '/absolute/file', content: 'BODY_SECRET' } },
      { type: 'toolCall', name: 'edit', arguments: { path: 'C:\\project\\file', replacement: 'REPLACEMENT_SECRET' } },
    ] },
    { role: 'toolResult', content: 'OUTPUT_SECRET', arguments: { path: 'OUTPUT_PATH_SECRET' } },
    { role: 'branchSummary', summary: 'BRANCH_SECRET' },
  ];
  const evidence = normalizeReferenceEvidence(messages);
  assert.deepEqual(evidence, { compactionSummary: null, transcript: [
    { role: 'tool_activity', operation: 'read', path: ' ../a file/😀.md ' },
    { role: 'tool_activity', operation: 'write', path: '/absolute/file' },
    { role: 'tool_activity', operation: 'edit', path: 'C:\\project\\file' },
  ] });
  assert.doesNotMatch(JSON.stringify(evidence), /SECRET/);
});

test('nested metadata requires a matching visible prior codemode call and keeps result-position order', () => {
  const nestedCalls = { complete: false, calls: [
    { id: 'a', name: 'read', arguments: { path: 'nested-a', secret: 'ARG_SECRET' }, status: 'ok' },
    { id: 'b', name: 'write', arguments: { path: 'nested-b', content: 'BODY_SECRET' }, status: 'error', error: 'ERROR_SECRET' },
    { id: 'c', name: 'edit', arguments: { path: 'nested-c', edits: 'EDIT_SECRET' }, status: 'unfinished' },
    { id: 'd', name: 'read', argumentsBytes: 50000, status: 'ok' },
    { id: 'e', name: 'bash', arguments: { path: 'UNKNOWN_SECRET' }, status: 'ok' },
    { id: 'f', name: 'read', arguments: '{"path":"SERIALIZED_SECRET"}', status: 'ok' },
    null,
  ] };
  const result = { role: 'toolResult', toolCallId: 'parent', toolName: 'codemode', nestedCalls,
    content: [{ type: 'text', text: 'OUTPUT_SECRET' }], details: { calls: [{ name: 'read', arguments: '{"path":"DETAIL_SECRET"}' }] } };
  const parent = { role: 'assistant', content: [
    { type: 'toolCall', id: 'parent', name: 'codemode', arguments: { code: 'await tools.read({path:"CODE_SECRET"})' } },
    { type: 'text', text: 'Waiting' },
  ] };
  assert.deepEqual(normalizeReferenceEvidence([parent, { role: 'user', content: 'Meanwhile' }, result, { role: 'assistant', content: 'Done' }]), {
    compactionSummary: null, transcript: [
      { role: 'assistant', text: 'Waiting' }, { role: 'user', text: 'Meanwhile' },
      { role: 'tool_activity', operation: 'read', path: 'nested-a' },
      { role: 'tool_activity', operation: 'write', path: 'nested-b' },
      { role: 'tool_activity', operation: 'edit', path: 'nested-c' },
      { role: 'assistant', text: 'Done' },
    ],
  });
  for (const messages of [
    [result], [result, parent],
    [parent, { ...result, toolCallId: 'other' }],
    [parent, { ...result, toolName: 'bash' }],
    [{ role: 'assistant', content: [{ type: 'toolCall', id: 'parent', name: 'bash', arguments: {} }] }, result],
    [parent, { ...result, nestedCalls: { complete: false } }],
    [parent, { ...result, nestedCalls: { calls: 'MALFORMED_SECRET', complete: true } }],
    [parent, { ...result, nestedCalls: undefined }],
  ]) {
    assert.equal(normalizeReferenceEvidence(messages).transcript.some(entry => entry.role === 'tool_activity'), false);
  }
});

test('real SDK projection respects replaced arguments, removed parents and redacted or removed results', () => {
  const manager = SessionManager.inMemory('/project');
  const direct = manager.appendMessage({ role: 'assistant', content: [
    { type: 'toolCall', id: 'direct', name: 'read', arguments: { path: 'OLD_SECRET' } },
    { type: 'toolCall', id: 'dropped', name: 'write', arguments: { path: 'DROPPED_SECRET' } },
  ] });
  const parent = manager.appendMessage({ role: 'assistant', content: [
    { type: 'toolCall', id: 'parent', name: 'codemode', arguments: { code: 'CODE_SECRET' } },
  ] });
  const result = manager.appendMessage({ role: 'toolResult', toolName: 'codemode', toolCallId: 'parent',
    content: [{ type: 'text', text: 'OUTPUT_SECRET' }],
    nestedCalls: { complete: true, calls: [{ id: 'nested', name: 'read', arguments: { path: 'nested.md' }, status: 'ok' }] },
  });
  manager.appendContextEdit(direct, { content: [
    { type: 'toolCall', id: 'direct', name: 'read', arguments: { path: 'current.md' } },
  ] });
  manager.appendContextEdit(result, { content: [] });
  const activities = () => normalizeReferenceEvidence(manager.buildSessionProjection().messages).transcript;
  assert.deepEqual(activities(), [
    { role: 'tool_activity', operation: 'read', path: 'current.md' },
    { role: 'tool_activity', operation: 'read', path: 'nested.md' },
  ]);
  manager.appendContextEdit(parent, { content: [{ type: 'text', text: 'Parent call removed' }] });
  assert.deepEqual(activities(), [
    { role: 'tool_activity', operation: 'read', path: 'current.md' },
    { role: 'assistant', text: 'Parent call removed' },
  ]);
  manager.appendContextEdit(parent, null);
  assert.equal(manager.buildSessionProjection().messages.some(message => message.role === 'toolResult'), true);
  assert.deepEqual(activities(), [{ role: 'tool_activity', operation: 'read', path: 'current.md' }]);
  manager.appendContextEdit(result, null);
  manager.appendContextEdit(direct, null);
  assert.deepEqual(activities(), []);
});

test('reference evidence uses only postcompaction projected history', () => {
  const manager = SessionManager.inMemory('/project');
  manager.appendMessage({ role: 'assistant', content: [{ type: 'toolCall', name: 'read', arguments: { path: 'PRECOMPACTION_SECRET' } }] });
  const kept = manager.appendMessage({ role: 'user', content: 'Current request' });
  manager.appendCompaction('Earlier summary', kept, 1000);
  assert.deepEqual(normalizeReferenceEvidence(manager.buildSessionProjection().messages), {
    compactionSummary: 'Earlier summary', transcript: [{ role: 'user', text: 'Current request' }],
  });
});

test('reference evidence copies uncapped chronological text without mutating projected input', () => {
  const text = 'EARLIEST_REQUEST' + '\n"\\'.repeat(24000);
  const messages = Object.freeze([
    Object.freeze({ role: 'user', content: text }),
    Object.freeze({ role: 'assistant', content: Object.freeze([
      Object.freeze({ type: 'toolCall', name: 'read', arguments: Object.freeze({ path: 'file.md' }) }),
      Object.freeze({ type: 'text', text }),
    ]) }),
    Object.freeze({ role: 'toolResult', content: 'OUTPUT_SECRET'.repeat(24000) }),
  ]);
  const before = structuredClone(messages);
  const evidence = normalizeReferenceEvidence(messages);
  assert.deepEqual(evidence, { compactionSummary: null, transcript: [
    { role: 'user', text }, { role: 'tool_activity', operation: 'read', path: 'file.md' }, { role: 'assistant', text },
  ] });
  evidence.transcript[0].text = 'Changed copy';
  evidence.transcript[1].path = 'Changed path';
  assert.deepEqual(messages, before);
  assert.equal(normalizeReferenceEvidence([{ role: 'compactionSummary', summary: 123 }]).compactionSummary, null);
});

test('reference continuity permits appended input but rejects edited or removed captured evidence', () => {
  const previous = { compactionSummary: 'Checkpoint', transcript: [
    { role: 'user', text: 'Review the design' },
    { role: 'tool_activity', operation: 'read', path: 'plan.md' },
  ] };
  assert.equal(referenceEvidenceContinues(previous, structuredClone(previous)), true);
  assert.equal(referenceEvidenceContinues(previous, { ...previous, transcript: [...previous.transcript, { role: 'assistant', text: 'Reviewed' }] }), true);
  for (const current of [
    { ...previous, compactionSummary: 'New checkpoint' },
    { ...previous, transcript: previous.transcript.slice(0, 1) },
    { ...previous, transcript: [{ role: 'user', text: '[redacted]' }, previous.transcript[1]] },
    { ...previous, transcript: [previous.transcript[0], { role: 'tool_activity', operation: 'read', path: 'other.md' }] },
    { ...previous, transcript: [previous.transcript[0], { role: 'tool_activity', operation: 'edit', path: 'plan.md' }] },
  ]) assert.equal(referenceEvidenceContinues(previous, current), false);
});

test('latest projected compaction summary uses the summary field; no compaction is null', () => {
  assert.deepEqual(normalizeEvidence([]), { compactionSummary: null, transcript: [] });
  assert.equal(normalizeEvidence([{ role: 'branchSummary', summary: 'Not compaction' }]).compactionSummary, null);
  assert.equal(normalizeEvidence([
    { role: 'compactionSummary', summary: 'Old' },
    { role: 'compactionSummary', summary: 'Latest', content: 'Not the summary' },
  ]).compactionSummary, 'Latest');
});

test('all chronological dialogue survives beyond 24k despite large trailing tool output', () => {
  const userText = 'EARLIEST_REQUEST' + '\n"\\'.repeat(12000);
  const assistantText = 'Outcome '.repeat(6000);
  const messages = Object.freeze([
    Object.freeze({ role: 'user', content: userText }),
    Object.freeze({ role: 'assistant', content: assistantText }),
    Object.freeze({ role: 'toolResult', content: 'TOOL_SECRET'.repeat(24000) }),
  ]);
  const before = structuredClone(messages);
  assert.deepEqual(normalizeEvidence(messages), {
    compactionSummary: null,
    transcript: [{ role: 'user', text: userText }, { role: 'assistant', text: assistantText }],
  });
  assert.deepEqual(messages, before);
});
