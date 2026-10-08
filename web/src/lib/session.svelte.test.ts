// @vitest-environment jsdom
/**
 * `session.refresh()` turns `401` into "signed out" and nothing else.
 *
 * Treating any other failure as signed-out would send everyone through sign-in
 * during an incident.
 */

import { afterEach, describe, expect, it, vi } from 'vitest';

import { ApiError } from './api';
import { session } from './session.svelte';

const INFO = {
  user: { id: 'u1', email: 'rob@acme.test', name: null },
  org: { id: 'o1', slug: 'acme', name: 'Acme', plan: 'free' },
  role: 'owner',
  scopes: ['jobs:read'],
  platformUrl: 'https://otto.example'
};

function respond(status: number, body: unknown) {
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => new Response(JSON.stringify(body), { status }))
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
  session.clear();
  session.ready = false;
});

describe('session.refresh', () => {
  it('holds who the server says is signed in', async () => {
    respond(200, INFO);
    await session.refresh();
    expect(session.signedIn).toBe(true);
    expect(session.info?.org.slug).toBe('acme');
    expect(session.platformUrl).toBe('https://otto.example');
    expect(session.ready).toBe(true);
  });

  it('treats a 401 as signed out, and ready', async () => {
    respond(401, { error: { code: 'unauthenticated', message: 'no' } });
    await session.refresh();
    expect(session.signedIn).toBe(false);
    expect(session.ready).toBe(true);
  });

  it('does not treat a server error as signed out', async () => {
    respond(500, { error: { code: 'internal_error', message: 'x' } });
    await expect(session.refresh()).rejects.toBeInstanceOf(ApiError);
    expect(session.ready).toBe(true);
  });

  it('forgets who it was when cleared', async () => {
    respond(200, INFO);
    await session.refresh();
    session.clear();
    expect(session.signedIn).toBe(false);
  });
});
