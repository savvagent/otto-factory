// @vitest-environment jsdom
/**
 * Presence-pill, heading, explainer, and info-affordance coverage for the
 * repos page. This page reads no query params (no `$app/navigation` mocking
 * needed, unlike the queue harness) — it only needs `fetch` stubbed.
 */

import { mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import Harness from './RepoHarness.svelte';

let container: HTMLElement;

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' }
  });
}

const baseRepo = {
  id: 'r1',
  orgId: 'o1',
  slug: 'api',
  name: 'api',
  provider: 'github',
  defaultBranch: 'main',
  teamId: null,
  defaultAgentType: null,
  trackerBinding: {},
  active: true,
  createdAt: new Date().toISOString(),
  createdBy: null,
  hasActiveLease: true
};

const quietRepo = {
  ...baseRepo,
  id: 'r2',
  slug: 'quiet',
  hasActiveLease: false
};

function makeFetchMock(opts?: {
  repos?: unknown[];
  leasesByRepo?: Record<string, unknown[]>;
  bindingsByRepo?: Record<string, unknown[]>;
}) {
  const repos = opts?.repos ?? [baseRepo, quietRepo];
  const leasesByRepo = opts?.leasesByRepo ?? {};
  const bindingsByRepo = opts?.bindingsByRepo ?? {};

  return vi.fn((path: string) => {
    if (path.includes('/teams')) return Promise.resolve(jsonResponse([]));
    if (path.includes('/tracker-bindings')) {
      const match = path.match(/\/repos\/([^/]+)\/tracker-bindings/);
      const slug_ = match?.[1];
      return Promise.resolve(jsonResponse((slug_ && bindingsByRepo[slug_]) ?? []));
    }
    if (path.includes('/leases')) {
      const match = path.match(/\/repos\/([^/]+)\/leases/);
      const slug_ = match?.[1];
      return Promise.resolve(jsonResponse((slug_ && leasesByRepo[slug_]) ?? []));
    }
    if (path.includes('/repos')) return Promise.resolve(jsonResponse(repos));
    return Promise.resolve(new Response('', { status: 404 }));
  });
}

beforeEach(() => {
  vi.useFakeTimers();
  container = document.createElement('div');
  document.body.appendChild(container);
});

afterEach(() => {
  container.remove();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

/** Let the page's effects and their fetches settle. */
const settle = () => vi.advanceTimersByTimeAsync(0);

describe('the repos page presence pill', () => {
  it('renders "In use" only for the repo with an active lease', async () => {
    vi.stubGlobal('fetch', makeFetchMock());

    const instance = mount(Harness, { target: container, props: { slug: 'acme' } });
    await settle();

    const occurrences = (container.textContent?.match(/In use/g) ?? []).length;
    expect(occurrences).toBe(1);

    unmount(instance);
  });
});

describe('the repos page toggle', () => {
  it('flips text and aria-expanded when clicked', async () => {
    vi.stubGlobal('fetch', makeFetchMock());

    const instance = mount(Harness, { target: container, props: { slug: 'acme' } });
    await settle();

    const toggle = Array.from(container.querySelectorAll('button')).find((b) =>
      b.hasAttribute('aria-expanded')
    ) as HTMLButtonElement;
    expect(toggle).toBeTruthy();
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
    expect(toggle.textContent).toBe('Who is in here?');

    toggle.click();
    await settle();

    expect(toggle.getAttribute('aria-expanded')).toBe('true');
    expect(toggle.textContent).toBe('Hide');

    unmount(instance);
  });
});

describe('the repos page lease panel heading and explainer', () => {
  it('shows the heading and the always-visible note even with zero leases', async () => {
    vi.stubGlobal(
      'fetch',
      makeFetchMock({ leasesByRepo: { api: [] }, bindingsByRepo: { api: [] } })
    );

    const instance = mount(Harness, { target: container, props: { slug: 'acme' } });
    await settle();

    const toggle = Array.from(container.querySelectorAll('button')).find(
      (b) => b.hasAttribute('aria-expanded') && b.textContent === 'Who is in here?'
    ) as HTMLButtonElement;
    toggle.click();
    await settle();

    expect(container.textContent).toContain("Who's here");
    expect(container.textContent).toContain(
      'Leases are advisory. The server cannot see a git push, so a lease makes a collision visible — it does not prevent one.'
    );

    unmount(instance);
  });
});

describe('the repos page info affordance', () => {
  it('opens on focus, closes on Escape, and closes on blur', async () => {
    vi.stubGlobal(
      'fetch',
      makeFetchMock({ leasesByRepo: { api: [] }, bindingsByRepo: { api: [] } })
    );

    const instance = mount(Harness, { target: container, props: { slug: 'acme' } });
    await settle();

    const toggle = Array.from(container.querySelectorAll('button')).find(
      (b) => b.hasAttribute('aria-expanded') && b.textContent === 'Who is in here?'
    ) as HTMLButtonElement;
    toggle.click();
    await settle();

    const infoButton = container.querySelector(
      'button[aria-label="About leases"]'
    ) as HTMLButtonElement;
    expect(infoButton).toBeTruthy();

    const detail =
      'Leases are what the list_leases and acquire_lease MCP tools coordinate through, and expire on their own if never renewed.';

    expect(container.textContent).not.toContain(detail);

    infoButton.dispatchEvent(new FocusEvent('focus', { bubbles: true }));
    await settle();
    expect(container.textContent).toContain(detail);

    infoButton.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    await settle();
    expect(container.textContent).not.toContain(detail);

    infoButton.dispatchEvent(new FocusEvent('focus', { bubbles: true }));
    await settle();
    expect(container.textContent).toContain(detail);

    infoButton.dispatchEvent(new FocusEvent('blur', { bubbles: true }));
    await settle();
    expect(container.textContent).not.toContain(detail);

    unmount(instance);
  });
});
