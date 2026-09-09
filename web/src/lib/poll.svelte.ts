/**
 * A page that keeps itself current.
 *
 * The overview describes a queue that changes while nobody is touching the
 * browser — agents claim jobs over MCP, leases expire, counters move — and it
 * is also the page most likely to be left open on a second monitor. That is the
 * bad combination: the numbers on screen are from whenever the tab was opened,
 * and nothing on the page admits it.
 *
 * This is the timing half of fixing that, kept out of the page because the
 * rules below are each easy to leave out on their own, and because the queue
 * and repos pages have the same problem and should not re-derive them.
 *
 * Four rules, each of which is a way a polling page becomes worse than a static
 * one:
 *
 * - **A refresh is not a load.** `loading` is true only while the *first*
 *   result for a subscription is outstanding. Flipping it on every tick
 *   replaces a working page with a skeleton twice a minute, which is the
 *   opposite of the point — the numbers are supposed to change in place.
 * - **A failed refresh keeps the last good result.** `error` means "there is
 *   nothing to show"; a failure on top of data sets `stale` and leaves the data
 *   alone. Throwing away a working dashboard because of one 502 is worse than
 *   being thirty seconds behind.
 * - **A hidden tab does not poll**, and refreshes the moment it is shown again.
 *   A backgrounded tab polling all weekend is pure waste, and the first thing
 *   wanted on return is current data rather than up to a full interval of stale
 *   data.
 * - **Refreshes never overlap.** The interval is measured from the end of one
 *   refresh to the start of the next — a chained `setTimeout`, not a
 *   `setInterval` — so a slow response delays the next tick instead of stacking
 *   behind it. `refresh()` while one is in flight is a no-op for the same
 *   reason.
 *
 * There is deliberately no "updated 12 seconds ago" chrome. Keeping such a
 * label honest needs a second timer running at human resolution, which is a
 * lot of machinery to say "nothing has gone wrong"; the page says something
 * only when it has something to say, which is when a refresh has failed.
 *
 * Nothing here is billable. Metering counts MCP tool calls (`of-billing`), and
 * every request this makes is a console `GET` — the interval is a courtesy to
 * the server, not a cost control.
 */

/** Thirty seconds: fast enough to feel live for jobs that run for minutes. */
export const REFRESH_INTERVAL = 30_000;

/**
 * Whether anyone is looking, injected rather than read from `document`.
 *
 * The same shape as `LocaleStore` in `locale.ts` and for the same reason: it
 * makes the interesting behaviour — pause while hidden, refresh on return —
 * testable without a browser or a stubbed global.
 */
export interface Visibility {
  hidden(): boolean;
  /** Subscribe to changes; the returned function unsubscribes. */
  subscribe(onChange: () => void): () => void;
}

export const documentVisibility: Visibility = {
  hidden: () => typeof document !== 'undefined' && document.visibilityState === 'hidden',
  subscribe(onChange) {
    if (typeof document === 'undefined') return () => {};
    document.addEventListener('visibilitychange', onChange);
    return () => document.removeEventListener('visibilitychange', onChange);
  }
};

export interface PollOptions {
  /** Milliseconds between the end of one refresh and the start of the next. */
  interval?: number;
  visibility?: Visibility;
}

export class Poller<T> {
  /** The last successful result, or `undefined` before the first one lands. */
  value = $state<T | undefined>(undefined);
  /** True only while the first result of the current subscription is in flight. */
  loading = $state(false);
  /** Set only when there is nothing to render; a failed refresh sets `stale`. */
  error = $state<unknown>(undefined);
  /** The last refresh failed, and `value` is whatever worked before it. */
  stale = $state(false);

  #load: (() => Promise<T>) | undefined;
  #interval = REFRESH_INTERVAL;
  #visibility: Visibility = documentVisibility;
  #timer: ReturnType<typeof setTimeout> | undefined;
  #unsubscribe: (() => void) | undefined;
  #inFlight = false;

  /**
   * Which subscription a result belongs to.
   *
   * Bumped by `start` and by teardown, so a response that arrives after the org
   * changed is dropped instead of rendered under the new org's heading — the
   * same hazard `o/[org]/+layout.svelte` guards by comparing slugs, expressed
   * once here where every caller gets it.
   */
  #generation = 0;

  /**
   * Poll `load` until the returned teardown is called.
   *
   * Written to be the whole body of an `$effect`, which is what ties the
   * subscription's lifetime to the page and to the org it is about:
   *
   * ```ts
   * $effect(() => {
   *   const slug = org.slug;
   *   if (!slug) return;
   *   return overview.start(() => load(slug));
   * });
   * ```
   */
  start(load: () => Promise<T>, options: PollOptions = {}): () => void {
    this.#stop();

    this.#load = load;
    this.#interval = options.interval ?? REFRESH_INTERVAL;
    this.#visibility = options.visibility ?? documentVisibility;

    const generation = this.#generation;
    this.value = undefined;
    this.error = undefined;
    this.stale = false;
    this.loading = true;

    this.#unsubscribe = this.#visibility.subscribe(() => {
      if (generation !== this.#generation) return;
      if (this.#visibility.hidden()) this.#clearTimer();
      else void this.#run(generation);
    });

    void this.#run(generation);

    // Guarded, because an `$effect` cleanup that fired after a later `start`
    // would otherwise stop the subscription that replaced it.
    return () => {
      if (generation === this.#generation) this.#stop();
    };
  }

  /** Refresh now. A no-op while a refresh is already in flight. */
  async refresh(): Promise<void> {
    await this.#run(this.#generation);
  }

  async #run(generation: number): Promise<void> {
    const load = this.#load;
    if (!load || this.#inFlight || generation !== this.#generation) return;

    this.#inFlight = true;
    this.#clearTimer();

    try {
      const value = await load();
      if (generation !== this.#generation) return;
      this.value = value;
      this.error = undefined;
      this.stale = false;
    } catch (failure) {
      if (generation !== this.#generation) return;
      if (this.value === undefined) this.error = failure;
      else this.stale = true;
    } finally {
      // A superseded subscription owns none of this state any more: `#stop`
      // has already cleared `#inFlight` for whoever replaced it, and setting
      // `loading` here would report the old org's fetch as the new org's.
      if (generation === this.#generation) {
        this.loading = false;
        this.#inFlight = false;
        this.#schedule(generation);
      }
    }
  }

  #schedule(generation: number): void {
    this.#clearTimer();
    if (this.#visibility.hidden()) return;
    this.#timer = setTimeout(() => void this.#run(generation), this.#interval);
  }

  #clearTimer(): void {
    if (this.#timer !== undefined) clearTimeout(this.#timer);
    this.#timer = undefined;
  }

  #stop(): void {
    this.#clearTimer();
    this.#unsubscribe?.();
    this.#unsubscribe = undefined;
    // Anything in flight belongs to the generation being abandoned; bumping
    // here is what makes its result unusable rather than merely unwanted.
    this.#generation += 1;
    this.#inFlight = false;
  }
}
