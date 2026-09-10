# Repeat the lease-resource backfill with the documented per-org loop

> **Status:** DRAFT — closes savvagent/otto-factory#119. **Revised after PR #135 review**:
> the original design's `resource NOT LIKE 'branch:%'` guard was Critical-severity wrong —
> `security-auditor`, `architect-reviewer`, and `pr-review-toolkit:code-reviewer` independently
> found it during the mandatory review trio, before merge. See Risks & Open Questions for the
> full history; the corrected design below is provenance-bounded (by `acquired_at` against
> `0027`'s `installed_on`), not shape-guarded.

## Assumptions

- 0027 already ran, and ran correctly, on every real deployment (local dev, `#[sqlx::test]`,
  and the actual Fly deployment, whose connecting role is a superuser per `docs/deploy/fly.md`
  — RLS is bypassed there regardless of `FORCE`). This spec does not re-verify that against
  production; it was already established by the issue and by
  `docs/specs/2026-09-10-migration-org-scoped-writes-design.md`'s sibling investigation of the
  same deployment shape.
- **`repo_leases.resource` has been genuinely free-form since 0027 itself shipped, not just a
  branch name.** 0027 is the same migration (`savvagent/otto-factory#115`,
  `docs/specs/2026-09-10-leases-generalize-resource-design.md`) that generalized `(repo,
  branch)` to `(repo, resource)` specifically so a team could serialize on "a staging slot or a
  migration lock" (`crates/of-mcp/src/server.rs`), not only a branch. `acquire_lease`
  (`crates/of-core/src/leases.rs`) validates only non-empty and `MAX_RESOURCE_LEN`; nothing
  requires a `branch:` prefix except by convention for the branch case specifically.
  `crates/of-core/tests/queue.rs`'s `leases_are_per_resource_not_just_branch` exercises a live
  `deploy:staging` lease today. **Any migration touching this column must not assume every
  unprefixed value is a legacy branch name** — a first draft of this spec made exactly that
  assumption and it was wrong.
- **0027 cannot have partially succeeded on any deployment.** Its own `UPDATE` carries no
  `WHERE` clause, and its preceding `ALTER TABLE repo_leases NO FORCE ROW LEVEL SECURITY`
  requires table ownership — a non-owner connecting role errors outright on that statement, the
  migration transaction rolls back, and `Db::migrate` returns `Err` before `0028` (or anything
  after it) ever runs. So there is no reachable deployment where 0027 applied but left some rows
  prefixed and others not: either every row present at that moment got prefixed, or the whole
  migration chain — including this one — never advanced past `0027`. A "safety net for a
  deployment where 0027 somehow missed some rows" is therefore not a real scenario; the only
  genuine risk is rows that **did not exist yet** when 0027 ran, which is a provenance
  (timestamp) question, not a value-shape question.
- Because of the previous point, any predicate for this migration must distinguish rows by
  provenance (`acquired_at` before 0027 installed) rather than by whether the value happens to
  start with `branch:` — the latter cannot tell "a legacy branch row 0027 already prefixed"
  apart from "a free-form resource legitimately taken after 0027 shipped", and conflating them
  silently corrupts live, currently-held leases (see Risks & Open Questions).
- This is precedent/consistency work, not a live data-correctness fix (the issue says so
  explicitly), and under the corrected, provenance-bounded predicate it is expected to be a true
  no-op on every real deployment — there should be no row whose `acquired_at` predates 0027's
  own installation and which 0027 failed to touch, precisely because 0027 cannot partially
  succeed. The value is a genuinely safe, worked example of the documented per-org-loop pattern
  in the migration history, not a live-data fix.

## Goal & Success Criteria

Add a new forward-only migration that backfills any `repo_leases.resource` row that both (a)
lacks the `branch:` prefix and (b) was `acquired_at` before `0027` was installed, using the
per-org loop pattern CLAUDE.md documents (explicit `org_id` predicate **and**
`set_config('app.org_id', ..., true)`), instead of 0027's `NO FORCE`/`FORCE ROW LEVEL SECURITY`
toggle — without editing 0027, which is already applied, and without touching any row created
under the free-form `resource` contract 0027 itself introduced.

- [x] A new migration file exists, numbered after the current latest (`0029`), that performs the
      backfill via the per-org loop, bounded by provenance (`acquired_at < installed_on` for
      migration version 27) rather than by the shape of the value.
- [x] The migration cannot touch a row created after 0027 shipped, regardless of whether that
      row's `resource` happens to look unprefixed (e.g. `deploy:staging`, `migration-lock`) —
      verified by a test that seeds exactly such a row and asserts it is unchanged.
- [x] A test exercises the migration's actual SQL — read from the shipped file via
      `include_str!`, not copied inline — under the FORCE-RLS-fallback shape (`of_app`, no
      automatic `app.org_id`), proving both that a genuinely pre-0027 row *is* matched and that a
      post-0027 free-form row is *not*.
- [x] `cargo test -p of-core --test isolation`, `cargo clippy --all-targets -- -D warnings`, and
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
- Any change to `acquire_lease`, `list_leases`, or any other application code. The corrected
  migration cannot touch a row written through the normal read/write path at all (every such row
  postdates 0027), so this really is a one-time, data-inert backfill for the narrow pre-0027
  case only — unlike the rejected first draft, which would have touched live application data.
- Any change to the three constraints in `CLAUDE.md` (repo-anchored coordination,
  substrate-not-workflow, coding-agent agnosticism) — this change touches none of them.

## §1 — The migration

`crates/of-core/migrations/0030_lease_resource_backfill.sql`, adapting CLAUDE.md's per-org-loop
template to `repo_leases`/`resource`, bounded by provenance rather than value shape:

```sql
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
  PERFORM set_config('app.org_id', '', true);
END $$;
```

**Why provenance, not shape.** A first draft of this migration guarded only by
`resource NOT LIKE 'branch:%'`. That guard cannot distinguish "0027 already prefixed this row"
from "this row's raw value legitimately has no `branch:` prefix because it was never a branch" —
and per the Assumptions section, the latter is not a rare edge case, it is every non-branch
lease taken since 0027 shipped (`deploy:staging`, a migration lock, and so on).
`_sqlx_migrations.installed_on` (populated by sqlx itself, one row per applied migration version)
gives a real provenance signal the value's shape cannot: any row `acquired_at` before 0027's own
`installed_on` genuinely predates the free-form-resource contract, and any row after it is
covered by that contract and must never be rewritten. Under the "0027 cannot partially succeed"
assumption above, this predicate is expected to always select zero rows in practice — but unlike
the shape guard, it is safe *by construction* even if that assumption is ever wrong for some
deployment shape not yet audited, because it structurally cannot match anything acquired after
the cutoff.

`orgs`, `repo_leases`, and `_sqlx_migrations` are all already-existing tables/schema objects
(`0001_identity.sql`, `0002_repos.sql`, sqlx's own migration bookkeeping); no schema change
accompanies this migration. `_sqlx_migrations` is not a tenant table (no `org_id` column, no RLS
policy), so reading it is unaffected by which deployment shape guard 2 is on.

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

Shape: seed two `repo_leases` rows per org, for **two** orgs (`acme` and `globex`) —

1. An unprefixed `resource = 'main'` with `acquired_at` set explicitly to one hour before
   `_sqlx_migrations`'s recorded `installed_on` for version 27, standing in for a genuinely
   pre-0027 row.
2. A free-form, unprefixed `resource = 'deploy:staging'` with a normal (post-migration, i.e.
   "now") `acquired_at`, standing in for a legitimate lease taken under the contract 0027 itself
   introduced — the exact case the rejected shape-only guard would have corrupted.

— then open an unpinned transaction, `SET LOCAL ROLE of_app` (the non-owner role FORCE binds
without needing table ownership — the same technique the existing negative test uses), run
0030's own migration **file**, read via `include_str!("../migrations/0030_lease_resource_backfill.sql")`
so the test exercises the artifact that actually ships rather than a copy that could drift from
it, commit, then read both orgs' rows back through each org's own pinned `Tx` and assert row 1
now reads `branch:main` while row 2 is untouched at `deploy:staging`, **for both orgs**.

The second org is not incidental: on this deployment's actual shape (RLS bypassed), the loop's
explicit `org_id = o` predicate — not the `set_config` half — is the guard doing the real work,
and a single-org test cannot tell "the loop reaches every org" apart from "the loop happened to
touch the one org it was seeded with." Seeding both orgs and asserting both independently closes
that gap; found in review (security-auditor) after the single-org version of this test initially
shipped.

This is deliberately the mirror image of the existing negative test in one respect (the per-org
loop's own `set_config` calls are what make the pre-0027 rows match, under a role RLS actually
binds, unlike a bare unscoped `UPDATE`) and a direct regression guard against the rejected first
draft in another (the post-0027 free-form rows must never move, in either org).

No `of-billing::classify` entry needed — this change adds no MCP tool.

## Error Handling & Edge Cases

- **A genuinely free-form, non-branch resource taken after 0027** (`deploy:staging`, a migration
  lock, a staging slot): never matches the migration's predicate, because its `acquired_at` is
  necessarily after 0027's `installed_on` — it did not exist for 0027 to have missed it. This is
  the case the corrected design exists to get right; the rejected shape-only guard would have
  silently rewritten it.
- **A branch already named `branch:main`** (the rare pre-existing collision 0027's own comment
  calls out): if such a row genuinely predates 0027, it was already turned into
  `branch:branch:main` unconditionally when 0027 ran, and that value already fails the
  `NOT LIKE 'branch:%'` half of this migration's predicate, so it is correctly left alone
  regardless of the provenance bound.
- **An org with zero `repo_leases` rows, or zero rows older than the cutoff:** the loop's
  `UPDATE` matches zero rows for that iteration and moves on; no error.
- **A fresh database, or any database created after 0027 already exists** (every `#[sqlx::test]`
  run, and any new deployment): `_sqlx_migrations`'s `installed_on` for version 27 is set at
  migration-apply time, and no `repo_leases` row can have an `acquired_at` earlier than that
  moment on a database where 0027 and every later migration ran in the normal forward sequence —
  so the migration is a true no-op everywhere except the one test that deliberately backdates a
  seeded row's `acquired_at` to exercise it.
- **`_sqlx_migrations` has no row for version 27** (a squashed or renumbered migration history —
  not a scenario this repo currently has, but the predicate is defensive against it): the plain
  `SELECT installed_on INTO cutoff` leaves `cutoff` NULL with no error (no `STRICT` clause), and
  `acquired_at < NULL` is NULL for every row, which Postgres treats as not-matched. The migration
  degrades to a safe no-op rather than raising or matching everything.
- **A prefixed value collides with an already-live lease** (only reachable if the "0027 cannot
  partially fail" reasoning in Assumptions is ever wrong for some deployment shape): `'branch:' ||
  resource` landing on a value that already has a live `(repo_id, resource)` row raises a Postgres
  unique violation against `repo_leases_live_key` (`0002_repos.sql`), failing the migration — and
  therefore `of-server`'s startup — loudly rather than corrupting data silently. Consistent with
  this repo's stated preference for errors that stop over errors that guess.

## Risks & Open Questions

- **Production verification is out of reach for this workflow.** The issue's acceptance criteria
  ask to "confirm against the actual deployed database... that the backfill produced the expected
  `branch:<name>` values." This workflow has no production database credentials. What's
  confirmable here: the migration is correct, provably safe against live free-form leases, and
  covered by a test that exercises its exact SQL (via `include_str!` of the shipped file) under
  the deployment shape where it would matter. The PR body should say this explicitly so a human
  with deploy access can do the one remaining check — `SELECT resource FROM repo_leases WHERE
  resource NOT LIKE 'branch:%' AND acquired_at < (SELECT installed_on FROM _sqlx_migrations WHERE
  version = 27)` should return zero rows, post-deploy — rather than it being silently dropped.
- **This migration will almost certainly run as a no-op everywhere it's ever actually applied**,
  since 0027 cannot have partially succeeded (see Assumptions) and the provenance bound cannot
  match anything acquired afterward. That is the intended, and now provably safe, outcome — the
  value is the precedent (a real, working, and *correct* example of the documented pattern in the
  migration history), not a live-data fix.
- **The design's original guard was Critical-severity wrong, caught by review before merge — this
  is the load-bearing history of this spec.** The first draft guarded by `resource NOT LIKE
  'branch:%'` alone, reasoning that any unprefixed row must be a legacy leftover 0027 somehow
  missed. That reasoning was internally consistent but rested on a false premise: it treated
  `resource` as if it were still branch-only, when 0027 — the very migration this backfill
  repeats — is what generalized it to free-form. On the actual deployment shape (RLS bypassed,
  the loop's own `org_id`/`set_config` scoping notwithstanding), that guard would have matched
  and silently rewritten every live non-branch lease in every org (`deploy:staging` →
  `branch:deploy:staging`, etc.), breaking the exact-string match `acquire_lease`/`renew_lease`/
  `release_lease` key on and letting two agents believe they both hold the same resource — the
  precise collision the lease primitive exists to make visible — with no error on either side.
  It also silently rewrote released rows kept as console history, and could in principle abort
  the migration outright (and thus block `of-server` from ever binding a port) if a repo held
  both a live bare `main` and a live `branch:main` lease simultaneously, since prefixing the
  former would then collide with the latter on `repo_leases_live_key`.

  This was found independently by three of PR #135's four review passes —
  `security-auditor` (blind-diff, Critical), `architect-reviewer` (Critical, citing the same
  root cause), and `pr-review-toolkit:code-reviewer` (Critical, plus the further observation that
  0027 cannot partially fail at all, which is what makes a provenance bound both sufficient and
  necessary) — none of which were given the design's own reasoning, only the diff. `rust-pro`'s
  pass, working from a different lens (idiomatic-Rust/sqlx-test correctness rather than the
  data-semantics of `resource`), did not catch it — a reminder that the mandatory trio's value is
  in running all three angles, not any single one.

  The corrected design in this revision (§1) replaces the shape guard with the provenance bound
  described above, verified safe against exactly the case that would have exposed the bug (a
  `deploy:staging` row seeded with a post-cutoff `acquired_at`, asserted unchanged in §3's test).
  Superseded by this fix: the previous risk note about a `branch:main`-named legacy branch
  colliding under the shape guard's ambiguity no longer applies to the *shape* guard (there isn't
  one anymore) — see Error Handling & Edge Cases for how that same named-collision case is
  handled under the corrected, provenance-bounded predicate instead.
