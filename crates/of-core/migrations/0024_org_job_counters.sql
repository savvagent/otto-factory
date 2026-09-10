-- Jobs::stats' completed/failed counts were a COUNT(*) FILTER scan over
-- every job an org has ever run - unbounded over the org's lifetime, unlike
-- pending/in-progress/active/blocked, which are bounded by current queue
-- depth. The console overview polls this every 30 seconds (#57/#58), so a
-- busy org's whole history got rescanned roughly 2,880 times a day for
-- numbers that move by a handful of jobs between polls.
--
-- orgs is not one of 0007_rls.sql's tenant_tables - it is the tenant, not
-- tenant-scoped data, and carries no org-scoping RLS policy. So, unlike
-- 0020's tenant-table UPDATE (see #70), this one needs no `app.org_id`
-- pinned to see every row it backfills.
ALTER TABLE orgs
  ADD COLUMN jobs_completed_total bigint NOT NULL DEFAULT 0,
  ADD COLUMN jobs_failed_total    bigint NOT NULL DEFAULT 0;

UPDATE orgs SET
  jobs_completed_total = (
    SELECT count(*) FROM jobs WHERE jobs.org_id = orgs.id AND jobs.status = 'completed'
  ),
  jobs_failed_total = (
    SELECT count(*) FROM jobs WHERE jobs.org_id = orgs.id AND jobs.status = 'failed'
  );
