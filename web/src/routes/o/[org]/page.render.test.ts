// @vitest-environment jsdom
/**
 * What the overview does when a refresh fails — asserted where a user would
 * see it, because the rule lives half in `Poller` and half in the markup.
 *
 * `poll.svelte.test.ts` proves the state machine. It cannot prove that a failed
 * tick leaves the *tiles* alone, that the warning is the reason the six message
 * catalogs gained a string, or that the error branch replaces the page rather
 * than sitting above stale data. Those only exist here.
 */

import { mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { REFRESH_INTERVAL } from '$lib/poll.svelte';
import Harness from './OrgPageHarness.svelte';

const stats = {
  pending: 4,
  inProgress: 1,
  active: 2,
  completed: 7,
  failed: 0,
  blocked: 0,
  total: 14
};

const jobs = [
  {
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
    claimedByLabel: null
  }
];

const repos = [
  {
    id: 'repo-1',
    orgId: 'org-1',
    slug: 'otto-factory',
    name: 'otto-factory',
    provider: 'github',
    defaultBranch: 'master',
    teamId: null,
    defaultAgentType: null,
    active: true,
    createdAt: new Date().toISOString(),
    createdBy: null
  }
];

const usage = {
  plan: 'free',
  includedOps: 1000,
  billableUsed: 12,
  remaining: 988,
  totalCalls: 40,
  periodStart: '2026-09-01',
  warning: false,
  hardStop: false,
  enforced: false
};

/** Answers the four calls the overview makes, or fails all of them. */
function serve(healthy: () => boolean) {
  return vi.fn((path: string) => {
    if (!healthy()) return Promise.resolve(new Response('', { status: 502 }));
    const body = path.includes('/jobs/stats')
      ? stats
      : path.includes('/jobs')
        ? jobs
        : path.includes('/repos')
          ? repos
          : usage;
    return Promise.resolve(
      new Response(JSON.stringify(body), { headers: { 'content-type': 'application/json' } })
    );
  });
}

let container: HTMLElement;
let healthy = true;

beforeEach(() => {
  vi.useFakeTimers();
  healthy = true;
  container = document.createElement('div');
  document.body.appendChild(container);
  vi.stubGlobal(
    'fetch',
    serve(() => healthy)
  );
});

afterEach(() => {
  container.remove();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

/** Let the page's effects and their fetches settle. */
const settle = () => vi.advanceTimersByTimeAsync(0);

describe('the overview', () => {
  it('keeps its data and says so when a refresh fails, then recovers', async () => {
    const instance = mount(Harness, { target: container, props: { slug: 'acme' } });
    await settle();

    expect(container.textContent).toContain('Wire the webhook ingest');
    expect(container.textContent).toContain('otto-factory');
    expect(container.querySelector('[role="status"]')).toBeNull();

    healthy = false;
    // 1.4x, not 1x: the page cannot inject a random source, so the tick lands
    // somewhere inside the subscription's phase offset (up to +30%). Exact
    // timing is pinned in poll.svelte.test.ts, where the source is injectable.
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 1.4);

    // The whole point of the stale rule: one 502 must not cost the reader a
    // working dashboard.
    //
    // `advanceTimersByTimeAsync` above is what fires the refresh's fake timer;
    // it is not what finishes the rejected fetch. The real `fetch` mock answers
    // with a real `Response`, and `Response.text()` resolves through the
    // runtime's own body-reading machinery, which needs a genuine turn of the
    // real event loop, not just the fake clock's microtask flush. Advancing the
    // clock further would only narrow that race, not close it — `vi.waitFor`
    // actually polls on a real timer, so it keeps giving the pending fetch
    // chain real turns until the assertion is true or the wait times out.
    await vi.waitFor(() => {
      expect(container.textContent).toContain('Wire the webhook ingest');
      const note = container.querySelector('[role="status"]');
      expect(note).not.toBeNull();
      expect(note?.textContent).toContain('Refresh failed');
    });

    healthy = true;
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 4);
    // Same race on the way back: recovery also reads a real `Response` body
    // before `stale` clears.
    await vi.waitFor(() => {
      expect(container.querySelector('[role="status"]')).toBeNull();
      expect(container.textContent).toContain('Wire the webhook ingest');
    });

    unmount(instance);
  });

  it('shows an error instead of the page when the first load fails', async () => {
    healthy = false;
    const instance = mount(Harness, { target: container, props: { slug: 'acme' } });
    await settle();

    expect(container.textContent).not.toContain('Wire the webhook ingest');
    expect(container.textContent).toContain('Still retrying');

    unmount(instance);
  });
});
