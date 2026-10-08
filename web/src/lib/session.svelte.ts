/**
 * Who is signed in, held in one place.
 *
 * A rune-backed module, not a Svelte store: `$state` in a `.svelte.ts` file is
 * reactive anywhere it is imported, and the whole console reads the same object
 * rather than each page fetching `/api/session` on mount.
 *
 * **There is no cached credential here.** The session lives in an `HttpOnly`
 * cookie the browser holds, and it keys a token pair the *server* holds; this
 * object caches only the *answer* to "who is that cookie", which is a rendering
 * convenience. The server re-resolves the cookie on every request regardless, so
 * the worst a stale copy here can do is render the wrong name for one paint; it
 * can never grant access, because nothing downstream trusts it.
 *
 * A session is signed in to exactly one org. Everything org-shaped the console
 * shows — the name, the role, the plan — comes from this answer.
 */

import { api, ApiError } from './api';
import type { SessionInfo } from './types';

class Session {
  /** `undefined` while the first `/api/session` is in flight, and when signed out. */
  info = $state<SessionInfo | undefined>(undefined);
  /** False until the first resolution attempt finishes, success or not. */
  ready = $state(false);

  get signedIn(): boolean {
    return this.info !== undefined;
  }

  /** The platform's own console, for everything identity-shaped. */
  get platformUrl(): string | undefined {
    return this.info?.platformUrl;
  }

  /**
   * Re-resolve the session cookie.
   *
   * A `401` is not an error to report — it is the ordinary answer for a visitor
   * who has not signed in yet, and the layout uses it to decide whether to send
   * them through sign-in. Anything else is left to throw: a `500` from
   * `/api/session` is not "you are signed out", and silently treating it as such
   * would send everyone through sign-in during an incident.
   */
  async refresh(): Promise<SessionInfo | undefined> {
    try {
      this.info = await api.session();
    } catch (error) {
      if (error instanceof ApiError && error.isUnauthenticated) {
        this.info = undefined;
      } else {
        this.ready = true;
        throw error;
      }
    }
    this.ready = true;
    return this.info;
  }

  /** Drop the local copy. The cookie is cleared by `POST /auth/logout`. */
  clear(): void {
    this.info = undefined;
  }
}

export const session = new Session();
