import { readFileSync } from 'node:fs';

// Rust's patch schemas for default records, pinned by its own tests against this fixture.
const fixture = JSON.parse(readFileSync(new URL('../fixtures/agent-patch-schemas.json', import.meta.url), 'utf8'));
const manualTitle = (record, fallback) => (record.title_mode === 'inherit' ? fallback : record.title_mode) === 'manual';

/** A fake of Rust's patch schemas: automatic reports cannot write manual text. */
export function patchSchemas({stream, session, title_defaults}) {
  const schemas = {};
  if (stream) schemas.stream = {
    automatic: fixture.stream.automatic.filter(({field}) => field === 'title' ? !manualTitle(stream, title_defaults.stream) : stream.goal_mode !== 'manual'),
    user: fixture.stream.user,
  };
  if (session) schemas.session = {
    automatic: fixture.session.automatic.filter(({field}) => field !== 'title' || !manualTitle(session, title_defaults.session)),
    user: fixture.session.user,
  };
  return structuredClone(schemas);
}

/** Give a fake context patch schemas that follow later edits to its records, as Rust's would. */
export function withPatchSchemas(context) {
  // Assigning schemas is ignored: they are derived from the records.
  return Object.defineProperty(context, 'patch_schemas', {enumerable: true, configurable: true, get() { return patchSchemas(this); }, set() {}});
}
