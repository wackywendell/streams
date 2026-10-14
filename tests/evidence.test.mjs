import test from 'node:test';
import assert from 'node:assert/strict';
import { recentText } from '../dist/pi/evidence.js';

test('host projection normalization copies only user, assistant and tool-result text', () => {
  const messages = [
    null, [], { role: 'system', content: 'SYSTEM_SECRET' },
    { role: 'user', content: 'User request' },
    { role: 'assistant', content: [
      { type: 'text', text: 'Visible response', textSignature: 'SIGNATURE_SECRET' },
      { type: 'text', text: 'Second block' },
      { type: 'thinking', thinking: 'THINKING_SECRET' },
      { type: 'toolCall', name: 'bash', arguments: { command: 'ARG_SECRET' } },
      { type: 'text', text: 123 }, null,
    ] },
    { role: 'toolResult', details: { secret: 'DETAIL_SECRET' }, content: [
      { type: 'text', text: 'Result evidence' }, { type: 'image', data: 'IMAGE_SECRET' },
    ] },
    { role: 'custom', content: 'CUSTOM_SECRET' },
    { role: 'user', content: ' \n ' }, { role: 'assistant', content: {} },
  ];
  const before = structuredClone(messages);
  const evidence = recentText(messages, 24000);
  assert.deepEqual(evidence, [
    { role: 'user', text: 'User request' },
    { role: 'assistant', text: 'Visible response\nSecond block' },
    { role: 'toolResult', text: 'Result evidence' },
  ]);
  assert.doesNotMatch(JSON.stringify(evidence), /SECRET/);
  assert.deepEqual(messages, before);
});

test('queued text budget walks newest first, keeps suffixes and preserves omission semantics', () => {
  const messages = Object.freeze([
    Object.freeze({ role: 'user', content: 'oldest' }),
    Object.freeze({ role: 'assistant', content: 'earlier text' }),
    Object.freeze({ role: 'toolResult', content: 'new' }),
  ]);
  const before = structuredClone(messages);
  assert.deepEqual(recentText(messages, 8), [
    { role: 'assistant', text: '[Earlier text omitted]\n text' },
    { role: 'toolResult', text: 'new' },
  ]);
  assert.deepEqual(recentText(messages, 0), []);
  assert.deepEqual(recentText(messages, 3), [{ role: 'toolResult', text: 'new' }]);
  assert.deepEqual(messages, before);
});
