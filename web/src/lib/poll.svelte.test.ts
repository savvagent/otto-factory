/**
 * The six rules in `poll.svelte.ts`, plus the generation guard, each asserted
 * where it would otherwise regress silently: a skeleton that flashes twice a
 * minute, a dashboard wiped out by one failed request, a revoked session
 * whispered as a hiccup, a backgrounded tab polling all weekend, a slow
 * response stacking a second one behind it, an outage met at full rate, an
 * unattended tab holding a session open, and a response from the org you just
 * navigated away from landing on the page.
 *
 * No jsdom and no fake `document`: visibility and activity are injected, so the
 * interesting half is ordinary state and the timing half is `vi.useFakeTimers()`.
 * Every test injects both — a test that let them default would be exercising
 * `documentVisibility`, which in this environment answers "hidden" and polls
 * nothing. `poll.dom.test.ts` covers those defaults under jsdom instead.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
  LOAD_TIMEOUT,
  Poller,
  PollTimeout,
  REFRESH_INTERVAL,
  type Activity,
  type PollOptions,
  type Visibility
} from './poll.svelte';

/** A tab whose visibility and user input the test drives. */
function fakeTab() {
  let hidden = false;
  const watchers = new Set<() => void>();
  const actors = new Set<() => void>();

  const visibility: Visibility = {
    hidden: () => hidden,
    subscribe(onChange) {
      watchers.add(onChange);
      return () => watchers.delete(onChange);
    }
  };
  const activity: Activity = {
    subscribe(onActive) {
      actors.add(onActive);
      return () => actors.delete(onActive);
    }
  };

  return {
    /** Injected into every `start` — jitter fixed so delays are exact. */
    options: { visibility, activity, random: () => 0.5 } satisfies PollOptions,
    visibility,
    listenerCount: () => watchers.size + actors.size,
    hide(next: boolean) {
      hidden = next;
      for (const watcher of [...watchers]) watcher();
    },
    touch() {
      for (const actor of [...actors]) actor();
    }
  };
}

/** Let queued promise callbacks run without advancing the fake clock. */
const settle = () => vi.advanceTimersByTimeAsync(0);

