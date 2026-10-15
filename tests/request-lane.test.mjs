import test from 'node:test';
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { RequestLane } from '../dist/pi/request-lane.js';

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const signal = () => new AbortController().signal;
const registry = () => globalThis[Symbol.for('work-cards.pi.request-lanes.v1')];

for (const settlement of ['resolve', 'reject']) test(`re-evaluated modules share occupancy until late provider ${settlement}`, async () => {
  const { RequestLane: ReloadedLane } = await import(`../dist/pi/request-lane.js?${randomUUID()}`);
  assert.notEqual(RequestLane, ReloadedLane, 'exercise actual module re-evaluation');
  const id = randomUUID(), old = new RequestLane(), replacement = new ReloadedLane();
  const provider = deferred(), processing = deferred();
  const first = old.run(id, signal(), async () => {
    old.provider(() => provider.promise);
    await processing.promise;
  });
  processing.resolve(); await first;
  let started = false;
  const second = replacement.run(id, signal(), async () => { started = true; });
  assert.equal(started, false);
  assert.equal(registry().has(id), true);
  provider[settlement](new Error('late rejection'));
  await second;
  assert.equal(started, true);
  assert.equal(registry().has(id), false, 'settled idle occupancy must be removed');
});

test('cancelled waiters finish promptly without starting or releasing the occupied slot', async () => {
  const id = randomUUID(), lane = new RequestLane(), next = new RequestLane(), provider = deferred();
  await lane.run(id, signal(), async () => { lane.provider(() => provider.promise); });
  const controller = new AbortController(); let starts = 0;
  const queued = next.run(id, controller.signal, async () => { starts++; });
  controller.abort(); await queued;
  await next.run(id, controller.signal, async () => { starts++; });
  assert.equal(starts, 0);
  assert.equal(registry().has(id), true);
  provider.resolve();
  await next.run(id, signal(), async () => { starts++; });
  assert.equal(starts, 1);
  assert.equal(registry().has(id), false);
});

test('unrelated session UUIDs remain independent even within one lane', async () => {
  const id = randomUUID(), otherId = randomUUID(), lane = new RequestLane();
  const provider = deferred(), entered = deferred(), processing = deferred(), otherProvider = deferred();
  const first = lane.run(id, signal(), async () => {
    entered.resolve(); await processing.promise;
    // This continuation must still own the original UUID after another run starts.
    lane.provider(() => provider.promise);
  });
  await entered.promise;
  await lane.run(otherId, signal(), async () => { lane.provider(() => otherProvider.promise); });
  processing.resolve(); await first;
  assert.equal(registry().has(id), true); assert.equal(registry().has(otherId), true);
  otherProvider.resolve();
  await lane.run(otherId, signal(), async () => {});
  assert.equal(registry().has(id), true); assert.equal(registry().has(otherId), false);
  provider.resolve(); await lane.run(id, signal(), async () => {});
  assert.equal(registry().has(id), false);
});

test('whole-check processing retains ownership after its providers settle', async () => {
  const id = randomUUID(), lane = new RequestLane(), other = new RequestLane();
  const providerSettled = deferred(), processing = deferred(); let nextStarted = false;
  const first = lane.run(id, signal(), async () => {
    await lane.provider(async () => {}); providerSettled.resolve(); await processing.promise;
  });
  await providerSettled.promise;
  const second = other.run(id, signal(), async () => { nextStarted = true; });
  assert.equal(nextStarted, false);
  processing.resolve(); await Promise.all([first, second]);
  assert.equal(nextStarted, true); assert.equal(registry().has(id), false);
});

test('providers require a processing owner and cannot overlap within a check', async () => {
  const id = randomUUID(), lane = new RequestLane(), provider = deferred();
  assert.throws(() => lane.provider(async () => {}), /active request owner/);
  await lane.run(id, signal(), async () => {
    lane.provider(() => provider.promise);
    assert.throws(() => lane.provider(async () => {}), /provider.*already.*running/i);
  });
  assert.equal(registry().has(id), true);
  provider.resolve(); await lane.run(id, signal(), async () => {});
  assert.equal(registry().has(id), false);
});
