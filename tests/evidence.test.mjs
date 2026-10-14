import test from 'node:test';
import assert from 'node:assert/strict';
import { normalizeEvidence } from '../dist/pi/evidence.js';

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
