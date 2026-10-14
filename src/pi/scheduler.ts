import { DEFAULT_CONFIG } from '../config.js';

export interface EvidenceSnapshot {
  revision: string | number;
}

export interface CheckSchedulerOptions<T extends EvidenceSnapshot> {
  check_every_n_responses?: number;
  check_every_n_cycles?: number;
  check: (snapshot: T, signal: AbortSignal) => Promise<void>;
  onError?: (error: unknown, snapshot: T) => void;
}

export interface CheckScheduler<T extends EvidenceSnapshot> {
  onCycle(snapshot: T): void;
  onSettled(snapshot: T, aborted?: boolean): void;
  reset(): void;
}

/** Only main-agent boundaries feed this scheduler; helper calls are not events. */
export function createCheckScheduler<T extends EvidenceSnapshot>(options: CheckSchedulerOptions<T>): CheckScheduler<T> {
  const responsesEvery = options.check_every_n_responses ?? DEFAULT_CONFIG.check_every_n_responses;
  const cyclesEvery = options.check_every_n_cycles ?? DEFAULT_CONFIG.check_every_n_cycles;
  for (const value of [responsesEvery, cyclesEvery]) {
    if (!Number.isSafeInteger(value) || value < 0) {
      throw new Error("Check intervals must be a non-negative safe integer.");
    }
  }
  let generation = 0;
  let responses = 0;
  let cycles = 0;
  let lastAttempted: T["revision"] | undefined;
  let pending: T | undefined;
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
      if (next) request(next);
    }
  }

  function request(snapshot: T): void {
    if (snapshot.revision === lastAttempted) return;
    if (running) {
      pending = snapshot;
      return;
    }
    lastAttempted = snapshot.revision;
    const controller = new AbortController();
    running = controller;
    // Later cycles queue newer evidence, never cancel this check or require idle.
    void run(snapshot, controller, generation);
  }

  return {
    onCycle(snapshot) {
      cycles++;
      if (cyclesEvery > 0 && cycles % cyclesEvery === 0) request(snapshot);
    },
    onSettled(snapshot, aborted = false) {
      cycles = 0;
      if (aborted) return;
      responses++;
      if (responsesEvery > 0 && responses % responsesEvery === 0) request(snapshot);
    },
    reset() {
      generation++;
      responses = 0;
      cycles = 0;
      lastAttempted = undefined;
      pending = undefined;
      // Keep the occupied slot until cancellation finishes: never overlap calls.
      running?.abort();
    },
  };
}
