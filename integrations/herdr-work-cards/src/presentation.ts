/** Labels are presentation only; callers retain checkout keys and absolute targets. */
export function checkoutLabels(checkouts: Record<string, { directory: string }>): Record<string, string> {
  const entries = Object.entries(checkouts).map(([key, checkout]) => ({ key, directory: checkout.directory, parts: checkout.directory.split('/').filter(Boolean), depth: 1 }));
  const label = (entry: typeof entries[number]): string => entry.depth > entry.parts.length ? entry.directory : entry.parts.slice(-entry.depth).join('/');
  for (;;) {
    const collisions = entries.filter(e => entries.some(other => other !== e && label(other) === label(e)));
    if (!collisions.length) return Object.fromEntries(entries.map(e => [e.key, label(e) || '/']));
    if (collisions.some(e => e.depth > e.parts.length)) throw new Error('Duplicate checkout directory');
    for (const entry of collisions) entry.depth++;
  }
}
