# Repeat the lease-resource backfill with the documented per-org loop

> **Status:** DRAFT — closes savvagent/otto-factory#119.

## Assumptions

- 0027 already ran, and ran correctly, on every real deployment (local dev, `#[sqlx::test]`,
  and the actual Fly deployment, whose connecting role is a superuser per `docs/deploy/fly.md`
  — RLS is bypassed there regardless of `FORCE`). This spec does not re-verify that against
  production; it was already established by the issue and by
  `docs/specs/2026-09-10-migration-org-scoped-writes-design.md`'s sibling investigation of the
  same deployment shape.
- Because 0027 already applied everywhere, a new migration repeating the same transformation
  must be idempotent against rows 0027 already touched — re-prefixing an already-prefixed
  `resource` would produce `branch:branch:main`, which is wrong. The guard is
  `resource NOT LIKE 'branch:%'`.
- This is precedent/consistency work, not a live data-correctness fix (the issue says so
  explicitly). The value is having the codebase's own migration history demonstrate the
  documented safe pattern, and a safety net for any environment where 0027 somehow left a row
  unprefixed.

## Goal & Success Criteria

Add a new forward-only migration that backfills any remaining unprefixed `repo_leases.resource`
row using the per-org loop pattern CLAUDE.md documents (explicit `org_id` predicate **and**
`set_config('app.org_id', ..., true)`), instead of 0027's `NO FORCE`/`FORCE ROW LEVEL SECURITY`
toggle — without editing 0027, which is already applied.

- [ ] A new migration file exists, numbered after the current latest (`0029`), that performs the
      backfill via the per-org loop.
- [ ] The migration is idempotent: re-running the same transformation against rows 0027 already
      prefixed does not double-prefix them.
- [ ] A test exercises the migration's actual SQL under the FORCE-RLS-fallback shape (`of_app`,
      no automatic `app.org_id`) and proves it *matches* the seeded row — the positive case that
      distinguishes this from `rls_scopes_a_migration_style_update_with_no_org_context`, which
      proves the opposite for a bare, unscoped statement.
- [ ] `cargo test -p of-core --test isolation`, `cargo clippy --all-targets -- -D warnings`, and
      `cargo fmt --all --check` all pass.

## Scope

**In:**
- One new migration file in `crates/of-core/migrations/`.
- One new test in `crates/of-core/tests/isolation.rs`, in the `rls_scopes_*` family.

**Out:**
- Editing `0027_lease_resource.sql` — forbidden outright; migrations are forward-only
  (Load-Bearing Invariant 12).
- Verifying against the actual deployed production database. That requires production
  credentials this workflow does not have; the issue's own acceptance criteria already record
  that 0027's backfill was previously verified correct there. Recorded as a known gap in Risks &
  Open Questions, not silently skipped.
- Any change to `acquire_lease`, `list_leases`, or any other application code — the `resource`
  column's read/write path is unaffected; this is a one-time backfill migration only.
- Any change to the three constraints in `CLAUDE.md` (repo-anchored coordination,
  substrate-not-workflow, coding-agent agnosticism) — this change touches none of them.

## §1 — The migration

`crates/of-core/migrations/0030_lease_resource_backfill.sql`, following the loop template CLAUDE.md
gives verbatim (the `tracker_bindings`/`trigger_label` example), adapted to `repo_leases`/`resource`:

```sql
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
  PERFORM set_config('app.org_id', '', true);
END $$;
```

The `resource NOT LIKE 'branch:%'` guard is the one addition beyond the CLAUDE.md template,
required because — unlike the `tracker_bindings` example, which was never actually run this way
before — this exact transformation was already applied once by 0027. Without the guard this
migration would be safe on the deployment shape it actually runs against (a no-op, since every row
already matches `'branch:%'`) but would silently corrupt data on the other shape if that shape is
ever reached with unprefixed rows present from some other source. `repo_leases_live_key`'s unique
index is `(repo_id, resource) WHERE released_at IS NULL`, and the guard does not interact with it:
the transformation is unconditionally rewriting a matched row's value, not comparing across rows.

`orgs` and `repo_leases` are both already-existing tables (`0001_identity.sql`,
`0002_repos.sql`); no schema change accompanies this migration.

## §2 — Tenant isolation

`repo_leases` is already a registered tenant table (`0007_rls.sql`'s `tenant_tables` array,
policy `repo_leases_tenant_isolation`) — this migration does not touch that registration, only the
migration-time write path against it, which is exactly the case CLAUDE.md's "migration that
touches a tenant table's data" section exists to cover. No new tenant table, so no new RLS policy
and no new `0007_rls.sql` entry.

## §3 — Testing

