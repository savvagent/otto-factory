/**
 * `fatalApiFailure` is the one classifier shared by every page that polls an
 * org-scoped resource, and its 401 branch has a real side effect — clearing
 * the session. A regression in that branch, or in the 403 branch beside it,
 * would not otherwise be caught: the page-level tests only exercise a 404.
 */

import { afterEach, describe, expect, it, vi } from 'vitest';

import { ApiError } from './api';
import { fatalApiFailure } from './poll-fatal';
import { session } from './session.svelte';

afterEach(() => vi.restoreAllMocks());

describe('fatalApiFailure', () => {
  it('is not fatal for a failure that is not an ApiError, and does not touch the session', () => {
    const clear = vi.spyOn(session, 'clear');
    expect(fatalApiFailure(new Error('network blip'))).toBe(false);
    expect(fatalApiFailure(undefined)).toBe(false);
    expect(clear).not.toHaveBeenCalled();
  });

  it('is fatal for a 401 and clears the session', () => {
    const clear = vi.spyOn(session, 'clear');
    const unauthenticated = new ApiError(401, 'unauthenticated', 'no session');
    expect(fatalApiFailure(unauthenticated)).toBe(true);
    expect(clear).toHaveBeenCalledOnce();
  });

  it('is fatal for a 404, and does not clear the session', () => {
    const clear = vi.spyOn(session, 'clear');
    const notFound = new ApiError(404, 'org_not_found', 'no such org');
    expect(fatalApiFailure(notFound)).toBe(true);
    expect(clear).not.toHaveBeenCalled();
  });

  it('is fatal for a plain 403, and does not clear the session', () => {
    const clear = vi.spyOn(session, 'clear');
    const forbidden = new ApiError(403, 'forbidden', 'not allowed');
    expect(fatalApiFailure(forbidden)).toBe(true);
    expect(clear).not.toHaveBeenCalled();
  });

  it('is not fatal for an ApiError that is neither 401, 404 nor 403', () => {
    const badGateway = new ApiError(502, 'bad_gateway', 'upstream is down');
    expect(fatalApiFailure(badGateway)).toBe(false);
  });
});
