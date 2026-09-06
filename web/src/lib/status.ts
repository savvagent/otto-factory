/**
 * The words for a job's status.
 *
 * **The wire values do not change.** `JobStatus` is `pending` / `in-progress` /
 * `active` / `completed` / `failed` on the API, in the database, and in every
 * MCP tool; this is only what a human reads. `StatusPill` rendered the enum
 * value directly, which was fine while the console was English-only and is a
 * screenful of untranslated jargon now.
 *
 * A separate module rather than a function inside `StatusPill`, because the
 * queue's filter dropdown renders the same five words and has to agree with the
 * pills beside it.
 */

import { m } from '$lib/paraglide/messages';
import type { JobStatus } from '$lib/types';

export function statusLabel(status: JobStatus): string {
  switch (status) {
    case 'pending':
      return m.status_pending();
    case 'in-progress':
      return m.status_in_progress();
    case 'active':
      return m.status_active();
    case 'completed':
      return m.status_completed();
    case 'failed':
      return m.status_failed();
  }
}
