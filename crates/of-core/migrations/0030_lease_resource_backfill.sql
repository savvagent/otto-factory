-- 0027_lease_resource.sql backfilled repo_leases.resource ('branch:' || resource) using
-- the NO FORCE / FORCE ROW LEVEL SECURITY toggle pattern, before CLAUDE.md's tenant-
-- isolation section documented that toggle as "not recommended" in favor of a per-org
-- loop with both an explicit org_id predicate and set_config('app.org_id', ..., true).
-- 0027 is an already-applied migration, so it is not edited in place.
--
-- An earlier draft of this migration guarded by `resource NOT LIKE 'branch:%'`, matching
-- any row that didn't already look prefixed. That is wrong: 0027 is the same migration
-- that generalized `resource` from "always a branch name" to free-form ("a branch, a
-- staging slot, a migration lock" -- crates/of-mcp/src/tools/coord.rs's acquire_lease
-- description), and only the *branch* case uses the `branch:<name>` form. Every
-- non-branch lease taken since 0027 shipped -- 'deploy:staging', 'migration-lock', a
-- deliberate bare 'main' held alongside 'branch:main' -- legitimately has no prefix and
-- is not a leftover. A shape-based guard would silently rewrite every one of them on the
-- next deploy, breaking the exact-string match acquire_lease/renew_lease/release_lease
-- key on (crates/of-core/src/leases.rs) and falsifying retained lease history in the
-- process -- caught in PR review, not shipped.
--
-- 0027's own UPDATE carries no WHERE clause and its NO FORCE step requires table
-- ownership (ALTER TABLE errors outright for a non-owner), so 0027 cannot have partially
-- succeeded: either every row present at that moment got prefixed, or the whole migration
-- -- and every one after it, including this one -- never ran at all. There is therefore no
-- reachable deployment where an unprefixed *pre-0027* row survives for this migration to
-- find. The only predicate that can safely repeat 0027's transformation is one bounded by
-- provenance rather than by the value's shape: `acquired_at` before 0027 was installed.
-- Under the reasoning above that always selects zero rows -- this migration is expected
-- to be a true no-op everywhere -- but unlike a shape guard it is safe *by construction*:
-- it cannot match a resource written under the free-form contract 0027 itself introduced,
-- regardless of whether the "0027 cannot partially fail" reasoning above turns out to be
-- wrong for some deployment shape not yet audited. See
-- docs/specs/2026-09-10-lease-resource-backfill-org-loop-design.md's Risks & Open
-- Questions for the full history of this reasoning, including the shape-guard's rejected
-- collision risk.
DO $$
DECLARE
  o uuid;
  cutoff timestamptz;
BEGIN
  SELECT installed_on INTO cutoff FROM _sqlx_migrations WHERE version = 27;

  FOR o IN SELECT id FROM orgs LOOP
    PERFORM set_config('app.org_id', o::text, true);
    UPDATE repo_leases
      SET resource = 'branch:' || resource
      WHERE org_id = o
        AND resource NOT LIKE 'branch:%'
        AND acquired_at < cutoff;
  END LOOP;
  -- Leaving `app.org_id` set past the loop would apply it to any later statement in
  -- this same migration file on an RLS-applying shape -- restore it to unset.
  PERFORM set_config('app.org_id', '', true);
END $$;
