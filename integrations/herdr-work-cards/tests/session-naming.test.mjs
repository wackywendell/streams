import test from 'node:test';
import assert from 'node:assert/strict';
import { SessionManager } from '@earendil-works/pi-coding-agent';
import { projectSessionName, isProjectedSessionNameChange } from '../dist/pi/session-naming.js';

function fixture(manager = SessionManager.inMemory()) {
  let sets = 0;
  const pi = {
    getSessionName: () => manager.getSessionName(),
    setSessionName: name => { sets++; manager.appendSessionInfo(name); },
  };
  const apply = (title = 'Own task', guard = () => {}) => projectSessionName(pi, manager, manager.getSessionId(), title, guard);
  return { manager, pi, apply, sets: () => sets };
}

test('saved titles replace any observed name and unchanged projection has no effect', () => {
  const h = fixture();
  h.manager.appendSessionInfo('Independent frontend name');
  assert.equal(h.apply(), 'renamed');
  assert.equal(h.manager.getSessionName(), 'Own task');
  assert.equal(h.apply(), 'unchanged');
  assert.equal(h.sets(), 1);
  h.manager.appendSessionInfo('Own task');
  assert.equal(h.apply('Updated task'), 'renamed');
  h.manager.appendSessionInfo('');
  assert.equal(h.apply('Updated task'), 'renamed');
  assert.equal(h.manager.getEntries().filter(e => e.type === 'custom').length, 0);
});

test('projection ignores old provenance and does not scan transcript entries', () => {
  const h = fixture();
  h.manager.appendCustomEntry('work-cards.session-naming', { version: 999 });
  h.manager.getEntries = () => { throw Error('must not read transcript'); };
  assert.equal(h.apply(), 'renamed');
  assert.equal(h.apply(), 'unchanged');
});

test('current identity, cancellation and nonempty titles are checked before any effect', () => {
  const h = fixture();
  for (const title of ['', '   ']) assert.throws(() => h.apply(title), /nonempty/i);
  assert.throws(() => h.apply('Task', () => { throw Error('cancelled'); }), /cancelled/);
  assert.throws(() => projectSessionName(h.pi, h.manager, 'other-session', 'Task', () => {}), /session/i);
  assert.equal(h.sets(), 0);
});

test('name writes must be acknowledged by the same live session and guard', () => {
  for (const behavior of ['ignored', 'wrong-name', 'stale-id', 'stale-guard']) {
    const h = fixture();
    let changed = false;
    h.pi.setSessionName = name => {
      changed = true;
      if (behavior === 'stale-id') h.manager.getSessionId = () => 'other-session';
      if (behavior !== 'ignored') h.manager.appendSessionInfo(behavior === 'wrong-name' ? 'Other task' : name);
    };
    assert.throws(() => h.apply('Task', () => {
      if (changed && behavior === 'stale-guard') throw Error('cancelled');
    }), /acknowledged|cancelled/i);
    assert.equal(isProjectedSessionNameChange(), false);
  }
});

test('async event delivery retains projection origin without marking independent edits', async () => {
  const h = fixture(), pending = [], origins = [];
  const set = h.pi.setSessionName;
  let release;
  const delayedHandler = new Promise(resolve => { release = resolve; });
  h.pi.setSessionName = name => {
    set(name);
    pending.push((async () => {
      await delayedHandler;
      origins.push([name, isProjectedSessionNameChange()]);
    })());
  };
  assert.equal(h.apply(), 'renamed');
  assert.equal(isProjectedSessionNameChange(), false);
  h.pi.setSessionName('User /name');
  release();
  await Promise.all(pending);
  assert.deepEqual(origins, [['Own task', true], ['User /name', false]]);
});

test('setter failures leave neither durable provenance nor a leaked write context', () => {
  const h = fixture();
  h.pi.setSessionName = () => {
    assert.equal(isProjectedSessionNameChange(), true);
    throw Error('set failed');
  };
  assert.throws(() => h.apply(), /set failed/);
  assert.equal(isProjectedSessionNameChange(), false);
  assert.equal(h.manager.getEntries().length, 0);
});
