// Deterministic extraction only: no pane, filesystem document resolution, or native client.
// Run from the repository root after the integration's documented build task.
import { writeFileSync } from 'node:fs';
import { renderStreamsCard } from '../../../integrations/herdr-work-cards/dist/herdr/streams-view.js';
import stringWidth from '../../../integrations/herdr-work-cards/node_modules/string-width/index.js';
const unstyled = text => text.replace(/\x1b\[[\d;]*m/g, '').replace(/\x1b\]8;;[^\x1b]*\x1b\\/g, '');
const native = '12345678-1234-4234-8234-123456789abc';
function snapshot() {
  return {
    scope: { catalogSource: 'catalog', hostId: 'host', streamId: 'shared', sessionId: 'own', nativeSessionId: native },
    native: { frontend: 'herdr', pane_id: 'w:p1', tab_id: 'w:t1', workspace_id: 'w' },
    stream: { id: 'shared', title: 'Shared', owner: { host_id: 'host' }, title_mode: 'manual', goal_short: 'Ship library',
      goal: 'Ship library | client', repositories: { lib: { url: 'https://example.com/lib' } },
      documents: [{ repository: 'lib', ref: 'topic', path: 'docs/design.md', description: 'Design' }],
      issues: [{ url: 'https://example.com/org/repo/issues/42', description: 'Track work' }],
      prs: [{ url: 'https://example.com/org/repo/pull/51', description: 'Review' }] },
    session: { id: 'own', host_id: 'host', stream_id: 'shared', harness: 'pi', harness_session_id: native,
      title: 'Own assignment', title_mode: 'manual', state_short: 'Implement', status: 'active', state: 'Implement | verify', remaining: '界😀é | release' },
    checkouts: [{ repository: 'lib', checkout_id: 'lib', root: '/code/library' }], documentLocations: [],
  };
}
function locations(s) {
  s.documentLocations = s.stream.documents.map(({ repository, path }) => {
    const binding = s.checkouts.find(b => b.repository === repository);
    return { repository, path, checkout_id: binding?.checkout_id ?? null, root: binding?.root ?? null,
      file: binding ? `${binding.root}/${path}` : null, error: binding ? null : 'No registered checkout' };
  });
}
const cases = [];
function add(name, s, width, height, notice) {
  if (s && !s.documentLocations.length) locations(s);
  const rendered = renderStreamsCard(s, width, height, notice);
  cases.push({ name, snapshot: s ?? null, width, height, notice: notice ?? null,
    lines: rendered.text ? unstyled(rendered.text).split('\n').map(s => s.trimEnd()) : [],
    widths: rendered.text ? unstyled(rendered.text).split('\n').map(s => stringWidth(s)) : [],
    documents: [...rendered.documents],
    links: [...rendered.text.matchAll(/\x1b\]8;;([^\x1b]+)\x1b\\/g)].map(m => m[1]) });
}
for (const status of ['active', 'waiting', 'blocked', 'parked', 'done']) {
  const s = snapshot(); s.session.status = status; add(`status-${status}`, s, 100, 12);
}
{
  const s = snapshot(); Object.assign(s.scope, { sessionId: 'other', nativeSessionId: '12345678-1234-4234-8234-123456789abd' });
  Object.assign(s.session, { id: 'other', harness_session_id: s.scope.nativeSessionId, title: 'Other', state: 'Other stage', remaining: 'Other todo', status: 'blocked' });
  add('other-viewing-session-shared-goal-never-progress', s, 100, 12);
}
for (const status of ['active', 'waiting', 'blocked', 'parked', 'done']) {
  const s = snapshot(); s.session.status = status; add(`hidden-status-${status}`, s, 5, 5);
}
for (const width of [0, 1, 2, 3, 5, 8, 9, 18, 28, 35, 50, 100]) for (const height of [0, 1, 2, 3, 5, 8]) {
  const s = snapshot(); s.stream.documents = Array.from({ length: 28 }, (_, i) => ({ repository: 'lib', ref: '界😀', path: `docs/é-${i}.md`, description: '' }));
  add(`geometry-${width}-${height}`, s, width, height);
}
for (const width of [1, 5, 18, 35, 80]) for (const word of ['界'.repeat(6), 'é'.repeat(8), '👍🏽'.repeat(6), '🇫🇷'.repeat(6), '👩‍💻'.repeat(4), 'a\u0d4e'.repeat(6)]) {
  const s = snapshot(); s.stream.goal = word; s.stream.documents = []; s.stream.issues = []; s.stream.prs = []; s.session.remaining = ''; s.session.state = '';
  add(`unicode-${word}-${width}`, s, width, 30);
}
for (const height of [5, 6, 7, 8, 9]) {
  const s = snapshot(); s.stream.goal = 'Purpose'; s.session.title = 'Assignment'; s.session.state = 'Stage'; s.session.remaining = 'Next work';
  s.stream.documents = [...'abcdefgh'].map(c => ({ repository: 'lib', ref: 'topic', path: `docs/${c}.md`, description: '' })); s.stream.issues = []; s.stream.prs = [];
  add(`quiet-grid-${height}`, s, 50, height);
  add(`notice-grid-${height}`, s, 50, height, 'Notice 界😀é');
}
{
  const s = snapshot(); s.stream.goal = 'goal\n'.repeat(30); s.session.state = 'now\n'.repeat(30); s.session.remaining = 'next\n'.repeat(30);
  s.stream.documents = Array.from({ length: 9 }, (_, i) => ({ repository: 'lib', ref: 'topic', path: `d${i}.md`, description: '' }));
  add('multiline-overflow', s, 35, 5);
}
{
  const s = snapshot(); s.stream.repositories = Object.fromEntries(['lib', 'client', 'same', 'unbound'].map(k => [k, { url: `https://example.com/${k}` }]));
  s.stream.documents = Object.keys(s.stream.repositories).map(repository => ({ repository, ref: 'topic', path: 'docs/design.md', description: '', added_at: 'abcdef0123456789' }));
  s.checkouts = [{ repository: 'lib', checkout_id: 'lib', root: '/backend/common' }, { repository: 'client', checkout_id: 'client', root: '/frontend/common' }, { repository: 'same', checkout_id: 'same', root: '/backend/common' }];
  add('distinguishing-roots-provenance', s, 180, 12);
}
for (const error of ['Document not present in this checkout', 'Ambiguous viewing-session checkout binding', 'No registered checkout']) {
  const s = snapshot(); locations(s); Object.assign(s.documentLocations[0], { file: null, error });
  if (error === 'No registered checkout') { s.checkouts = []; Object.assign(s.documentLocations[0], { checkout_id: null, root: null }); }
  add(error, s, 100, 10);
}
{
  const s = snapshot(); s.stream.issues = [{ url: 'https://github.com/acme/project/issues/42#issuecomment-900', description: 'Duplicate' }, { url: 'https://gitlab.example/team/subgroup/repo/-/issues/456', description: 'Duplicate' }];
  s.stream.prs = [{ url: 'https://github.enterprise.test/team/repo/pull/7/files', description: 'Duplicate' }, { url: 'https://example.com/19?pull=20#21', description: '' }];
  add('duplicate-link-names-order-targets', s, 100, 12);
}
for (const url of ['javascript:alert(1)', 'https://example.com/%0Aevil', 'https://example.com/%E2%80%AEevil', 'https://example.com/%ZZ', 'https://example.com/a\\b', 'https://example.com/a b', 'https://example.com/界', 'https://example.com']) {
  const s = snapshot(); s.stream.issues = [{ url, description: 'Unsafe or canonicalized' }]; s.stream.prs = [];
  add(`url-${url}`, s, 100, 10);
}
add('failed-frame', undefined, 80, 5, 'Card unavailable: Malformed record. Refresh to retry.');
add('refreshing-cached-frame-paint-only', snapshot(), 100, 12, 'Refreshing…');
add('empty-frame', undefined, 5, 1);
writeFileSync(new URL('./active-renderer.json', import.meta.url), JSON.stringify(cases, null, 2) + '\n');
console.log(`Extracted ${cases.length} active renderer fixtures`);
