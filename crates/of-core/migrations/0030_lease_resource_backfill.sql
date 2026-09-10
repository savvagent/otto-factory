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
-- 0027's own UPDATE carries no WHERE clause and its NO FORCE step requires the migrating
-- role to be the table's owner, a member of the owning role, or a superuser (ALTER TABLE
-- errors outright otherwise), so 0027 cannot have partially succeeded: either every row
-- present at that moment got prefixed (owner/superuser: NO FORCE lets it see and rewrite
-- every row, or RLS was already bypassed), or the whole migration -- and every one after
-- it, including this one -- never ran at all. There is therefore no reachable deployment
-- where an unprefixed *pre-0027* row survives for this migration to find. The only
-- predicate that can safely repeat 0027's transformation is one bounded by provenance
-- rather than by the value's shape: `acquired_at` before 0027 was installed.
--
-- `cutoff` reads sqlx's own `_sqlx_migrations` bookkeeping table rather than a value this
-- product owns -- the one place in this codebase that does. That is deliberate: it is the
-- only durable record of *when* 0027 actually ran on a given database, which is exactly
-- what a provenance bound needs and nothing `repo_leases` itself carries (no format
-- marker, no schema-version column). If that row is ever absent (a squashed/renumbered
-- migration history), the plain `SELECT ... INTO cutoff` leaves `cutoff` NULL with no
-- error, `acquired_at < NULL` matches zero rows for every comparison, and this migration
-- degrades to a safe no-op rather than misbehaving.
--
-- Under the reasoning above the predicate always selects zero rows in the field -- this
-- migration is expected to be a true no-op everywhere -- but unlike a shape guard it is
-- safe *by construction*: `acquired_at` is written only by `repo_leases`'s own `DEFAULT
-- now()` (crates/of-core/src/leases.rs never sets or backdates it), so no resource written
-- under the free-form contract 0027 itself introduced can ever have an `acquired_at`
-- earlier than 0027's own installation. That holds regardless of whether the "0027 cannot
-- partially fail" reasoning above turns out to be wrong for some deployment shape not yet
-- audited. (If this UPDATE ever did match a live row, and that row's newly-prefixed value
-- collided with an already-live `branch:<same>` lease on the same repo, the statement
-- would raise a Postgres unique violation against `repo_leases_live_key`
-- (0002_repos.sql) and this migration -- and therefore `of-server`'s startup -- would fail
-- loudly rather than corrupt data silently. Unreachable under the reasoning above, but
-- worth naming: failing loudly here beats guessing, per this repo's own error-handling
-- convention.) See docs/specs/2026-09-10-lease-resource-backfill-org-loop-design.md's
-- Risks & Open Questions for the full history of this reasoning, including the rejected
-- shape-only guard's collision risk.
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
