import { AsyncLocalStorage } from 'node:async_hooks';

// jiti re-evaluates extensions on reload. Only transient occupancy crosses factories.
const registryKey = Symbol.for('work-cards.pi.request-lanes.v1');
const processState = globalThis as typeof globalThis & { [registryKey]?: Map<string, Promise<void>> };
const occupied = processState[registryKey] ??= new Map<string, Promise<void>>();

function waitForSettlement(settled: Promise<void>, signal: AbortSignal): Promise<void> {
  return new Promise(resolve => {
    const done = () => { signal.removeEventListener('abort', done); resolve(); };
    signal.addEventListener('abort', done, { once: true });
    void settled.then(done);
    if (signal.aborted) done();
  });
}

interface RequestOwner {
  providerSettled: Promise<void>;
  processing: boolean;
  providerRunning: boolean;
}

/** Hold a whole check's slot until both processing and physical provider work end. */
export class RequestLane {
  // A cached refresher's registry must track the owner of this invocation, not its first check.
  private readonly current = new AsyncLocalStorage<RequestOwner>();

  async run(sessionId: string, signal: AbortSignal, process: () => Promise<void>): Promise<void> {
    while (!signal.aborted) {
      const prior = occupied.get(sessionId);
      if (prior) { await waitForSettlement(prior, signal); continue; }
      let release!: () => void;
      const settled = new Promise<void>(resolve => { release = resolve; });
      occupied.set(sessionId, settled);
      const owner: RequestOwner = { providerSettled: Promise.resolve(), processing: true, providerRunning: false };
      try {
        await this.current.run(owner, process);
      } finally {
        owner.processing = false;
        // Cancellation ends processing promptly, but cannot release physical occupancy.
        void owner.providerSettled.then(() => {
          occupied.delete(sessionId);
          release();
        });
      }
      return;
    }
  }

  provider<T>(start: () => Promise<T>): Promise<T> {
    const owner = this.current.getStore();
    if (!owner?.processing) throw new Error('Work-card provider requires an active request owner');
    if (owner.providerRunning) throw new Error('Work-card provider is already running');
    const result = start();
    owner.providerRunning = true;
    // Attach rejection handling immediately, including when abortable processing stops waiting.
    const settled = () => { owner.providerRunning = false; };
    owner.providerSettled = result.then(settled, settled);
    return result;
  }
}
