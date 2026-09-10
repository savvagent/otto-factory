import type { Job } from '$lib/types';

/**
 * True for an in-progress/active job whose claim has lapsed — nobody is
 * actually working it, but nothing has reclaimed it yet. Derived
 * client-side from fields the server already returns; no new server
 * state, matching how the console derives every other display-only fact.
 *
 * A separate module rather than a function inside `labels.ts` or
 * `format.ts`: both of those are strictly value-in, string-out (a wire
 * value or a date mapped to display text), and this is a boolean decision
 * about job state — a different kind of thing from either.
 */
export function isClaimStranded(job: Pick<Job, 'status' | 'claimExpiresAt'>): boolean {
  if (job.status !== 'in-progress' && job.status !== 'active') return false;
  if (!job.claimExpiresAt) return false;
  return new Date(job.claimExpiresAt).getTime() <= Date.now();
}
