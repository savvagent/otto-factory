-- `ALTER TABLE ... RENAME COLUMN` carries the partial unique index's definition
-- forward to the new column name (Postgres rewrites dependent index and constraint
-- definitions on a column rename), so `repo_leases_live_key` reads
-- `(repo_id, resource) WHERE released_at IS NULL` after this statement with no
-- further action -- verified; a DROP/CREATE of that index would only take an
-- ACCESS EXCLUSIVE lock to reproduce what this statement already does.
ALTER TABLE repo_leases RENAME COLUMN branch TO resource;

-- repo_leases is a tenant table with FORCE ROW LEVEL SECURITY (0007_rls.sql), whose
-- policy is `USING (org_id = current_org())`. A migration connection never sets
-- `app.org_id` (there is no tenant transaction to pin), so `current_org()` reads NULL
-- and the policy's predicate is never TRUE for any row -- a bare UPDATE below would
-- silently affect zero rows on any deployment where FORCE actually matters (managed
-- Postgres, where the migrating role is the table's owner and is neither a superuser
-- nor BYPASSRLS). Locally and in #[sqlx::test], the connecting role is a superuser and
-- bypasses RLS regardless of FORCE, which is exactly what would have hidden this: the
-- backfill would have looked correct everywhere except the one deployment shape it has
-- to work on. Suspending FORCE only around this statement, as the table's owner, lets
-- the owner's own migration see and rewrite every row; it is restored immediately after.
ALTER TABLE repo_leases NO FORCE ROW LEVEL SECURITY;

-- Existing rows predate the resource convention and are bare branch names; rewrite them
-- to `branch:<name>` so their meaning is preserved under the renamed column. Prefixed
-- unconditionally rather than guarded by `WHERE resource NOT LIKE 'branch:%'`: a git
-- branch can itself be named `branch:main`, and a guard would map it to the same string
-- as a lease on `main`, colliding on the unique index below. Prefixing unconditionally
-- keeps every pre-existing row distinct, at the cost of a rare double `branch:branch:`
-- for the one branch name that already collided with the convention -- correct, not
-- merely simpler.
UPDATE repo_leases SET resource = 'branch:' || resource;

ALTER TABLE repo_leases FORCE ROW LEVEL SECURITY;
