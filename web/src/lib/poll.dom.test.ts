// @vitest-environment jsdom
/**
 * The two default seams, which every other test in `poll.svelte.test.ts`
 * replaces with a fake.
 *
 * That file injects `Visibility` and `Activity` everywhere, which is what makes
 * it deterministic — and it means the implementations that actually ship are
 * the one part it cannot cover. A regression in `documentVisibility` would take
 * the whole hidden-tab rule out of production with a fully green suite.
 */

import { afterEach, describe, expect, it, vi } from 'vitest';

import { documentActivity, documentVisibility } from './poll.svelte';

/** jsdom reports `visible` and offers no setter; the property is configurable. */
function setVisibility(state: DocumentVisibilityState) {
  Object.defineProperty(document, 'visibilityState', { value: state, configurable: true });
  document.dispatchEvent(new Event('visibilitychange'));
}

afterEach(() => setVisibility('visible'));

describe('documentVisibility', () => {
  it('tracks the document and notifies on change', () => {
    const onChange = vi.fn();
    const unsubscribe = documentVisibility.subscribe(onChange);

    expect(documentVisibility.hidden()).toBe(false);
    setVisibility('hidden');
    expect(documentVisibility.hidden()).toBe(true);
    expect(onChange).toHaveBeenCalledTimes(1);

    unsubscribe();
    setVisibility('visible');
    expect(onChange).toHaveBeenCalledTimes(1);
  });
});

describe('documentActivity', () => {
  it('reports input and stops when unsubscribed', () => {
    const onActive = vi.fn();
    const unsubscribe = documentActivity.subscribe(onActive);

    document.dispatchEvent(new Event('keydown'));
    document.dispatchEvent(new Event('pointerdown'));
    expect(onActive).toHaveBeenCalledTimes(2);

    unsubscribe();
    document.dispatchEvent(new Event('keydown'));
    expect(onActive).toHaveBeenCalledTimes(2);
  });

  it('still sees input a handler stops propagating', () => {
    // Capture-phase, because a poll that parked itself over a swallowed event
    // would read a working page as an empty desk.
    const onActive = vi.fn();
    const unsubscribe = documentActivity.subscribe(onActive);
    const target = document.createElement('button');
    document.body.appendChild(target);
    target.addEventListener('keydown', (event) => event.stopPropagation());

    target.dispatchEvent(new Event('keydown', { bubbles: true }));
    expect(onActive).toHaveBeenCalledTimes(1);

    unsubscribe();
    target.remove();
  });
});
