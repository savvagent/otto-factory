// @vitest-environment jsdom
/**
 * Starting sign-in, and not looping over it.
 *
 * The loop guard is the part worth testing: a session is bound to one org, so a
 * visitor on `/o/globex` with an `acme` session is sent to sign in for `globex`.
 * If the platform signs them in to `acme` anyway they come straight back to the
 * same mismatch, and without the guard they would bounce forever.
 */

import { beforeEach, describe, expect, it } from 'vitest';

import { loginUrl, mayRetryLogin, orgOf } from './login';

describe('loginUrl', () => {
  it('carries the org and where to come back to, encoded', () => {
    expect(loginUrl('acme', '/o/acme/queue?status=pending')).toBe(
      '/auth/login?org=acme&next=%2Fo%2Facme%2Fqueue%3Fstatus%3Dpending'
    );
  });

  it('omits what is absent', () => {
    expect(loginUrl()).toBe('/auth/login');
    expect(loginUrl('acme')).toBe('/auth/login?org=acme');
    expect(loginUrl(undefined, '/o/x')).toBe('/auth/login?next=%2Fo%2Fx');
  });

  it('cannot be made to smuggle a second parameter through the org', () => {
    expect(loginUrl('a&org=evil')).toBe('/auth/login?org=a%26org%3Devil');
  });
});

describe('orgOf', () => {
  it('reads the slug out of a console path', () => {
    expect(orgOf('/o/acme')).toBe('acme');
    expect(orgOf('/o/acme/queue/job-1')).toBe('acme');
    expect(orgOf('/o/a%20b/queue')).toBe('a b');
  });

  it('is nothing for a path that is not under an org', () => {
    for (const path of ['/', '/docs/api', '/trackers/callback', '/o', '/o/']) {
      expect(orgOf(path), path).toBeUndefined();
    }
    expect(orgOf('/o/%E0%A4%A')).toBeUndefined();
  });
});

describe('mayRetryLogin', () => {
  beforeEach(() => sessionStorage.clear());

  it('allows the first attempt and refuses a repeat for the same org', () => {
    expect(mayRetryLogin('globex', 1_000)).toBe(true);
    expect(mayRetryLogin('globex', 2_000)).toBe(false);
  });

  it('allows a different org, and the same org again once the window has passed', () => {
    expect(mayRetryLogin('globex', 1_000)).toBe(true);
    expect(mayRetryLogin('acme', 2_000)).toBe(true);
    expect(mayRetryLogin('acme', 2_000 + 61_000)).toBe(true);
  });

  it('treats "no org" as its own key', () => {
    expect(mayRetryLogin(undefined, 1_000)).toBe(true);
    expect(mayRetryLogin(undefined, 2_000)).toBe(false);
    expect(mayRetryLogin('acme', 2_000)).toBe(true);
  });

  it('allows an attempt when storage is unavailable, rather than failing sign-in', () => {
    const real = Object.getOwnPropertyDescriptor(window, 'sessionStorage');
    Object.defineProperty(window, 'sessionStorage', {
      configurable: true,
      get() {
        throw new Error('blocked');
      }
    });
    try {
      expect(mayRetryLogin('acme', 1_000)).toBe(true);
    } finally {
      if (real) Object.defineProperty(window, 'sessionStorage', real);
    }
  });
});
