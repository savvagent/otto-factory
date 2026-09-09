/**
 * The four rules in `poll.svelte.ts`, each asserted where it would otherwise
 * regress silently: a skeleton that flashes twice a minute, a dashboard wiped
 * out by one failed request, a backgrounded tab polling all weekend, and a
 * response from the org you just navigated away from landing on the page.
 *
 * No jsdom and no fake `document`: visibility is injected, so the interesting
 * half is ordinary state, and the timing half is `vi.useFakeTimers()`.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { Poller, REFRESH_INTERVAL, type Visibility } from './poll.svelte';

/** A tab whose visibility the test controls. */
function fakeVisibility() {
  let hidden = false;
  const listeners = new Set<() => void>();
  const visibility: Visibility = {
    hidden: () => hidden,
    subscribe(onChange) {
      listeners.add(onChange);
      return () => listeners.delete(onChange);
    }
  };
  return {
    visibility,
    listenerCount: () => listeners.size,
    set(next: boolean) {
      hidden = next;
      for (const listener of [...listeners]) listener();
    }
  };
}

/** Let queued promise callbacks run without advancing the fake clock. */
const settle = () => vi.advanceTimersByTimeAsync(0);

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe('the first load', () => {
  it('is the only one that shows the loading state', async () => {
    const load = vi.fn().mockResolvedValue('first');
    const poller = new Poller<string>();
    const stop = poller.start(load);

    expect(poller.loading).toBe(true);
    await settle();
    expect(poller.loading).toBe(false);
    expect(poller.value).toBe('first');

    // The refresh is what a skeleton flash would come from: `loading` has to
    // stay false across it, or the page blanks itself twice a minute.
    load.mockResolvedValue('second');
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);
    expect(poller.loading).toBe(false);
    expect(poller.value).toBe('second');

    stop();
  });

  it('reports a failure as an error, because there is nothing to show', async () => {
    const failure = new Error('down');
    const load = vi.fn().mockRejectedValue(failure);
    const poller = new Poller<string>();
    const stop = poller.start(load);

    await settle();
    expect(poller.error).toBe(failure);
    expect(poller.value).toBeUndefined();
    expect(poller.stale).toBe(false);
    expect(poller.loading).toBe(false);

    // A first load that failed keeps polling, so a page opened during a blip
    // fills itself in rather than waiting for someone to press reload.
    load.mockResolvedValue('recovered');
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);
    expect(poller.value).toBe('recovered');
    expect(poller.error).toBeUndefined();

    stop();
  });
});

describe('a failed refresh', () => {
  it('keeps the last good value and marks it stale', async () => {
    const load = vi.fn().mockResolvedValue('good');
    const poller = new Poller<string>();
    const stop = poller.start(load);
    await settle();

    load.mockRejectedValue(new Error('502'));
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);

    expect(poller.value).toBe('good');
    expect(poller.stale).toBe(true);
    expect(poller.error).toBeUndefined();

    // And it recovers on its own: the next tick clears the marker.
    load.mockResolvedValue('better');
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);
    expect(poller.value).toBe('better');
    expect(poller.stale).toBe(false);

    stop();
  });
});

describe('a hidden tab', () => {
  it('does not poll, and refreshes as soon as it is shown again', async () => {
    const tab = fakeVisibility();
    const load = vi.fn().mockResolvedValue('a');
    const poller = new Poller<string>();
    const stop = poller.start(load, { visibility: tab.visibility });
    await settle();
    expect(load).toHaveBeenCalledTimes(1);

    tab.set(true);
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 5);
    expect(load).toHaveBeenCalledTimes(1);

    tab.set(false);
    await settle();
    expect(load).toHaveBeenCalledTimes(2);

    // And the ordinary interval resumes from there rather than staying stopped.
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);
    expect(load).toHaveBeenCalledTimes(3);

    stop();
  });

  it('is not polled even if it goes hidden while a refresh is in flight', async () => {
    const tab = fakeVisibility();
    let release: (value: string) => void = () => {};
    const load = vi.fn(() => new Promise<string>((resolve) => (release = resolve)));
    const poller = new Poller<string>();
    const stop = poller.start(load, { visibility: tab.visibility });

    tab.set(true);
    release('landed anyway');
    await settle();
    expect(poller.value).toBe('landed anyway');

    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 3);
    expect(load).toHaveBeenCalledTimes(1);

    stop();
  });
});

describe('overlapping work', () => {
  it('ignores a refresh while one is already in flight', async () => {
    let release: (value: string) => void = () => {};
    const load = vi.fn(() => new Promise<string>((resolve) => (release = resolve)));
    const poller = new Poller<string>();
    const stop = poller.start(load);

    void poller.refresh();
    void poller.refresh();
    expect(load).toHaveBeenCalledTimes(1);

    release('once');
    await settle();
    expect(poller.value).toBe('once');

    stop();
  });

  it('measures the interval from the end of a slow refresh, not its start', async () => {
    let release: (value: string) => void = () => {};
    const load = vi
      .fn()
      .mockResolvedValueOnce('first')
      .mockImplementationOnce(() => new Promise<string>((resolve) => (release = resolve)));
    const poller = new Poller<string>();
    const stop = poller.start(load);
    await settle();

    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);
    expect(load).toHaveBeenCalledTimes(2);

    // The second call is still outstanding a full interval later; a
    // `setInterval` would have fired a third by now.
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 2);
    expect(load).toHaveBeenCalledTimes(2);

    release('slow');
    await settle();
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);
    expect(load).toHaveBeenCalledTimes(3);

    stop();
  });
});

describe('teardown', () => {
  it('stops the timer and unsubscribes', async () => {
    const tab = fakeVisibility();
    const load = vi.fn().mockResolvedValue('a');
    const poller = new Poller<string>();
    const stop = poller.start(load, { visibility: tab.visibility });
    await settle();
    expect(tab.listenerCount()).toBe(1);

    stop();
    expect(tab.listenerCount()).toBe(0);
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 10);
    expect(load).toHaveBeenCalledTimes(1);
  });

  it('drops a response belonging to the subscription it replaced', async () => {
    let release: (value: string) => void = () => {};
    const slow = vi.fn(() => new Promise<string>((resolve) => (release = resolve)));
    const poller = new Poller<string>();
    poller.start(slow);

    // The org changed: a second `start` supersedes the first, and the first
    // org's response must not render under the second org's heading.
    const stop = poller.start(vi.fn().mockResolvedValue('org b'));
    release('org a');
    await settle();

    expect(poller.value).toBe('org b');

    stop();
  });

  it('starts the replacement even when the old fetch is still outstanding', async () => {
    const poller = new Poller<string>();
    poller.start(vi.fn(() => new Promise<string>(() => {})));

    const load = vi.fn().mockResolvedValue('org b');
    const stop = poller.start(load);
    await settle();

    expect(load).toHaveBeenCalledTimes(1);
    expect(poller.value).toBe('org b');
    expect(poller.loading).toBe(false);

    stop();
  });
});