`crates/of-core/tests/isolation.rs` gets one new test, placed next to
`rls_scopes_a_migration_style_update_with_no_org_context` (the existing negative-case test in the
same family, added by `#111`/`savvagent/otto-factory#70`'s spec): `rls_scopes_the_lease_resource_
backfills_per_org_loop`.

Shape: seed a `repo_leases` row directly on the pool with an unprefixed `resource` (standing in
for a row that predates 0027, or that 0027 somehow missed), open an unpinned transaction, `SET
LOCAL ROLE of_app` (the non-owner role FORCE binds without needing table ownership — the same
technique the existing negative test uses), run 0030's own `DO $$ ... $$` block verbatim, commit,
then read the row back through a normal pinned `Tx` and assert it now reads `branch:main`.

This is deliberately the mirror image of the existing negative test: that one proves a *bare*
unscoped `UPDATE` matches zero rows under this role; this one proves the *per-org-loop* statement
— which supplies its own `app.org_id` — does match, under the identical role and the identical
absence of an automatically-set org context. The two tests together are the evidence for the rule
CLAUDE.md states: scoping the statement yourself is what makes it work under RLS-applies shapes,
not some property of the role or the table.

No `of-billing::classify` entry needed — this change adds no MCP tool.

## Error Handling & Edge Cases

- **A branch already named `branch:main`** (i.e., the rare pre-existing collision 0027's own
  comment calls out): 0027 already turned it into `branch:branch:main` unconditionally when it
  ran, and that row already fails the `NOT LIKE 'branch:%'` guard in this migration (it does
  start with `branch:`), so this migration correctly leaves it alone — it does not attempt to
  "fix" 0027's already-applied, intentional behavior.
- **An org with zero `repo_leases` rows:** the loop's `UPDATE` matches zero rows for that
  iteration and moves on; no error.
- **A fresh database that has never had an unprefixed row** (every `#[sqlx::test]` run, and any
  new deployment created after 0027 already exists): the migration is a true no-op everywhere
  except the one test that deliberately seeds a pre-0027-shaped row to exercise it.

## Risks & Open Questions

- **Production verification is out of reach for this workflow.** The issue's acceptance criteria
  ask to "confirm against the actual deployed database... that the backfill produced the expected
  `branch:<name>` values." This workflow has no production database credentials. What's
  confirmable here: the migration is correct, idempotent, and covered by a test that exercises its
  exact SQL under the deployment shape where it would matter. The PR body should say this
  explicitly so a human with deploy access can do the one remaining check (`SELECT resource FROM
  repo_leases WHERE resource NOT LIKE 'branch:%'` returning zero rows, post-deploy) rather than it
  being silently dropped.
- **This migration will almost certainly run as a no-op everywhere it's ever actually applied**,
  since 0027 already did the real work. That is the intended outcome, not a sign the migration is
  pointless — the value is the precedent (a real, working example of the documented pattern in the
  migration history) and the safety net for an edge case (a missed row, a differently-shaped
  future deployment) that is real but currently unobserved.
- **The idempotency guard (`resource NOT LIKE 'branch:%'`) cannot distinguish "0027 already
  transformed this row" from "this row's raw, pre-migration branch name was already literally
  `branch:<something>`.** 0027's own comment explains why it prefixed unconditionally rather than
  guarding: a legacy branch named `branch:main` would, under a guard, end up sharing a string with
  a genuine `main` branch's lease once that one gets prefixed — 0027 accepted a cosmetic double
  prefix (`branch:branch:main`) specifically to keep such rows distinct, and called that "correct,
  not merely simpler." Because 0027's own `UPDATE` carries no `WHERE` clause, its only two possible
  outcomes on any deployment are "every existing row gets prefixed" (the migrating role could see
  the rows) or "zero rows get prefixed" (RLS blocked all of them) — never a partial subset. So the
  one scenario where 0030's guard's ambiguity is live is the same one it exists to catch: 0027
  failed entirely on some deployment. In that compound case, if a repo has both an unreleased lease
  on a real `main` branch (raw resource `main`) and an unreleased lease on a branch genuinely named
  `branch:main` (raw resource `branch:main`), 0030 prefixes the first and, misreading the second as
  already-done, leaves it — producing two live rows with the same `(repo_id, resource)`. This does
  not silently corrupt data: `repo_leases_live_key` (`0002_repos.sql`) is a unique index on exactly
  that pair `WHERE released_at IS NULL`, so the colliding `UPDATE` fails with a Postgres unique-
  violation and the migration does not apply — a loud failure, not silent corruption, surfaced at
  deploy time on the one deployment shape where it could ever occur. A guard that could tell these
  two cases apart would need a signal this schema doesn't carry (no schema-version column, no lease-
  format marker), so the choice here is the same shape as 0027's own trade-off: accept a narrow,
  loud-failure corner case in exchange for the guard doing its job everywhere else, rather than
  reintroducing 0027's real, silent-corruption-in-the-common-case failure mode (unconditional
  re-prefixing of rows 0027 already touched) to close it.
