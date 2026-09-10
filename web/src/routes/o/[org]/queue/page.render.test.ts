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
