import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { SessionManager } from '@earendil-works/pi-coding-agent';
import { projectSessionName, sessionNamingEntry, acceptSessionTitle, sessionTitleReady } from '../dist/pi/session-naming.js';

function fixture(manager = SessionManager.inMemory()) {
  let sets = 0, writes = 0;
  const pi = {
    getSessionName: () => manager.getSessionName(),
    setSessionName: name => { sets++; manager.appendSessionInfo(name); },
    appendEntry: (type, data) => { writes++; manager.appendCustomEntry(type, data); },
  };
  const apply = (title = 'Own task', options = {}) => projectSessionName(pi, manager, manager.getSessionId(), title, options.titleMode ?? 'manual', options.guard ?? (() => {}));
  return { manager, pi, apply, sets: () => sets, writes: () => writes };
}

test('own title names an empty Pi session, tracks changes and does not append unchanged effects', () => {
  const h = fixture();
  assert.equal(h.apply(), 'renamed'); assert.equal(h.manager.getSessionName(), 'Own task');
  assert.equal(h.apply(), 'unchanged'); assert.equal(h.sets(), 1); assert.equal(h.writes(), 1);
  assert.equal(h.apply('Updated own task'), 'renamed'); assert.equal(h.manager.getSessionName(), 'Updated own task');
  assert.equal(h.writes(), 2);
});

test('unproven existing names and later external renames remain manually protected, even after clearing', () => {
  for (const existing of ['Manual name', 'Own task']) {
    const h = fixture(); h.manager.appendSessionInfo(existing);
    assert.equal(h.apply(), 'manual'); assert.equal(h.sets(), 0);
    h.manager.appendSessionInfo(''); h.apply('New task'); assert.equal(h.manager.getSessionName(), undefined);
    assert.equal(h.writes(), 1);
  }
  const h = fixture(); h.apply(); h.manager.appendSessionInfo('External /name');
  assert.equal(h.apply('Changed title'), 'manual');
  h.manager.appendSessionInfo('Own task'); h.apply('Another title');
  assert.equal(h.manager.getSessionName(), 'Own task'); assert.equal(h.sets(), 1);
});

test('same-text manual session-info writes revoke automatic authority', () => {
  const h = fixture(); h.apply(); h.manager.appendSessionInfo('Own task');
  assert.equal(h.apply('New task'), 'manual'); assert.equal(h.manager.getSessionName(), 'Own task');
  assert.equal(h.sets(), 1);
});

test('same-session reload and tree navigation retain authority, copied fork entries do not', () => {
  const h = fixture(); h.apply();
  const reload = fixture(h.manager); assert.equal(reload.apply('Reload title'), 'renamed');
  h.manager.resetLeaf(); assert.equal(reload.apply('Tree title'), 'renamed');
  const fork = SessionManager.inMemory();
  for (const entry of h.manager.getEntries()) {
    if (entry.type === 'custom') fork.appendCustomEntry(entry.customType, entry.data);
    if (entry.type === 'session_info') fork.appendSessionInfo(entry.name);
  }
  const other = fixture(fork); assert.equal(other.apply('Fork task'), 'manual'); assert.equal(other.sets(), 0);
});

