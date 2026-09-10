-- A claim needs an expiry so a crashed agent's job becomes claimable again
-- instead of staying in-progress forever — the same reasoning 0002_repos.sql
-- states for repo_leases, applied to jobs (see savvagent/otto-factory#65).
--
-- Nullable, with no backfill: a job already in-progress/active when this
-- migration runs gets no retroactive TTL. NULL never compares <= now(), so
-- claim_jobs's reap step and ready()'s claimable check both simply never
-- treat such a row as expired — nothing already claimed is silently reaped
-- the moment this ships. Only a claim taken through the new claim_jobs sets
-- this column going forward; a job stuck in-progress from before this
-- migration is recoverable exactly as it is today, via repend_job, until an
-- agent repends or completes/fails it and the next claim on it carries a
-- real expiry.
ALTER TABLE jobs
    ADD COLUMN claim_expires_at timestamptz;
