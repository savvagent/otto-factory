import { ApiError } from './api';
import { session } from './session.svelte';

/**
 * Which `Poller` failures must not be retried, shared by every page that
 * polls an org-scoped resource. A `401` means the session is gone — clearing
 * the local copy is what lets the root layout's guard send the tab to
 * `/login`. A `403`/`404` means the org (or, for a page with its own
 * sub-resource filters, the specific resource) is gone or this account no
 * longer has access to it — the data on screen belongs to a query this
 * reader can no longer make. Everything else is a blip worth retrying.
 */
export function fatalApiFailure(failure: unknown): boolean {
  if (!(failure instanceof ApiError)) return false;
  if (failure.isUnauthenticated) {
    session.clear();
    return true;
  }
  return failure.isNotFound || failure.status === 403;
}