/** A load the test decides the fate of. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (failure: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe('the first load', () => {
  it('is the only one that shows the loading state', async () => {
    const tab = fakeTab();
    const load = vi.fn().mockResolvedValue('first');
    const poller = new Poller<string>();
    const stop = poller.start(load, tab.options);

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

  it('runs even in a hidden tab, so a backgrounded page has something to show', async () => {
    const tab = fakeTab();
    tab.hide(true);
    const load = vi.fn().mockResolvedValue('ready');
    const poller = new Poller<string>();
    const stop = poller.start(load, tab.options);
    await settle();

    expect(load).toHaveBeenCalledTimes(1);
    expect(poller.value).toBe('ready');
    // It is the repeat that is suppressed, not the first load.
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 3);
    expect(load).toHaveBeenCalledTimes(1);

    stop();
  });

  it('reports a failure as failed, because there is nothing to show', async () => {
    const tab = fakeTab();
    const failure = new Error('down');
    const load = vi.fn().mockRejectedValue(failure);
    const poller = new Poller<string>();
    const stop = poller.start(load, tab.options);

    await settle();
    expect(poller.failed).toBe(true);
    expect(poller.error).toBe(failure);
    expect(poller.value).toBeUndefined();
    expect(poller.stale).toBe(false);
    expect(poller.loading).toBe(false);
    expect(poller.stopped).toBe(false);

    // A first load that failed keeps polling, so a page opened during a blip
    // fills itself in rather than waiting for someone to press reload.
    load.mockResolvedValue('recovered');
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 2);
    expect(poller.value).toBe('recovered');
    expect(poller.failed).toBe(false);
    expect(poller.error).toBeUndefined();

    stop();
  });

  it('is failed, not silently empty, when the rejection carries no value', async () => {
    // `failed` is a flag rather than `error !== undefined` precisely so this
    // cannot render as a confident empty dashboard.
    const tab = fakeTab();
    const poller = new Poller<string>();
    const stop = poller.start(vi.fn().mockRejectedValue(undefined), tab.options);
    await settle();

    expect(poller.failed).toBe(true);
    expect(poller.error).toBeUndefined();
    expect(poller.value).toBeUndefined();

    stop();
  });
});

describe('a failed refresh', () => {
  it('keeps the last good value and marks it stale', async () => {
    const tab = fakeTab();
    const load = vi.fn().mockResolvedValue('good');
    const poller = new Poller<string>();
    const stop = poller.start(load, tab.options);
    await settle();
    const updatedAt = poller.updatedAt;

    load.mockRejectedValue(new Error('502'));
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);

    expect(poller.value).toBe('good');
    expect(poller.stale).toBe(true);
    expect(poller.failed).toBe(false);
    expect(poller.failures).toBe(1);
    // The age shown beside the warning has to be the age of the data, not of
    // the attempt that failed to replace it.
    expect(poller.updatedAt).toBe(updatedAt);

    load.mockResolvedValue('better');
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 2);
    expect(poller.value).toBe('better');
    expect(poller.stale).toBe(false);
    expect(poller.failures).toBe(0);

    stop();
  });

  it('backs off while it keeps failing, and recovers the interval on success', async () => {
    const tab = fakeTab();
    const load = vi.fn().mockResolvedValue('good');
    const poller = new Poller<string>();
    const stop = poller.start(load, tab.options);
    await settle();

    load.mockRejectedValue(new Error('502'));
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);
    expect(load).toHaveBeenCalledTimes(2);

    // One failure doubles the gap: nothing at the plain interval...
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);
    expect(load).toHaveBeenCalledTimes(2);
    // ...and the retry at twice it.
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);
    expect(load).toHaveBeenCalledTimes(3);

    // Two failures quadruple it.
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 3);
    expect(load).toHaveBeenCalledTimes(3);
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);
    expect(load).toHaveBeenCalledTimes(4);

    load.mockResolvedValue('good again');
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 8);
    const afterRecovery = load.mock.calls.length;
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);
    expect(load.mock.calls.length).toBe(afterRecovery + 1);

    stop();
  });

  it('stops for good on a failure the caller calls fatal', async () => {
    const tab = fakeTab();
    const revoked = new Error('401');
    const load = vi.fn().mockResolvedValueOnce('good').mockRejectedValue(revoked);
    const fatal = vi.fn((failure: unknown) => failure === revoked);
    const poller = new Poller<string>();
    const stop = poller.start(load, { ...tab.options, fatal });
    await settle();
    expect(poller.value).toBe('good');

    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);

    // The data goes with the credential: a page that kept rendering it under a
    // small warning would be asserting access it no longer has.
    expect(poller.failed).toBe(true);
    expect(poller.stopped).toBe(true);
    expect(poller.value).toBeUndefined();
    expect(poller.stale).toBe(false);
    expect(poller.error).toBe(revoked);

    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 10);
    expect(load).toHaveBeenCalledTimes(2);
    expect(vi.getTimerCount()).toBe(0);
    expect(tab.listenerCount()).toBe(0);

    stop();
  });

  it('treats a load that never answers as a failure rather than a pause', async () => {
    // A hung fetch is the dangerous one: without the timeout the poll is dead
    // and every visible field still says the page is healthy.
    const tab = fakeTab();
    const load = vi
      .fn()
      .mockResolvedValueOnce('good')
      .mockImplementationOnce(() => new Promise<string>(() => {}))
      .mockResolvedValue('back');
    const poller = new Poller<string>();
    const stop = poller.start(load, tab.options);
    await settle();

    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);
    expect(load).toHaveBeenCalledTimes(2);
    expect(poller.stale).toBe(false);

    await vi.advanceTimersByTimeAsync(LOAD_TIMEOUT);
    expect(poller.stale).toBe(true);
    expect(poller.error).toBeInstanceOf(PollTimeout);

    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 2);
    expect(poller.value).toBe('back');
    expect(poller.stale).toBe(false);

    stop();
  });
});

describe('a hidden tab', () => {
  it('does not poll, and refreshes as soon as it is shown again', async () => {
    const tab = fakeTab();
    const load = vi.fn().mockResolvedValue('a');
    const poller = new Poller<string>();
    const stop = poller.start(load, tab.options);
    await settle();
    expect(load).toHaveBeenCalledTimes(1);

    tab.hide(true);
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 5);
    expect(load).toHaveBeenCalledTimes(1);

    tab.hide(false);
    await settle();
    expect(load).toHaveBeenCalledTimes(2);

    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);
    expect(load).toHaveBeenCalledTimes(3);

    stop();
  });

  it('is not polled even if it goes hidden while a refresh is in flight', async () => {
    const tab = fakeTab();
    const first = deferred<string>();
    const load = vi.fn(() => first.promise);
    const poller = new Poller<string>();
    const stop = poller.start(load, tab.options);

    tab.hide(true);
    first.resolve('landed anyway');
    await settle();
    expect(poller.value).toBe('landed anyway');

    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 3);
    expect(load).toHaveBeenCalledTimes(1);

    stop();
  });
});

describe('an unattended tab', () => {
  it('parks itself once nobody has touched it, and any input starts it again', async () => {
    // Rule 6: an open tab that keeps polling slides the server's idle session
    // deadline forward forever, which is the control quietly not applying.
    const tab = fakeTab();
    const load = vi.fn().mockResolvedValue('a');
    const poller = new Poller<string>();
    const stop = poller.start(load, { ...tab.options, idleAfter: REFRESH_INTERVAL * 2 });
    await settle();

    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 2);
    expect(poller.parked).toBe(true);
    const parkedAfter = load.mock.calls.length;

    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 20);
    expect(load).toHaveBeenCalledTimes(parkedAfter);

    tab.touch();
    await settle();
    expect(poller.parked).toBe(false);
    expect(load).toHaveBeenCalledTimes(parkedAfter + 1);

    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);
    expect(load).toHaveBeenCalledTimes(parkedAfter + 2);

    stop();
  });

  it('does not re-phase the interval on every keystroke', async () => {
    const tab = fakeTab();
    const load = vi.fn().mockResolvedValue('a');
    const poller = new Poller<string>();
    const stop = poller.start(load, tab.options);
    await settle();

    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL / 2);
    tab.touch();
    tab.touch();
    await settle();
    expect(load).toHaveBeenCalledTimes(1);

    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL / 2);
    expect(load).toHaveBeenCalledTimes(2);

    stop();
  });
});

describe('overlapping work', () => {
  it('declines a refresh while one is already in flight, and says so', async () => {
    const tab = fakeTab();
    const first = deferred<string>();
    const second = deferred<string>();
    const load = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    const poller = new Poller<string>();
    const stop = poller.start(load, tab.options);

    first.resolve('one');
    await settle();
    expect(load).toHaveBeenCalledTimes(1);

    // A second refresh while the first is outstanding must not issue a request
    // and must not claim it refreshed.
    const inFlight = poller.refresh();
    const declined = await poller.refresh();
    expect(declined).toBe(false);
    expect(load).toHaveBeenCalledTimes(2);

    second.resolve('two');
    expect(await inFlight).toBe(true);
    expect(poller.value).toBe('two');

    stop();
  });

  it('measures the interval from the end of a slow refresh, not its start', async () => {
    const tab = fakeTab();
    const slow = deferred<string>();
    const load = vi
      .fn()
      .mockResolvedValueOnce('first')
      .mockImplementationOnce(() => slow.promise);
    const poller = new Poller<string>();
    const stop = poller.start(load, { ...tab.options, timeout: REFRESH_INTERVAL * 10 });
    await settle();

    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);
    expect(load).toHaveBeenCalledTimes(2);

    // The second call is still outstanding a full interval later; a
    // `setInterval` would have fired a third by now.
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 2);
    expect(load).toHaveBeenCalledTimes(2);

    slow.resolve('slow');
    await settle();
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);
    expect(load).toHaveBeenCalledTimes(3);

    stop();
  });

  it('honours a caller-supplied interval', async () => {
    const tab = fakeTab();
    const load = vi.fn().mockResolvedValue('a');
    const poller = new Poller<string>();
    const stop = poller.start(load, { ...tab.options, interval: 5_000 });
    await settle();

    await vi.advanceTimersByTimeAsync(5_000);
    expect(load).toHaveBeenCalledTimes(2);

    stop();
  });
});

describe('teardown', () => {
  it('stops the timer and unsubscribes', async () => {
    const tab = fakeTab();
    const load = vi.fn().mockResolvedValue('a');
    const poller = new Poller<string>();
    const stop = poller.start(load, tab.options);
    await settle();
    expect(tab.listenerCount()).toBe(2);

    stop();
    expect(tab.listenerCount()).toBe(0);
    // Not merely "no further calls" — the generation guard would give that even
    // with a timer still armed, holding the closure for another interval.
    expect(vi.getTimerCount()).toBe(0);

    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 10);
    expect(load).toHaveBeenCalledTimes(1);
  });

  it('leaves nothing for refresh() to resurrect', async () => {
    const tab = fakeTab();
    const load = vi.fn().mockResolvedValue('a');
    const poller = new Poller<string>();
    const stop = poller.start(load, tab.options);
    await settle();
    stop();

    // `refresh()` passes the *current* generation, so the guard cannot catch
    // this one — dropping the loader is what does.
    expect(await poller.refresh()).toBe(false);
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 5);
    expect(load).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('unsubscribes the replaced subscription when the org changes', async () => {
    const first = fakeTab();
    const second = fakeTab();
    const poller = new Poller<string>();
    poller.start(vi.fn().mockResolvedValue('org a'), first.options);
    const stop = poller.start(vi.fn().mockResolvedValue('org b'), second.options);
    await settle();

    expect(first.listenerCount()).toBe(0);
    expect(second.listenerCount()).toBe(2);

    stop();
  });

  it('is not stopped by a late cleanup from the subscription it replaced', async () => {
    // Svelte can run the old effect's cleanup after the new effect body; an
    // unguarded teardown would stop the poll that had just replaced it and the
    // page would silently never update again.
    const tab = fakeTab();
    const poller = new Poller<string>();
    const stopA = poller.start(vi.fn().mockResolvedValue('org a'), tab.options);
    const loadB = vi.fn().mockResolvedValue('org b');
    const stopB = poller.start(loadB, tab.options);

    stopA();
    await settle();
    expect(loadB).toHaveBeenCalledTimes(1);

    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);
    expect(loadB).toHaveBeenCalledTimes(2);

    stopB();
  });

  it('blanks the previous org before the replacement has anything to show', async () => {
    const tab = fakeTab();
    const poller = new Poller<string>();
    poller.start(vi.fn().mockResolvedValue('org a'), tab.options);
    await settle();
    expect(poller.value).toBe('org a');

    const stop = poller.start(
      vi.fn(() => new Promise<string>(() => {})),
      tab.options
    );
    expect(poller.value).toBeUndefined();
    expect(poller.loading).toBe(true);

    stop();
  });

  it('carries no failure across from the subscription it replaced', async () => {
    const tab = fakeTab();
    const poller = new Poller<string>();
    const load = vi.fn().mockResolvedValueOnce('org a').mockRejectedValue(new Error('502'));
    poller.start(load, tab.options);
    await settle();
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);
    expect(poller.stale).toBe(true);

    const stop = poller.start(
      vi.fn(() => new Promise<string>(() => {})),
      tab.options
    );
    expect(poller.stale).toBe(false);
    expect(poller.failed).toBe(false);
    expect(poller.error).toBeUndefined();
    expect(poller.failures).toBe(0);

    stop();
  });

  it('drops a response belonging to the subscription it replaced', async () => {
    const tab = fakeTab();
    const slow = deferred<string>();
    const poller = new Poller<string>();
    poller.start(
      vi.fn(() => slow.promise),
      tab.options
    );

    const stop = poller.start(vi.fn().mockResolvedValue('org b'), tab.options);
    slow.resolve('org a');
    await settle();

    expect(poller.value).toBe('org b');

    stop();
  });

  it('drops a rejection belonging to the subscription it replaced', async () => {
    // The likelier half: switching orgs usually aborts the old request, so the
    // superseded generation rejects rather than resolves.
    const tab = fakeTab();
    const aborted = deferred<string>();
    const poller = new Poller<string>();
    poller.start(
      vi.fn(() => aborted.promise),
      tab.options
    );

    const stop = poller.start(vi.fn().mockResolvedValue('org b'), tab.options);
    aborted.reject(new Error('org a request aborted'));
    await settle();

    expect(poller.value).toBe('org b');
    expect(poller.failed).toBe(false);
    expect(poller.stale).toBe(false);
    expect(poller.error).toBeUndefined();

    stop();
  });

  it('starts the replacement even when the old fetch is still outstanding', async () => {
    const tab = fakeTab();
    const poller = new Poller<string>();
    poller.start(
      vi.fn(() => new Promise<string>(() => {})),
      tab.options
    );

    const load = vi.fn().mockResolvedValue('org b');
    const stop = poller.start(load, tab.options);
    await settle();

    expect(load).toHaveBeenCalledTimes(1);
    expect(poller.value).toBe('org b');
    expect(poller.loading).toBe(false);

    stop();
  });
});
