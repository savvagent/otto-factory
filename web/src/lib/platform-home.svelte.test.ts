import { describe, expect, it, vi } from 'vitest';

import { PlatformHome } from './platform-home.svelte';

const PLATFORM = 'https://otto.example';

/** A discovery the test resolves by hand, to order it against the session. */
function pendingDiscovery() {
  let resolve!: (url: string | undefined) => void;
  const promise = new Promise<string | undefined>((r) => (resolve = r));
  const discover = vi.fn(() => promise);
  return {
    discover,
    async answer(url: string | undefined) {
      resolve(url);
      await promise;
      await Promise.resolve();
    }
  };
}

describe('PlatformHome', () => {
  it('uses the session address while signed in and never asks discovery', () => {
    const discover = vi.fn(async () => 'https://elsewhere.example');
    const home = new PlatformHome(discover);
    home.observe(PLATFORM, true);
    expect(home.url(PLATFORM)).toBe(PLATFORM);
    expect(discover).not.toHaveBeenCalled();
  });

  it('remembers the session address through sign-out, without fetching', () => {
    const discover = vi.fn(async () => undefined);
    const home = new PlatformHome(discover);
    home.observe(PLATFORM, true);
    home.observe(undefined, true);
    expect(home.url(undefined)).toBe(PLATFORM);
    expect(discover).not.toHaveBeenCalled();
  });

  it('waits for the session lookup to resolve before discovering', () => {
    const discover = vi.fn(async () => PLATFORM);
    const home = new PlatformHome(discover);
    home.observe(undefined, false);
    expect(discover).not.toHaveBeenCalled();
    expect(home.url(undefined)).toBeUndefined();
  });

  it('discovers once for a visitor with no usable address', async () => {
    const pending = pendingDiscovery();
    const home = new PlatformHome(pending.discover);
    home.observe(undefined, true);
    home.observe(undefined, true);
    expect(pending.discover).toHaveBeenCalledTimes(1);
    await pending.answer(PLATFORM);
    expect(home.url(undefined)).toBe(PLATFORM);
  });

  it('hides the link when discovery finds nothing', async () => {
    const pending = pendingDiscovery();
    const home = new PlatformHome(pending.discover);
    home.observe(undefined, true);
    await pending.answer(undefined);
    expect(home.url(undefined)).toBeUndefined();
  });

  it('lets the session address win over discovery, in either order', async () => {
    const pending = pendingDiscovery();
    const home = new PlatformHome(pending.discover);
    home.observe(undefined, true);
    expect(home.url(PLATFORM)).toBe(PLATFORM);
    // The session resolves signed in while discovery is still in flight; a late
    // failure must not erase what the session said once it is gone again.
    home.observe(PLATFORM, true);
    await pending.answer(undefined);
    home.observe(undefined, true);
    expect(home.url(undefined)).toBe(PLATFORM);
    expect(pending.discover).toHaveBeenCalledTimes(1);
  });
});
