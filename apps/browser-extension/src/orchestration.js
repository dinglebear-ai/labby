/**
 * Coalesces overlapping full scans while guaranteeing that a request arriving
 * during a scan causes exactly one follow-up pass.
 */
export class ScanScheduler {
  /** @param {() => Promise<void>} scan */
  constructor(scan) {
    this.scan = scan;
    /** @type {Promise<void> | undefined} */
    this.pending = undefined;
    this.again = false;
  }

  run() {
    if (this.pending) {
      this.again = true;
      return this.pending;
    }
    this.pending = this.#drain().finally(() => { this.pending = undefined; });
    return this.pending;
  }

  async #drain() {
    let firstError;
    do {
      this.again = false;
      try {
        await this.scan();
      } catch (error) {
        firstError ??= error;
      }
    } while (this.again);
    if (firstError !== undefined) throw firstError;
  }
}

/** @param {boolean} paused @param {boolean} permissionGranted */
export function executionAllowed(paused, permissionGranted) {
  return !paused && permissionGranted;
}

/**
 * Publish first and only expose the observation locally if the scan is still
 * current. A failed publish therefore cannot look like a successful discovery.
 * @template T
 * @param {number} generation
 * @param {() => number | undefined} currentGeneration
 * @param {() => Promise<unknown>} publish
 * @param {() => T} commit
 * @returns {Promise<T | undefined>}
 */
export async function publishCurrentObservation(generation, currentGeneration, publish, commit) {
  await publish();
  return currentGeneration() === generation ? commit() : undefined;
}

/**
 * Attempt every close when scanning is paused and return every failure so the
 * caller can log and repair with a later resync.
 * @param {Iterable<number>} tabIds
 * @param {(tabId: number) => Promise<void>} close
 */
export function closeObservations(tabIds, close) {
  return Promise.allSettled([...tabIds].map(close));
}
