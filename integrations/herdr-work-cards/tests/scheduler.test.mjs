import assert from 'node:assert/strict';
import test from 'node:test';
import { createCheckScheduler } from '../dist/pi/scheduler.js';

const flush = () => new Promise(resolve => setImmediate(resolve));
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const snapshot = revision => ({ revision });

test('default observes distinct input at any boundary', async () => {
  const checked = [];
  const scheduler = createCheckScheduler({ check: async s => { checked.push(s.revision); } });
  scheduler.observe(snapshot(1));
  await flush();
  scheduler.onSettled(snapshot(1));
  scheduler.observe(snapshot(1));
  await flush();
  scheduler.onSettled(snapshot(2));
  await flush();
  assert.deepEqual(checked, [1, 2]);
});

test('interval counts observations not duplicate hooks or last attempts', async () => {
  const checked = [];
  const scheduler = createCheckScheduler({ check_every_n_responses: 2, check: async s => { checked.push(s.revision); } });
  scheduler.observe(snapshot(1));
  scheduler.onSettled(snapshot(1));
  scheduler.observe(snapshot(1));
  await flush();
  assert.deepEqual(checked, []);
  scheduler.observe(snapshot(2));
  await flush();
  scheduler.onSettled(snapshot(2));
  scheduler.onSettled(snapshot(3), true);
  scheduler.observe(snapshot(3));
  await flush();
  assert.deepEqual(checked, [2]);
  scheduler.onSettled(snapshot(4));
  await flush();
  assert.deepEqual(checked, [2, 4]);
});

test('zero disables automatic checks', async () => {
  const checked = [];
  const scheduler = createCheckScheduler({ check_every_n_responses: 0, check: async s => { checked.push(s.revision); } });
  scheduler.observe(snapshot(1));
  scheduler.onSettled(snapshot(2));
  await flush();
  assert.deepEqual(checked, []);
  scheduler.refresh(snapshot(2));
  await flush();
  scheduler.refresh(snapshot(2));
  await flush();
  assert.deepEqual(checked, [2, 2]);
});

test('busy refresh coalesces one forced followup with newest eligible input without cancelling', async () => {
  const first = deferred(), checked = [], signals = [];
  let active = 0, maximum = 0;
  const scheduler = createCheckScheduler({ check: async (s, signal) => {
    checked.push(s.revision); signals.push(signal); maximum = Math.max(maximum, ++active);
    if (checked.length === 1) await first.promise;
    active--;
  } });
  scheduler.observe(snapshot(1));
  scheduler.refresh(snapshot(1));
  scheduler.refresh(snapshot(1));
  scheduler.observe(snapshot(2));
  scheduler.refresh(snapshot(2));
  scheduler.onSettled(snapshot(3));
  assert.deepEqual(checked, [1]);
  assert.equal(signals[0].aborted, false);
  first.resolve();
  await flush();
  assert.deepEqual(checked, [1, 3]);
  assert.equal(maximum, 1);
  scheduler.onSettled(snapshot(3));
  await flush();
  assert.deepEqual(checked, [1, 3]);
});

test('failure is not automatically retried but unchanged forced input retries', async () => {
  const checked = [], errors = [];
  const scheduler = createCheckScheduler({ check: async s => { checked.push(s.revision); throw new Error('offline'); }, onError: (e, s) => errors.push([e.message, s.revision]) });
  scheduler.observe(snapshot(1));
  scheduler.onSettled(snapshot(1));
  await flush();
  scheduler.onSettled(snapshot(1));
  await flush();
  assert.deepEqual(checked, [1]);
  scheduler.refresh(snapshot(1));
  await flush();
  assert.deepEqual(checked, [1, 1]);
  assert.deepEqual(errors, [['offline', 1], ['offline', 1]]);
});

test('reset cancels queued forced work, clears counters and keeps retired call occupied', async () => {
  const old = deferred(), checked = [], signals = [], errors = [];
  const scheduler = createCheckScheduler({ check_every_n_responses: 2, check: async (s, signal) => {
    checked.push(s.revision); signals.push(signal); if (checked.length === 1) await old.promise;
  }, onError: e => errors.push(e) });
  scheduler.observe(snapshot(1)); scheduler.observe(snapshot(2));
  scheduler.refresh(snapshot(3));
  scheduler.reset();
  assert.equal(signals[0].aborted, true);
  scheduler.observe(snapshot(1)); scheduler.onSettled(snapshot(1)); scheduler.observe(snapshot(2));
  assert.deepEqual(checked, [2]);
  old.reject(new Error('cancelled'));
  await flush();
  assert.deepEqual(checked, [2, 2]);
  assert.deepEqual(errors, []);
});

test('passes explicit force separately from snapshots, including newest automatic queued input', async () => {
  const first = deferred(), checked = [], signals = [];
  let active = 0, maximum = 0;
  const scheduler = createCheckScheduler({ check: async (s, signal, forced) => {
    checked.push([s.revision, forced]); signals.push(signal); maximum = Math.max(maximum, ++active);
    if (checked.length === 2) await first.promise;
    active--;
  } });
  scheduler.refresh(snapshot(1)); await flush();
  scheduler.observe(snapshot(2));
  scheduler.refresh(snapshot(2)); scheduler.observe(snapshot(3)); scheduler.observe(snapshot(4));
  assert.equal(signals[1].aborted, false);
  first.resolve(); await flush();
  assert.deepEqual(checked, [[1, 'refresh'], [2, 'automatic'], [4, 'refresh']]);
  assert.equal(maximum, 1);
  scheduler.observe(snapshot(5)); await flush();
  assert.deepEqual(checked.at(-1), [5, 'automatic']);
});

test('reset discards queued force while keeping cancelled work single-flight', async () => {
  const first = deferred(), checked = [], signals = [];
  const scheduler = createCheckScheduler({ check: async (s, signal, forced) => {
    checked.push([s.revision, forced]); signals.push(signal);
    if (checked.length === 1) await first.promise;
  } });
  scheduler.refresh(snapshot(1)); scheduler.refresh(snapshot(2)); scheduler.reset();
  scheduler.observe(snapshot(3));
  assert.equal(signals[0].aborted, true); assert.equal(checked.length, 1);
  first.resolve(); await flush();
  assert.deepEqual(checked, [[1, 'refresh'], [3, 'automatic']]);
});

test('busy reset keeps strongest intent while newer refresh and automatic evidence replaces its input', async () => {
  const first = deferred(), checked = [];
  const scheduler = createCheckScheduler({ check: async (s, signal, intent) => {
    checked.push([s.revision, intent]);
    if (checked.length === 1) await first.promise;
  } });
  scheduler.observe(snapshot(1));
  scheduler.refresh(snapshot(2), 'reset');
  scheduler.refresh(snapshot(3));
  scheduler.observe(snapshot(4));
  first.resolve(); await flush();
  assert.deepEqual(checked, [[1, 'automatic'], [4, 'reset']]);
  scheduler.refresh(snapshot(4), 'reset'); await flush();
  assert.deepEqual(checked.at(-1), [4, 'reset']);
});

test('rejects invalid threshold', () => {
  for (const key of ['check_every_n_responses']) for (const value of [-1, 1.5, NaN, Infinity]) {
    assert.throws(() => createCheckScheduler({ [key]: value, check: async () => {} }), /non-negative safe integer/);
  }
});
