export interface StateSections { goal: string; now: string; remaining: string }

/** Exact ordered carrier only. ` | ` and repeated field headers are reserved;
 * ambiguous text stays prose, including extra fields. Other literal pipes are content. */
export function parseStateSections(state: string): StateSections | undefined {
  if (/[\r\n\u2028\u2029]/u.test(state)) return undefined;
  const fields = state.split(' | ');
  const labels = ['Goal: ', 'Now: ', 'Remaining: '];
  if (fields.length !== labels.length) return undefined;
  const values: string[] = [];
  for (let index = 0; index < labels.length; index++) {
    if (!fields[index]!.startsWith(labels[index]!)) return undefined;
    const value = fields[index]!.slice(labels[index]!.length);
    if (!value || value !== value.trim() || /(?:Goal|Now|Remaining):/u.test(value)) return undefined;
    values.push(value);
  }
  return { goal: values[0]!, now: values[1]!, remaining: values[2]! };
}
