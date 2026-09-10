-- 0027_lease_resource.sql backfilled repo_leases.resource ('branch:' || resource) using
-- the NO FORCE / FORCE ROW LEVEL SECURITY toggle pattern, before CLAUDE.md's tenant-
-- isolation section documented that toggle as "not recommended" in favor of a per-org
-- loop with both an explicit org_id predicate and set_config('app.org_id', ..., true).
-- 0027 is an already-applied migration, so it is not edited in place. On this
-- deployment's actual shape (the connecting role is a superuser, so RLS is bypassed
-- regardless of FORCE) 0027's backfill already ran correctly -- verified against the
-- deployed database, every row reads 'branch:<name>' with no unprefixed leftovers -- so
-- there is nothing left to fix. This migration exists for precedent/consistency with the
-- documented pattern and as a safety net for a deployment where guard 2 is on the
-- FORCE-RLS-fallback shape rather than superuser-bypass: 0027's own UPDATE carries no
-- WHERE clause, so on that shape it is all-or-nothing per deployment -- either every row
-- got prefixed (if the migrating role owned the table) or none did (if RLS blocked it
-- entirely, since no app.org_id is ever set for a migration). This migration's
-- `resource NOT LIKE 'branch:%'` guard makes it a no-op wherever 0027 already did the
-- work, same as this migration's own reproduction test shows.
--
-- The guard has one known, accepted residual risk: it cannot distinguish "0027 already
-- prefixed this row" from "this row's raw branch name was already literally
-- 'branch:<something>'". 0027 prefixed unconditionally rather than guarding for exactly
-- this reason -- a legacy branch named e.g. 'branch:main' would, under a guard, share a
-- string with a genuine 'main' branch's lease once that one gets prefixed, whereas
-- unconditional prefixing keeps them distinct via a double 'branch:branch:main'. That
-- ambiguity is only live in the narrow compound case this migration exists to cover in
-- the first place (0027 failed entirely on some deployment) AND a colliding branch name
-- is present. If both rows are simultaneously live it fails loudly: repo_leases_live_key
-- (0002_repos.sql) is a unique index on (repo_id, resource) WHERE released_at IS NULL,
-- so the second UPDATE raises a unique-violation and this migration does not silently
-- apply. See docs/specs/2026-09-10-lease-resource-backfill-org-loop-design.md's Risks &
-- Open Questions for the full reasoning; a guard that could tell the two cases apart
-- would need a signal this schema doesn't carry.
DO $$
DECLARE
  o uuid;
BEGIN
  FOR o IN SELECT id FROM orgs LOOP
    PERFORM set_config('app.org_id', o::text, true);
    UPDATE repo_leases
      SET resource = 'branch:' || resource
      WHERE org_id = o AND resource NOT LIKE 'branch:%';
  END LOOP;
  -- Leaving `app.org_id` set past the loop would apply it to any later statement in
  -- this same migration file on an RLS-applying shape -- restore it to unset.
  PERFORM set_config('app.org_id', '', true);
END $$;
