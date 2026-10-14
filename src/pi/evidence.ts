export type DialogueText = Readonly<{ role: 'user' | 'assistant'; text: string }>;
export type ProjectedEvidence = Readonly<{
  compactionSummary: string | null;
  transcript: readonly DialogueText[];
}>;

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
