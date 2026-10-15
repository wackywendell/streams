import { DEFAULT_CONFIG } from '../config.js';

export interface EvidenceSnapshot {
  revision: string | number;
}

export interface CheckSchedulerOptions<T extends EvidenceSnapshot> {
  check_every_n_responses?: number;
  check: (snapshot: T, signal: AbortSignal) => Promise<void>;
  onError?: (error: unknown, snapshot: T) => void;
}

export interface CheckScheduler<T extends EvidenceSnapshot> {
  observe(snapshot: T): void;
  onSettled(snapshot: T, aborted?: boolean): void;
  refresh(snapshot: T): void;
  reset(): void;
}

/** Count changes to the actual summary input, not model/tool event counts. */
export function createCheckScheduler<T extends EvidenceSnapshot>(options: CheckSchedulerOptions<T>): CheckScheduler<T> {
  const responsesEvery = options.check_every_n_responses ?? DEFAULT_CONFIG.check_every_n_responses;
  if (!Number.isSafeInteger(responsesEvery) || responsesEvery < 0) {
    throw new Error('Check intervals must be a non-negative safe integer.');
  }
  let generation = 0;
  let responses = 0;
  let lastObserved: T['revision'] | undefined;
  let lastAttempted: T['revision'] | undefined;
  let pending: { snapshot: T; forced: boolean } | undefined;
  let running: AbortController | undefined;

  async function run(snapshot: T, controller: AbortController, startedGeneration: number): Promise<void> {
    try {
      await options.check(snapshot, controller.signal);
    } catch (error) {
      if (!controller.signal.aborted && generation === startedGeneration) options.onError?.(error, snapshot);
    } finally {
      running = undefined;
      const next = pending;
      pending = undefined;
      if (next) request(next.snapshot, next.forced);
    }
  }

  function request(snapshot: T, forced = false): void {
    if (running) {
      // A newer eligible input replaces queued input, not an explicit retry's intent.
      if (forced || pending || snapshot.revision !== lastAttempted) {
        pending = { snapshot, forced: forced || pending?.forced === true };
      }
      return;
    }
    if (!forced && snapshot.revision === lastAttempted) return;
    lastAttempted = snapshot.revision;
    const controller = new AbortController();
    running = controller;
    void run(snapshot, controller, generation);
  }

  function observe(snapshot: T): void {
    if (snapshot.revision === lastObserved) return;
    lastObserved = snapshot.revision;
    responses++;
    if (responsesEvery > 0 && responses % responsesEvery === 0) request(snapshot);
  }

  return {
    observe,
    onSettled(snapshot, aborted = false) {
      if (!aborted) observe(snapshot);
    },
    refresh(snapshot) {
      lastObserved = snapshot.revision;
      request(snapshot, true);
    },
    reset() {
      generation++;
      responses = 0;
      lastObserved = undefined;
      lastAttempted = undefined;
      pending = undefined;
      // Keep the occupied slot until cancellation finishes: never overlap calls.
      running?.abort();
    },
  };
}
