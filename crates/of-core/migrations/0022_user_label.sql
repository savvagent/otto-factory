-- The words a credential vault shows beside an account's passkey.
--
-- Every first passkey used to be named with one constant string, so two
-- accounts on this site were indistinguishable in the operating system's
-- picker. The label is what makes them distinguishable, and it lives on the
-- account rather than on the credential so a second key gets the same name.
--
-- It is a *name*, never an identifier: nothing looks an account up by it, and
-- it carries no unique index on purpose -- a cosmetic collision between two
-- accounts must not become a failed signup on the one path that has no fallback.
--
-- `users` is not a tenant table -- no org_id, absent from the tenant_tables
-- array in 0007_rls.sql -- so this column needs no RLS policy.
ALTER TABLE users ADD COLUMN label TEXT;

-- The word lists below duplicate of_core::labels, and the duplication is
-- deliberate rather than an oversight. This backfill runs once per cluster and
-- never again; from then on every row is written by the Rust generator. The two
-- copies therefore cannot drift in any way anybody can observe -- and the
-- alternative, backfilling from application code, would mean a migration that
-- only half-applies without a running binary. A short list is enough here for
-- the same reason: it names the handful of rows that predate the column.
--
-- `WHERE label IS NULL` is redundant the one time this runs -- the column was
-- added empty two statements ago -- and it is here so the paragraph above is
-- enforced by the statement rather than only by the migration runner's bookkeeping.
UPDATE users
SET label =
  (ARRAY['amber','brisk','clever','golden','lively','nimble','quiet','rugged'])
    [floor(random() * 8)::int + 1]
  || '-' ||
  (ARRAY['acorn','beacon','cedar','falcon','harbor','meadow','ridge','willow'])
    [floor(random() * 8)::int + 1]
  || '-' ||
  (10 + floor(random() * 90)::int)::text
WHERE label IS NULL;

ALTER TABLE users ALTER COLUMN label SET NOT NULL;