test('real persisted session resume recovers accepted source and name authority; SDK fork does not', async t => {
  const directory = await mkdtemp(join(tmpdir(), 'wc-session-naming-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const manager = SessionManager.create(directory, directory);
  manager.appendMessage({ role: 'user', content: 'Assignment evidence', timestamp: Date.now() });
  const h = fixture(manager), id = manager.getSessionId();
  acceptSessionTitle(h.pi, manager, id, 'Accepted task', () => {});
  h.apply('Accepted task', { titleMode: 'auto' });
  const resumed = fixture(SessionManager.open(manager.getSessionFile()));
  assert.equal(resumed.manager.getSessionId(), id);
  assert.equal(sessionTitleReady(resumed.manager, id, 'Accepted task', 'auto'), true);
  assert.equal(resumed.apply('Accepted task', { titleMode: 'auto' }), 'unchanged');
  acceptSessionTitle(resumed.pi, resumed.manager, id, 'New accepted title', () => {});
  assert.equal(resumed.apply('New accepted title', { titleMode: 'auto' }), 'renamed');
  resumed.manager.createBranchedSession(resumed.manager.getLeafId());
  assert.notEqual(resumed.manager.getSessionId(), id);
  assert.equal(sessionTitleReady(resumed.manager, resumed.manager.getSessionId(), 'New accepted title', 'auto'), false);
  assert.equal(resumed.apply('Fork title'), 'manual');
});

test('malformed latest provenance cannot revive old automatic authority', () => {
  for (const data of [null, {}, { version: 1, mode: 'auto', name: 'Own task' }, { version: 99, sessionId: 'invalid', mode: 'auto', name: 'Own task' }]) {
    const h = fixture(); h.apply(); h.manager.appendCustomEntry(sessionNamingEntry, data);
    assert.throws(() => h.apply('Other title'), /malformed.*suspended/i); assert.equal(h.sets(), 1);
  }
});

test('impossible naming states suspend rather than acquire source or overwrite authority', () => {
  const h = fixture(), sessionId = h.manager.getSessionId();
  for (const fields of [
    { mode: 'auto', sessionInfoId: 'info' },
    { mode: 'auto', name: 'Task' },
    { acceptedTitle: 'Task', name: 'Applied name' },
    { acceptedTitle: 'Task', sessionInfoId: 'info' },
    { mode: 'manual', name: '', sessionInfoId: 123 },
  ]) {
    h.manager.appendCustomEntry(sessionNamingEntry, { version: 1, sessionId, ...fields });
    assert.throws(() => acceptSessionTitle(h.pi, h.manager, sessionId, 'Accepted task', () => {}), /malformed.*suspended/i);
    assert.throws(() => h.apply('Task'), /malformed.*suspended/i);
  }
  assert.equal(h.sets(), 0); assert.equal(h.writes(), 0);
});

test('accepting a title preserves valid manual protection fields', () => {
  const h = fixture(), sessionId = h.manager.getSessionId();
  const manual = { version: 1, sessionId, mode: 'manual', name: '', sessionInfoId: 'previous-auto-info' };
  h.manager.appendCustomEntry(sessionNamingEntry, manual);
  acceptSessionTitle(h.pi, h.manager, sessionId, 'Accepted task', () => {});
  assert.deepEqual(h.manager.getEntries().at(-1).data, { ...manual, acceptedTitle: 'Accepted task' });
  assert.equal(h.apply('Accepted task', { titleMode: 'auto' }), 'manual');
});

test('accepting a title cannot erase malformed state or a manually cleared Pi name', () => {
  const h = fixture(), id = h.manager.getSessionId();
  h.manager.appendSessionInfo('My manual name'); assert.equal(h.apply('Task'), 'manual');
  h.manager.appendSessionInfo('');
  h.manager.appendCustomEntry(sessionNamingEntry, { version: 999, sessionId: id });
  const entries = h.manager.getEntries().length;
  assert.throws(() => acceptSessionTitle(h.pi, h.manager, id, 'Accepted task', () => {}), /malformed.*suspended/i);
  assert.throws(() => h.apply('Accepted task', { titleMode: 'auto' }), /malformed.*suspended/i);
  assert.equal(h.manager.getEntries().length, entries); assert.equal(h.sets(), 0);
  assert.equal(h.manager.getSessionName(), undefined);
});

test('valid foreign provenance can accept current sources but never inherits overwrite rights or loses empty manual protection', () => {
  for (const mode of ['auto', 'manual', undefined]) {
    const h = fixture(), id = h.manager.getSessionId();
    h.manager.appendSessionInfo('');
    h.manager.appendCustomEntry(sessionNamingEntry, { version: 1, sessionId: 'parent-session', acceptedTitle: 'Parent task',
      ...(mode ? { mode, name: '', ...(mode === 'auto' ? { sessionInfoId: 'parent-info' } : {}) } : {}) });
    acceptSessionTitle(h.pi, h.manager, id, 'Current task', () => {});
    assert.equal(sessionTitleReady(h.manager, id, 'Current task', 'auto'), true);
    assert.equal(h.apply('Current task', { titleMode: 'auto' }), 'manual');
    assert.equal(h.manager.getSessionName(), undefined); assert.equal(h.sets(), 0);
  }
});

test('blank and automatic bootstrap placeholder titles have no effects; explicit own titles are allowed', () => {
  const h = fixture();
  assert.equal(h.apply(''), 'skipped'); assert.equal(h.apply('Untitled task', { titleMode: 'auto' }), 'skipped');
  assert.equal(h.sets(), 0); assert.equal(h.writes(), 0);
  assert.equal(h.apply('Untitled task', { titleMode: 'manual' }), 'renamed');
});

test('automatic source readiness requires a published exact own title and survives manual Pi protection', () => {
  const h = fixture(), id = h.manager.getSessionId();
  assert.equal(sessionTitleReady(h.manager, id, 'Seed', 'auto'), false);
  assert.equal(h.apply('Seed', { titleMode: 'auto' }), 'skipped');
  acceptSessionTitle(h.pi, h.manager, id, 'Seed', () => {});
  assert.equal(sessionTitleReady(h.manager, id, 'Seed', 'auto'), true);
  assert.equal(sessionTitleReady(h.manager, id, 'Other', 'auto'), false);
  assert.equal(h.apply('Seed', { titleMode: 'auto' }), 'renamed');
  h.manager.appendSessionInfo('Manual'); h.apply('Seed', { titleMode: 'auto' });
  acceptSessionTitle(h.pi, h.manager, id, 'New accepted title', () => {});
  assert.equal(sessionTitleReady(h.manager, id, 'New accepted title', 'auto'), true);
  assert.equal(h.apply('New accepted title', { titleMode: 'auto' }), 'manual');
  assert.equal(fixture(h.manager).apply('New accepted title', { titleMode: 'auto' }), 'manual');
  assert.equal(sessionTitleReady(h.manager, 'fork-id', 'New accepted title', 'auto'), false);
});

test('identity and synchronous guard changes prevent names or provenance', () => {
  const h = fixture();
  assert.throws(() => h.apply('Task', { guard: () => { throw new Error('cancelled'); } }), /cancelled/);
  assert.throws(() => projectSessionName(h.pi, h.manager, 'other-session', 'Task', 'manual', () => {}), /session/i);
  assert.equal(h.sets(), 0); assert.equal(h.writes(), 0);
});

test('failed name application never persists auto authority; failed persistence cannot reclaim the applied name', () => {
  const h = fixture(); const original = h.pi.setSessionName;
  h.pi.setSessionName = () => { throw new Error('set failed'); };
  assert.throws(() => h.apply(), /set failed/); assert.equal(h.writes(), 0);
  h.pi.setSessionName = original;
  const append = h.pi.appendEntry; h.pi.appendEntry = () => { throw new Error('persist failed'); };
  assert.throws(() => h.apply(), /persist failed/); assert.equal(h.manager.getSessionName(), 'Own task');
  h.pi.appendEntry = append;
  assert.equal(h.apply('Retry title'), 'manual'); assert.equal(h.manager.getSessionName(), 'Own task');
});

test('a failed persistence after an automatic title change does not restore previous authority', () => {
  const h = fixture(); h.apply(); const append = h.pi.appendEntry;
  h.pi.appendEntry = () => { throw new Error('persist failed'); };
  assert.throws(() => h.apply('New title'), /persist failed/);
  h.pi.appendEntry = append; assert.equal(h.apply('Third title'), 'manual');
  assert.equal(h.manager.getSessionName(), 'New title');
});
