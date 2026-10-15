import { isDeepStrictEqual } from 'node:util';

export type DialogueText = Readonly<{ role: 'user' | 'assistant'; text: string }>;
export type ProjectedEvidence = Readonly<{
  compactionSummary: string | null;
  transcript: readonly DialogueText[];
}>;

type FileActivity = Readonly<{
  role: 'tool_activity';
  operation: 'read' | 'write' | 'edit';
  path: string;
}>;
export type ReferenceEvidence = Readonly<{
  compactionSummary: string | null;
  transcript: readonly (DialogueText | FileActivity)[];
}>;

/** Appends are safe; edits to captured evidence invalidate an in-flight proposal. */
export function referenceEvidenceContinues(previous: ReferenceEvidence, current: ReferenceEvidence): boolean {
  return previous.compactionSummary === current.compactionSummary
    && previous.transcript.length <= current.transcript.length
    && previous.transcript.every((entry, index) => isDeepStrictEqual(entry, current.transcript[index]));
}

function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

/** Copy only the projected compaction checkpoint and dialogue; branch summaries remain excluded. */
export function normalizeEvidence(messages: readonly unknown[]): ProjectedEvidence {
  let compactionSummary: string | null = null;
  const transcript: DialogueText[] = [];
  for (const message of messages) {
    if (!object(message)) continue;
    if (message.role === 'compactionSummary') {
      compactionSummary = typeof message.summary === 'string' ? message.summary : null;
      continue;
    }
    if (message.role !== 'user' && message.role !== 'assistant') continue;
    const content = message.content;
    const text = typeof content === 'string' ? content : Array.isArray(content)
      ? content.filter(block => object(block) && block.type === 'text' && typeof block.text === 'string')
        .map(block => block.text as string).join('\n') : '';
    if (text.trim()) transcript.push({ role: message.role, text });
  }
  return { compactionSummary, transcript };
}

function fileActivity(call: unknown): FileActivity | null {
  if (!object(call) || (call.name !== 'read' && call.name !== 'write' && call.name !== 'edit')) return null;
  if (!object(call.arguments)) return null;
  const path = call.arguments.path;
  if (typeof path !== 'string' || !path.trim() || !path.isWellFormed() || /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u.test(path)) return null;
  return { role: 'tool_activity', operation: call.name, path };
}

/** Project attempted file activity inline, without copying arguments, tool output or hidden history. */
export function normalizeReferenceEvidence(messages: readonly unknown[]): ReferenceEvidence {
  let compactionSummary: string | null = null;
  const transcript: (DialogueText | FileActivity)[] = [];
  const codemodeCalls = new Set<string>();
  for (const message of messages) {
    if (!object(message)) continue;
    if (message.role === 'compactionSummary') {
      compactionSummary = typeof message.summary === 'string' ? message.summary : null;
      continue;
    }
    if (message.role === 'toolResult') {
      // Only metadata belonging to a still-visible, prior codemode call is evidence.
      if (message.toolName !== 'codemode' || typeof message.toolCallId !== 'string'
        || !codemodeCalls.has(message.toolCallId) || !object(message.nestedCalls)
        || !Array.isArray(message.nestedCalls.calls)) continue;
      for (const call of message.nestedCalls.calls) {
        const activity = fileActivity(call);
        if (activity) transcript.push(activity);
      }
      continue;
    }
    if (message.role !== 'user' && message.role !== 'assistant') continue;
    if (typeof message.content === 'string') {
      if (message.content.trim()) transcript.push({ role: message.role, text: message.content });
      continue;
    }
    if (!Array.isArray(message.content)) continue;
    for (const block of message.content) {
      if (!object(block)) continue;
      if (block.type === 'text' && typeof block.text === 'string' && block.text.trim()) {
        transcript.push({ role: message.role, text: block.text });
      } else if (message.role === 'assistant' && block.type === 'toolCall') {
        if (block.name === 'codemode' && typeof block.id === 'string' && block.id.trim()) {
          codemodeCalls.add(block.id);
        }
        const activity = fileActivity(block);
        if (activity) transcript.push(activity);
      }
    }
  }
  return { compactionSummary, transcript };
}
