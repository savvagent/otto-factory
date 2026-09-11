// @vitest-environment jsdom
/**
 * The regression this file guards: `setFilter` and "Clear filters" must
 * navigate through `goto`, never bare `replaceState` from `$app/navigation`.
 * `replaceState` updates `history` and the address bar but never the
 * reactive `page.url` this page's filters read (see the module doc comment
 * in `+page.svelte`) — a future edit that reintroduces it would leave every
 * filter looking like it worked while the table never re-fetches.
 *
 * `$app/navigation` is mocked rather than exercised for real: this harness
 * `mount()`s `+page.svelte` directly, without SvelteKit's router ever
 * starting, and the real `goto`/`replaceState` throw ("before router is
 * initialized") in that situation. Mocking the module sidesteps the router
 * entirely and lets the test assert on *which* navigation primitive the
 * component called and with what arguments — the actual defect — without
 * needing a live router. What it cannot prove is that a real navigation
 * would re-render the table; that needs a browser, not this harness.
 *
 * Svelte 5 delegates `change` at the mount root rather than attaching a
 * listener directly to each element, so a synthetic event must be dispatched
 * with `{ bubbles: true }` or the delegated listener never sees it.
 */

import { mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { REFRESH_INTERVAL } from '$lib/poll.svelte';

const { gotoMock, replaceStateMock } = vi.hoisted(() => ({
  gotoMock: vi.fn((_url: URL | string, _opts?: Record<string, unknown>) => Promise.resolve()),
  replaceStateMock: vi.fn((_url: URL | string, _state: unknown) => undefined)
}));

vi.mock('$app/navigation', () => ({
  goto: gotoMock,
  replaceState: replaceStateMock
}));

import { page } from '$app/state';
import Harness from './QueueHarness.svelte';

let container: HTMLElement;
let urlDescriptor: PropertyDescriptor | undefined;

beforeEach(() => {
  vi.useFakeTimers();
  container = document.createElement('div');
  document.body.appendChild(container);
  vi.stubGlobal(
    'fetch',
    vi.fn(() =>
      Promise.resolve(new Response('[]', { headers: { 'content-type': 'application/json' } }))
    )
  );
  urlDescriptor = Object.getOwnPropertyDescriptor(page, 'url');
  gotoMock.mockClear();
  replaceStateMock.mockClear();
});

afterEach(() => {
  container.remove();
  vi.unstubAllGlobals();
  vi.useRealTimers();
  if (urlDescriptor) Object.defineProperty(page, 'url', urlDescriptor);
});

/** Let the page's effects and their fetches settle. */
const settle = () => vi.advanceTimersByTimeAsync(0);

describe('the queue filters', () => {
  it('changing Status navigates through goto, not replaceState', async () => {
    const instance = mount(Harness, { target: container, props: { slug: 'acme' } });
    await settle();

    const select = container.querySelector('select') as HTMLSelectElement;
    expect(select).not.toBeNull();

    select.value = 'pending';
    select.dispatchEvent(new Event('change', { bubbles: true }));
    await settle();

    expect(gotoMock).toHaveBeenCalledTimes(1);
    const [url, opts] = gotoMock.mock.calls[0]!;
    expect(String(url)).toContain('status=pending');
    expect(opts).toMatchObject({ replaceState: true, keepFocus: true, noScroll: true });
    expect(replaceStateMock).not.toHaveBeenCalled();

    unmount(instance);
  });

  it('"Clear filters" navigates to the bare path through goto, not replaceState', async () => {
    // The harness never runs a real navigation (goto is mocked, so `page.url`
    // never actually changes), so a filter is applied here the only way
    // available from outside the router: overriding the getter this page
    // reads. Object-literal getters are configurable by default, and
    // `$app/state`'s `page` is a plain singleton object, so this is the
    // public surface, not a private import.
    Object.defineProperty(page, 'url', {
      configurable: true,
      get: () => new URL('http://example.test/o/acme/queue?status=pending')
    });

    const instance = mount(Harness, { target: container, props: { slug: 'acme' } });
    await settle();

    const clearButton = container.querySelector('button');
    expect(clearButton).not.toBeNull();
    expect(clearButton?.textContent).toContain('Clear filters');

    clearButton?.click();
    await settle();

    expect(gotoMock).toHaveBeenCalledTimes(1);
    const [url, opts] = gotoMock.mock.calls[0]!;
    expect(String(url)).toBe('/o/acme/queue');
    expect(opts).toMatchObject({ replaceState: true, keepFocus: true, noScroll: true });
    expect(replaceStateMock).not.toHaveBeenCalled();

    unmount(instance);
  });
});

/**
 * The job-list fetch itself, now a `Poller` subscription keyed on org and
 * filters (see `+page.svelte`'s `jobsPoll`). `poll.svelte.test.ts` proves the
 * state machine; this proves the table, the stale/parked note, and the
 * per-filter subscription actually behave that way on this page.
 *
 * Every stub in this block must also answer `/repos` and `/teams` — the
 * page's untouched picker `$effect` fires its own `Promise.all` on mount
 * independent of the job poll, and a stub that only knows `/jobs` would make
 * that effect fall through to whatever the `/jobs` branch returns.
 */
describe('the queue poller', () => {
  const baseJob = {
    id: 'job-1',
    orgId: 'org-1',
    repoId: 'repo-1',
    teamId: null,
    title: 'Wire the webhook ingest',
    description: null,
    status: 'pending',
    ticketRef: null,
    tracker: null,
    agentType: null,
    metadata: {},
    createdAt: new Date().toISOString(),
    startedAt: null,
    completedAt: null,
    attempts: 0,
    result: null,
    error: null,
    createdBy: null,
    claimedBy: null,
    claimedByLabel: null,
    claimExpiresAt: null,
    cancelRequestedAt: null,
    cancelRequestedBy: null,
    cancelReason: null
  };

  function jsonResponse(body: unknown, status = 200) {
    return new Response(JSON.stringify(body), {
      status,
      headers: { 'content-type': 'application/json' }
    });
  }

  const emptyPickers = () => Promise.resolve(jsonResponse([]));

  it('keeps the table and shows the stale note when a refresh fails, then clears it on recovery', async () => {
    let jobsStatus = 200;
    const fetchMock = vi.fn((path: string) => {
      if (path.includes('/repos') || path.includes('/teams')) return emptyPickers();
      if (path.includes('/jobs')) {
        return Promise.resolve(
          jobsStatus === 200 ? jsonResponse([baseJob]) : new Response('', { status: jobsStatus })
        );
      }
      return Promise.resolve(new Response('', { status: 404 }));
    });
    vi.stubGlobal('fetch', fetchMock);

    const instance = mount(Harness, { target: container, props: { slug: 'acme' } });
    await settle();

    expect(container.textContent).toContain('Wire the webhook ingest');
    expect(container.querySelector('[role="status"]')).toBeNull();

    jobsStatus = 502;
    // 1.4x, not 1x: the tick lands somewhere inside the subscription's phase
    // offset (up to +30%), and the page cannot inject a random source to pin
    // it exactly — see poll.svelte.test.ts for that.
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 1.4);

    // The real `fetch` mock answers with a real `Response`, and reading its
    // body needs a genuine turn of the real event loop, not just the fake
    // clock's microtask flush — `vi.waitFor` polls on a real timer until the
    // assertion holds or the wait times out.
    await vi.waitFor(() => {
      expect(container.textContent).toContain('Wire the webhook ingest');
      const note = container.querySelector('[role="status"]');
      expect(note).not.toBeNull();
      expect(note?.textContent).toContain('Refresh failed');
    });

    jobsStatus = 200;
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 4);
    await vi.waitFor(() => {
      expect(container.querySelector('[role="status"]')).toBeNull();
      expect(container.textContent).toContain('Wire the webhook ingest');
    });

    unmount(instance);
  });

  it('shows the stale note even when the job list is empty', async () => {
    // Regression test for the render-tree bug where the stale/parked note lived
    // only inside the table's branch: a successful-but-empty poll (a filter
    // matching nothing, or a fresh org) followed by a failing tick used to
    // render a confident "no jobs" empty state with no staleness indication at
    // all. The note must now be reachable regardless of `jobs.length`.
    let jobsStatus = 200;
    const fetchMock = vi.fn((path: string) => {
      if (path.includes('/repos') || path.includes('/teams')) return emptyPickers();
      if (path.includes('/jobs')) {
        return Promise.resolve(
          jobsStatus === 200 ? jsonResponse([]) : new Response('', { status: jobsStatus })
        );
      }
      return Promise.resolve(new Response('', { status: 404 }));
    });
    vi.stubGlobal('fetch', fetchMock);

    const instance = mount(Harness, { target: container, props: { slug: 'acme' } });
    await settle();

    expect(container.textContent).toContain('Nothing has been queued yet.');
    expect(container.querySelector('[role="status"]')).toBeNull();

    jobsStatus = 502;
    // 1.4x, not 1x: the tick lands somewhere inside the subscription's phase
    // offset (up to +30%) — see poll.svelte.test.ts for that.
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 1.4);

    await vi.waitFor(() => {
      // Still the empty state — there is no table for this note to hide inside.
      expect(container.textContent).toContain('Nothing has been queued yet.');
      const note = container.querySelector('[role="status"]');
      expect(note).not.toBeNull();
      expect(note?.textContent).toContain('Refresh failed');
    });

    unmount(instance);
  });

  it('stops polling and shows the error in place of the table on a 404', async () => {
    const fetchMock = vi.fn((path: string) => {
      if (path.includes('/repos') || path.includes('/teams')) return emptyPickers();
      if (path.includes('/jobs')) return Promise.resolve(new Response('', { status: 404 }));
      return Promise.resolve(new Response('', { status: 404 }));
    });
    vi.stubGlobal('fetch', fetchMock);

    const instance = mount(Harness, { target: container, props: { slug: 'acme' } });
    await settle();

    expect(container.querySelector('table')).toBeNull();
    expect(container.textContent).not.toContain('Wire the webhook ingest');
    const alert = container.querySelector('[role="alert"]');
    expect(alert).not.toBeNull();
    // The 404 stub answers with an empty body (no `error.code`), so `messageFor`
    // falls through to its unknown-code sentence rather than `queue_load_failed`
    // — the fallback there only applies to a failure that is not an `ApiError`
    // at all.
    expect(alert?.textContent).toContain('Something went wrong');

    const jobsCallCount = () =>
      fetchMock.mock.calls.filter(([path]) => String(path).includes('/jobs')).length;
    expect(jobsCallCount()).toBe(1);

    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 6);
    await settle();
    expect(jobsCallCount()).toBe(1);

    unmount(instance);
  });

  it('restarts the poll when a filter changes, and drops a late response for the old filter', async () => {
    function makeDeferred() {
      let resolve!: (response: Response) => void;
      const promise = new Promise<Response>((res) => {
        resolve = res;
      });
      return { promise, resolve };
    }

    // Each `/jobs` call gets its own still-pending promise, keyed by which
    // filter it belongs to, so the initial unfiltered request and the
    // filtered request that supersedes it can be resolved independently and
    // in whatever order the test needs — a genuinely in-flight request, not
    // a second `resolve()` on one already settled.
    const jobsDeferred: Record<'unfiltered' | 'pending', ReturnType<typeof makeDeferred>[]> = {
      unfiltered: [],
      pending: []
    };

    const fetchMock = vi.fn((path: string) => {
      if (path.includes('/repos') || path.includes('/teams')) return emptyPickers();
      if (path.includes('/jobs')) {
        const key = path.includes('status=pending') ? 'pending' : 'unfiltered';
        const deferred = makeDeferred();
        jobsDeferred[key].push(deferred);
        return deferred.promise;
      }
      return Promise.resolve(new Response('', { status: 404 }));
    });
    vi.stubGlobal('fetch', fetchMock);

    const instance = mount(Harness, {
      target: container,
      props: { slug: 'acme', url: new URL('http://example.test/o/acme/queue') }
    }) as unknown as { setUrl: (next: URL) => void };
    await settle();

    // The initial unfiltered request is in flight but deliberately left
    // unresolved here — it is resolved late, after the filter switch below,
    // to prove the superseded subscription's response is dropped.
    expect(jobsDeferred.unfiltered).toHaveLength(1);

    instance.setUrl(new URL('http://example.test/o/acme/queue?status=pending'));
    await settle();

    // The filter switch tore down the unfiltered `Poller` subscription and
    // started a new one for `status=pending` — no second unfiltered call,
    // and exactly one pending-filtered call now in flight.
    expect(jobsDeferred.unfiltered).toHaveLength(1);
    expect(jobsDeferred.pending).toHaveLength(1);

    jobsDeferred.pending[0]!.resolve(
      jsonResponse([{ ...baseJob, id: 'job-pending', title: 'Pending job' }])
    );
    await vi.waitFor(() => {
      expect(container.textContent).toContain('Pending job');
    });

    // The response for the superseded (unfiltered) subscription arrives
    // late — from a promise that was never resolved before the switch — and
    // must not be applied. See `Poller`'s generation counter and
    // `docs/specs/2026-09-09-overview-polling-design.md` §3.
    jobsDeferred.unfiltered[0]!.resolve(
      jsonResponse([{ ...baseJob, id: 'job-late', title: 'Late unfiltered job' }])
    );
    await settle();
    expect(container.textContent).not.toContain('Late unfiltered job');
    expect(container.textContent).toContain('Pending job');

    unmount(instance);
  });

  it('a navigation error is its own independent notice: it is not erased by an unrelated poll success, and does not hide a live poll failure', async () => {
    // Regression test for the corrected `navError`/`pollError` design: the two
    // are no longer merged into one slot, so there is no "priority" between
    // them at all. A rejected `goto` renders its own `Alert`, independent of
    // whatever the job poll is doing, and it is retired only by a genuine
    // org/filter change — never by an unrelated poll tick merely succeeding.
    Object.defineProperty(page, 'url', {
      configurable: true,
      get: () => new URL('http://example.test/o/acme/queue')
    });

    let jobsStatus = 200;
    const fetchMock = vi.fn((path: string) => {
      if (path.includes('/repos') || path.includes('/teams')) return emptyPickers();
      if (path.includes('/jobs')) {
        return Promise.resolve(
          jobsStatus === 200 ? jsonResponse([baseJob]) : new Response('', { status: jobsStatus })
        );
      }
      return Promise.resolve(new Response('', { status: 404 }));
    });
    vi.stubGlobal('fetch', fetchMock);

    // The one rejected navigation this test needs — every other `goto` call
    // in this suite resolves via the default mock set up in `beforeEach`.
    gotoMock.mockRejectedValueOnce(new Error('chunk failed to load'));

    const instance = mount(Harness, { target: container, props: { slug: 'acme' } });
    await settle();
    expect(container.textContent).toContain('Wire the webhook ingest');

    const select = container.querySelector('select') as HTMLSelectElement;
    select.value = 'pending';
    select.dispatchEvent(new Event('change', { bubbles: true }));
    await settle();

    // `goto` rejected, so `page.url` never actually changed and the job-poll
    // effect never re-ran — the nav alert is showing, and the poll itself is
    // still healthy (no poll-error alert alongside it).
    const navAlert = () =>
      Array.from(container.querySelectorAll('[role="alert"]')).find((el) =>
        el.textContent?.includes('Could not load the queue.')
      );
    expect(navAlert()).toBeTruthy();
    expect(container.querySelectorAll('[role="alert"]')).toHaveLength(1);

    // A full healthy poll tick passes — nothing about org or filters changed,
    // so this must NOT clear the nav alert. (This is the regression the old
    // auto-clear-on-`updatedAt` effect would have caused: it retired `navError`
    // the moment any unrelated tick succeeded, with no bearing on whether the
    // navigation problem itself was ever fixed.)
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 1.4);
    await settle();
    expect(navAlert()).toBeTruthy();

    // Now the underlying poll hits a fatal failure on a later tick, with the
    // nav alert still up and nothing having changed org or filters. Both
    // alerts must be visible at once — one is not a substitute for the other.
    jobsStatus = 404;
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 1.4);
    await vi.waitFor(() => {
      expect(navAlert()).toBeTruthy();
      const pollAlert = Array.from(container.querySelectorAll('[role="alert"]')).find((el) =>
        el.textContent?.includes('Something went wrong')
      );
      expect(pollAlert).toBeTruthy();
      // A fatal (stopped) poll failure gets no "still retrying" hint.
      expect(pollAlert?.textContent).not.toContain('Still retrying');
    });
    expect(container.querySelectorAll('[role="alert"]')).toHaveLength(2);

    unmount(instance);
  });
});
