export type TextEvidence = Readonly<{ role: 'user' | 'assistant' | 'toolResult'; text: string }>;

function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

/** Normalize projected host messages once; retain only bounded recent text, never host objects. */
export function recentText(messages: readonly unknown[], budget: number): readonly TextEvidence[] {
  const recent: TextEvidence[] = [];
  for (let i = messages.length - 1; i >= 0 && budget > 0; i--) {
    const message = messages[i];
    if (!object(message) || (message.role !== 'user' && message.role !== 'assistant' && message.role !== 'toolResult')) continue;
    const content = message.content;
    const text = typeof content === 'string' ? content : Array.isArray(content)
      ? content.filter(block => object(block) && block.type === 'text' && typeof block.text === 'string')
        .map(block => block.text as string).join('\n') : '';
    if (!text.trim()) continue;
    const clipped = text.length > budget ? `[Earlier text omitted]\n${text.slice(-budget)}` : text;
    recent.unshift({ role: message.role, text: clipped });
    budget -= clipped.length;
  }
  return recent;
}
