import assert from "node:assert/strict";
import test from "node:test";
import { createCheckScheduler } from "../dist/pi/scheduler.js";

const flush = () => new Promise((resolve) => setImmediate(resolve));
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const snapshot = (revision) => ({ revision, cardRevision: "card-v1" });

test("defaults check each settled response and every ten cycles", async () => {
  const checked = [];
  const scheduler = createCheckScheduler({ check: async (value) => { checked.push(value.revision); } });
  for (let i = 1; i <= 9; i++) scheduler.onCycle(snapshot(i));
  assert.deepEqual(checked, []);
  scheduler.onCycle(snapshot(10));
  await flush();
  scheduler.onSettled(snapshot(10));
  await flush();
  assert.deepEqual(checked, [10]);
  scheduler.onSettled(snapshot(11));
  await flush();
  assert.deepEqual(checked, [10, 11]);
});

test("response and cycle thresholds count independently, cycles reset at every settle", async () => {
  const checked = [];
  const scheduler = createCheckScheduler({
    check_every_n_responses: 2, check_every_n_cycles: 3,
    check: async (value) => { checked.push(value.revision); },
  });
  scheduler.onCycle(snapshot(1));
  scheduler.onCycle(snapshot(2));
  scheduler.onSettled(snapshot(2));
  scheduler.onCycle(snapshot(3));
  scheduler.onCycle(snapshot(4));
  assert.deepEqual(checked, []);
  scheduler.onCycle(snapshot(5));
  await flush();
  scheduler.onSettled(snapshot(5));
  await flush();
  assert.deepEqual(checked, [5]);
  scheduler.onCycle(snapshot(6));
  scheduler.onCycle(snapshot(7));
  scheduler.onSettled(snapshot(7), true);
  scheduler.onCycle(snapshot(8));
  await flush();
  assert.deepEqual(checked, [5]);
  scheduler.onSettled(snapshot(8));
  scheduler.onSettled(snapshot(9));
  await flush();
  assert.deepEqual(checked, [5, 9]);
});

test("zero disables each trigger independently", async () => {
  for (const [responses, cycles, expected] of [[0, 0, []], [0, 1, [1]], [1, 0, [2]]]) {
    const checked = [];
    const scheduler = createCheckScheduler({
      check_every_n_responses: responses, check_every_n_cycles: cycles,
      check: async (value) => { checked.push(value.revision); },
    });
    scheduler.onCycle(snapshot(1));
    await flush();
    scheduler.onSettled(snapshot(2));
    await flush();
    assert.deepEqual(checked, expected);
  }
});

test("continuing cycles do not cancel a running check; newest eligible evidence coalesces", async () => {
  const first = deferred();
  const checked = [], signals = [];
  let active = 0, maximum = 0;
  const scheduler = createCheckScheduler({
    check_every_n_cycles: 2,
    check: async (value, signal) => {
      checked.push(value.revision); signals.push(signal);
      maximum = Math.max(maximum, ++active);
      if (value.revision === 2) await first.promise;
      active--;
    },
  });
  for (let i = 1; i <= 7; i++) scheduler.onCycle(snapshot(i));
  assert.deepEqual(checked, [2]);
  assert.equal(signals[0].aborted, false);
  first.resolve();
  await flush();
  assert.deepEqual(checked, [2, 6]);
  assert.equal(maximum, 1);
  scheduler.onSettled(snapshot(7));
  await flush();
  assert.deepEqual(checked, [2, 6, 7]);
});

test("failure reports once and never retries the same revision at settle", async () => {
  const checked = [], errors = [];
  const scheduler = createCheckScheduler({
    check_every_n_cycles: 1,
    check: async (value) => { checked.push(value.revision); throw new Error("offline"); },
    onError: (error, value) => { errors.push([error.message, value.revision]); },
  });
  scheduler.onCycle(snapshot(1));
  scheduler.onSettled(snapshot(1));
  await flush();
  scheduler.onSettled(snapshot(1));
  await flush();
  assert.deepEqual(checked, [1]);
  assert.deepEqual(errors, [["offline", 1]]);
});

test("reset aborts old work, clears pending/counters, and serializes the next generation", async () => {
  const old = deferred();
  const checked = [], signals = [], errors = [];
  const scheduler = createCheckScheduler({
    check_every_n_cycles: 2, check_every_n_responses: 0,
    check: async (value, signal) => {
      checked.push(value.revision); signals.push(signal);
      if (checked.length === 1) await old.promise;
    },
    onError: (error) => { errors.push(error); },
  });
  for (let i = 1; i <= 4; i++) scheduler.onCycle(snapshot(i));
  scheduler.reset();
  assert.equal(signals[0].aborted, true);
  scheduler.onCycle(snapshot(1));
  scheduler.onCycle(snapshot(2));
  assert.deepEqual(checked, [2]);
  old.reject(new Error("cancelled"));
  await flush();
  assert.deepEqual(checked, [2, 2]);
  assert.deepEqual(errors, []);
});

test("rejects invalid threshold configuration", () => {
  for (const value of [-1, 1.5, NaN, Infinity]) {
    assert.throws(() => createCheckScheduler({ check_every_n_cycles: value, check: async () => {} }), /non-negative safe integer/);
  }
});
