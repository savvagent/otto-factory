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

const leaseInfoDetail =
  'Leases are what the list_leases and acquire_lease MCP tools coordinate through, and expire on their own if never renewed.';

/** All "Who is in here?" / "Hide" row toggles, in row order. */
function rowToggles(root: HTMLElement) {
  return Array.from(root.querySelectorAll('button')).filter(
    (b) => b.hasAttribute('aria-expanded') && /^(Who is in here\?|Hide)$/.test(b.textContent ?? '')
  ) as HTMLButtonElement[];
}

/**
 * The info popover is always in the DOM once its row is expanded (so
 * `aria-controls` never points at a nonexistent id) and toggles visibility
 * via the `hidden` attribute rather than being mounted/unmounted — so
 * "closed" is asserted via `.hidden`, not via absence from the DOM or from
 * `container.textContent` (which includes hidden elements' text).
 */
function infoPanelFor(root: HTMLElement, slug: string) {
  return root.querySelector(`#repos-lease-info-${slug}`) as HTMLElement | null;
}

describe('the repos page info affordance', () => {
  it('does not open on a bare focus event, opens on click, and closes on Escape or blur', async () => {
    vi.stubGlobal(
      'fetch',
      makeFetchMock({ leasesByRepo: { api: [] }, bindingsByRepo: { api: [] } })
    );

    const instance = mount(Harness, { target: container, props: { slug: 'acme' } });
    await settle();

    rowToggles(container)[0]?.click();
    await settle();

    const infoButton = container.querySelector(
      'button[aria-label="About leases"]'
    ) as HTMLButtonElement;
    expect(infoButton).toBeTruthy();
    expect(infoButton.getAttribute('aria-controls')).toBe('repos-lease-info-api');
    expect(infoPanelFor(container, 'api')?.hidden).toBe(true);

    // A bare focus (no click) must not open the popover any more — a real
    // click fires focus before click, so an onfocus-opens handler would race
    // the onclick toggle and lose.
    infoButton.dispatchEvent(new FocusEvent('focus', { bubbles: true }));
    await settle();
    expect(infoButton.getAttribute('aria-expanded')).toBe('false');
    expect(infoPanelFor(container, 'api')?.hidden).toBe(true);

    infoButton.click();
    await settle();
    expect(infoButton.getAttribute('aria-expanded')).toBe('true');
    expect(infoPanelFor(container, 'api')?.hidden).toBe(false);
    expect(container.textContent).toContain(leaseInfoDetail);

    infoButton.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    await settle();
    expect(infoButton.getAttribute('aria-expanded')).toBe('false');
    expect(infoPanelFor(container, 'api')?.hidden).toBe(true);

    infoButton.click();
    await settle();
    expect(infoPanelFor(container, 'api')?.hidden).toBe(false);

    infoButton.dispatchEvent(new FocusEvent('blur', { bubbles: true }));
    await settle();
    expect(infoButton.getAttribute('aria-expanded')).toBe('false');
    expect(infoPanelFor(container, 'api')?.hidden).toBe(true);

    unmount(instance);
  });

  it('opens from a real mouse click, where focus fires immediately before click', async () => {
    vi.stubGlobal(
      'fetch',
      makeFetchMock({ leasesByRepo: { api: [] }, bindingsByRepo: { api: [] } })
    );

    const instance = mount(Harness, { target: container, props: { slug: 'acme' } });
    await settle();

    rowToggles(container)[0]?.click();
    await settle();

    const infoButton = container.querySelector(
      'button[aria-label="About leases"]'
    ) as HTMLButtonElement;

    // JSDOM's HTMLElement.click() does not fire a focus event first, unlike a
    // real browser (mousedown -> focus -> mouseup -> click), so this
    // reproduces that ordering explicitly. The onclick toggle must still win.
    infoButton.dispatchEvent(new FocusEvent('focus', { bubbles: true }));
    infoButton.click();
    await settle();

    expect(infoButton.getAttribute('aria-expanded')).toBe('true');
    expect(infoPanelFor(container, 'api')?.hidden).toBe(false);
    expect(container.textContent).toContain(leaseInfoDetail);

    unmount(instance);
  });

  it('does not leak an open popover into a different row switched to directly', async () => {
    vi.stubGlobal(
      'fetch',
      makeFetchMock({
        leasesByRepo: { api: [], quiet: [] },
        bindingsByRepo: { api: [], quiet: [] }
      })
    );

    const instance = mount(Harness, { target: container, props: { slug: 'acme' } });
    await settle();

    const toggles = rowToggles(container);
    expect(toggles).toHaveLength(2);
    const [firstToggle, secondToggle] = toggles as [HTMLButtonElement, HTMLButtonElement];
    firstToggle.click();
    await settle();

    const firstInfoButton = container.querySelector(
      'button[aria-label="About leases"]'
    ) as HTMLButtonElement;
    firstInfoButton.click();
    await settle();
    expect(firstInfoButton.getAttribute('aria-expanded')).toBe('true');
    expect(infoPanelFor(container, 'api')?.hidden).toBe(false);
    expect(container.textContent).toContain(leaseInfoDetail);

    secondToggle.click();
    await settle();

    const secondInfoButton = container.querySelector(
      'button[aria-label="About leases"]'
    ) as HTMLButtonElement;
    expect(secondInfoButton).toBeTruthy();
    expect(secondInfoButton.getAttribute('aria-controls')).toBe('repos-lease-info-quiet');
    expect(secondInfoButton.getAttribute('aria-expanded')).toBe('false');
    expect(infoPanelFor(container, 'quiet')?.hidden).toBe(true);

    unmount(instance);
  });
});
