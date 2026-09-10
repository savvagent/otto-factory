import { describe, expect, it } from 'vitest';

import { isClaimStranded } from './jobs';

const past = new Date(Date.now() - 60_000).toISOString();
const future = new Date(Date.now() + 60_000).toISOString();

describe('isClaimStranded', () => {
  it('is never stranded while pending, regardless of claimExpiresAt', () => {
    expect(isClaimStranded({ status: 'pending', claimExpiresAt: past })).toBe(false);
    expect(isClaimStranded({ status: 'pending', claimExpiresAt: future })).toBe(false);
  });

  it('is not stranded when in-progress with a claim that has not expired', () => {
    expect(isClaimStranded({ status: 'in-progress', claimExpiresAt: future })).toBe(false);
  });

  it('is stranded when in-progress with a claim that has lapsed', () => {
    expect(isClaimStranded({ status: 'in-progress', claimExpiresAt: past })).toBe(true);
  });

  it('is stranded when active with a claim that has lapsed', () => {
    expect(isClaimStranded({ status: 'active', claimExpiresAt: past })).toBe(true);
  });

  it('is not stranded when the claim was never set', () => {
    // Never claimed, or a pre-migration row with no claim_expires_at yet.
    expect(isClaimStranded({ status: 'in-progress', claimExpiresAt: null })).toBe(false);
  });

  it('is never stranded once a job has left an active state, regardless of claimExpiresAt', () => {
    expect(isClaimStranded({ status: 'completed', claimExpiresAt: past })).toBe(false);
    expect(isClaimStranded({ status: 'failed', claimExpiresAt: past })).toBe(false);
    expect(isClaimStranded({ status: 'cancelled', claimExpiresAt: past })).toBe(false);
  });
});
