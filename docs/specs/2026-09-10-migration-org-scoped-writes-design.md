# Migration-time unscoped writes against tenant tables

> **Status:** DRAFT — fixes `savvagent/otto-factory#70`: `0020_rename_trigger_label_default.sql`'s
> relabeling `UPDATE` runs with no `app.org_id` set, which is invisible under this deployment's
> current superuser connecting role but would silently match zero rows on the FORCE-RLS-fallback
> deployment shape. Investigation found no stale data to correct; this spec documents the general
> rule and adds a regression test, not a data migration.

## Goal & Success Criteria

A future migration author who writes a bare `UPDATE`/`DELETE` against a tenant table's data (not
its schema) has, in the same file they are about to copy the pattern from, both the rule that makes
this dangerous and a passing test that reproduces the exact failure mode. The specific defect in
`0020_rename_trigger_label_default.sql` is confirmed harmless in production as deployed today, and
that confirmation — not a guess — is what lets this spec skip a corrective data migration.

- `CLAUDE.md` states the rule: a migration that rewrites tenant-table data cannot rely on a bare
  `UPDATE`/`DELETE`, because `Db::migrate` never assumes `of_app` or sets `app.org_id`, so on the
  FORCE-RLS-fallback deployment shape every such statement silently matches zero rows, for every
  tenant, permanently.
- `crates/of-core/tests/isolation.rs` gains one test in the `rls_scopes_*` family that reproduces
  this exact failure: seed a row the way pre-0020 data would look, run 0020's own `UPDATE` text
  under `of_app` with no `app.org_id` set, assert zero rows affected.
- No corrective migration ships, because production `tracker_bindings` was confirmed empty (see
  Premise corrections) — there is nothing to relabel.
- All 26 applied migrations are audited for the same shape; the audit and its result are recorded
  here rather than repeated by the next reader.
- `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --all
  --check` all pass.

## Premise corrections

The originating job description (GH issue #70) is accurate about the mechanism — it is wrong about
nothing — but two things the issue could not have known without querying the deployed database and
re-reading `docs/deploy/fly.md` change what "fix it" means in practice:

1. **Production `tracker_bindings` has zero rows, not some rows mislabeled.** Queried directly
   against `otto-factory-mcp-db` (`otto_factory` database) on 2026-09-10:
   ```
   SELECT count(*) FROM tracker_bindings WHERE trigger_label = 'dark-factory';  -- 0
   SELECT trigger_label, count(*) FROM tracker_bindings GROUP BY trigger_label; -- (0 rows)
   ```
   No tracker connection has been configured against this deployment yet, so `tracker_bindings` is
   empty outright. There is no stale `'dark-factory'` row anywhere for 0020's `UPDATE` to have
   missed. Task item 2 from the issue ("if so, a forward migration...") is therefore vacuous — "if
   so" resolves to no.

2. **This deployment is not on the FORCE-RLS-fallback shape the bug requires.**
   `docs/deploy/fly.md` §"Tenant isolation on a dedicated Postgres instance" records that
   `otto-factory-mcp-db` is a dedicated, standalone Fly Postgres instance whose connecting role,
   `otto_factory_mcp`, is a **superuser** — confirmed again directly (`SELECT
   current_setting('is_superuser')` → `on`). A superuser bypasses row-level security unconditionally,
   `FORCE ROW LEVEL SECURITY` included; `Db::migrate` runs every migration statement on the raw pool
   (`crates/of-core/src/db.rs:89-95`), which for this deployment means every migration — 0020
   included — has always run as that superuser, RLS never in effect at all. `0020`'s `UPDATE` was
   never actually at risk of the failure the issue describes, in this specific deployment, as
   currently configured.

   `fly.toml`'s own header comment ("a dedicated `otto_factory` database ... on the shared
   `savvagent-pg` managed Postgres cluster") is stale and contradicts `docs/deploy/fly.md`, which
   explicitly documents that the shared-cluster plan was abandoned in favor of a dedicated instance
   specifically *because* the shared cluster's role model does not grant `CREATEROLE`. This
   inconsistency is real and confusing — it is what this investigation had to resolve by querying
   the live database rather than trusting either document — but it is a pre-existing documentation
   drift orthogonal to #70's migration-safety question. Filed separately
   (`savvagent/otto-factory#106`) rather than folded into this PR, per the
   Stop & Escalate guidance against silently widening scope when the same-shaped-but-different bug
   surfaces elsewhere. `docs/deploy/fly.md` is the accurate account and is not changed by this spec.

Neither correction weakens the underlying finding: `docs/deploy/fly.md` itself calls the
FORCE-RLS-fallback shape "the fallback path `of-server` supports if this ever moves to managed
Postgres" — a live, documented possibility, not a hypothetical. The bug is real, general, and worth
guarding against; it simply has not yet had an opportunity to corrupt data in *this* deployment.

## Scope

**In:**

- A `CLAUDE.md` rule, in the same register as the existing "A privilege granted to `of_app` is not a
  protection" paragraph, stating that migration-time data rewrites cannot rely on RLS having any org
  context to filter with, and naming the two ways to write one safely (see §2).
- One new regression test in `crates/of-core/tests/isolation.rs`, in the `rls_scopes_*` family,
  reproducing the exact failure mode using 0020's own `UPDATE` statement text.
- The audit of all 26 applied migrations for the same shape (§3), recorded in this spec rather than
  left for the next investigator to redo.
- A follow-up GH issue for the `fly.toml`/`docs/deploy/fly.md` documentation drift found during
  investigation (Premise correction 2), filed but not fixed in this PR.

**Out:**

- **No corrective data migration.** Premise correction 1 establishes there is nothing to correct.
  Writing one anyway to "be thorough" would add a migration that does nothing on every database that
  will ever apply it (fresh test databases start empty; production has already relabeled its zero
  rows) — dead weight with no test able to distinguish it from a bug.
- **No change to `Db::migrate`'s connection shape.** Making migrations run through `Db::begin`-style
  role assumption is a structural change with its own tradeoffs (which org would a schema migration
  even assume?) that this narrowly-scoped fix does not need to make: the two safe patterns in §2 both
  work within `Db::migrate`'s existing unscoped-connection shape.
- **No fix to the `fly.toml` header comment.** Real, but a different bug in a different file with no
  runtime effect — corrected via the filed follow-up issue instead of scope creep on this PR (see
  Premise correction 2).
- **No change to `0020_rename_trigger_label_default.sql` itself.** Migrations are forward-only; an
  applied migration is never edited (`CLAUDE.md`, Non-Negotiable Rule 6 in
  `otto-factory-development`). Nothing about this spec asks for that.

## Public-interface changes

None. No MCP tool, console route, OAuth/discovery endpoint, config key, or migration is added,
renamed, or removed. `docs/clients/matrix.md` is unaffected.

| Surface | Change |
|---|---|
| MCP | none |
| Console REST | none |
| OAuth/discovery | none |
| Config | none |
| Schema | none — no migration in this change |

## §1 The rule, in `CLAUDE.md`

Add a new paragraph to the "Tenant isolation — the rule that outranks convenience" section,
immediately after the existing "A privilege granted to `of_app` is not a protection" paragraph
(which it continues the reasoning of — that paragraph is about grants not surviving the
managed-Postgres shape; this one is about migrations not carrying any org context through it at
all):

> **A migration that touches a tenant table's data needs its own `org_id` scoping — RLS is not
> available to supply one.** `Db::migrate` runs every migration statement on the connection pool
> directly, never through `Db::begin`: `app.org_id` is never set and `of_app` is never assumed.
> What that means depends on which side of guard 2 the connecting role is on. If it is a
> superuser or has `BYPASSRLS` — this deployment's actual shape today, per `docs/deploy/fly.md`
> — RLS does not apply at all, `FORCE` included; an unscoped `UPDATE`/`DELETE` against a tenant
> table silently rewrites **every org's matching rows in one statement**, a cross-tenant write,
> not a no-op. Otherwise RLS does apply — every tenant table is `ENABLE`/`FORCE ROW LEVEL
> SECURITY` unconditionally (`0007_rls.sql`, `0008_audit.sql`, `0011_trackers.sql`), which holds
> whether the migrating role happens to own the table (where `FORCE` is what removes its
> exemption) or not (where a non-owner has no exemption to begin with) — and `current_org()` is
> NULL for the statement's entire lifetime, so `org_id = current_org()` is never true and the
> statement silently matches **zero rows, for every tenant**, forever. `savvagent/otto-factory#70`
> is the second outcome: it would have hit exactly this had
> `0020_rename_trigger_label_default.sql` run under a non-bypassing role; under this deployment's
> actual (bypassing) role it hit the first outcome instead, harmlessly, because that rewrite was
> genuinely meant to apply the same way to every org.
> `rls_scopes_a_migration_style_update_with_no_org_context` in `tests/isolation.rs` reproduces the
> zero-rows outcome directly, as a non-owner role — which needs no `FORCE` to be bound, and is the
> only path a superuser-connected `#[sqlx::test]` can exercise; both paths produce the same zero.
>
> The one pattern that is safe under every shape, for any migration that must rewrite existing
> tenant-table data — org-agnostic or not: loop over every org and give the statement **both** an
> explicit `org_id` predicate (what saves it when RLS is bypassed) **and**
> `set_config('app.org_id', ..., true)` (what saves it when RLS applies) — a migration author
> cannot know in advance which deployment will run it, so both together, not either alone.
> `Db::migrate` runs raw SQL with no parameter binding, so this is plpgsql, not a bound query:
>
> ```sql
> DO $$
> DECLARE
>   o uuid;
> BEGIN
>   FOR o IN SELECT id FROM orgs LOOP
>     PERFORM set_config('app.org_id', o::text, true);
>     UPDATE tracker_bindings SET trigger_label = 'otto-factory'
>       WHERE org_id = o AND trigger_label = 'dark-factory';
>   END LOOP;
> END $$;
> ```
>
> Temporarily toggling `ALTER TABLE <table> NO FORCE ROW LEVEL SECURITY` / `... FORCE ROW LEVEL
> SECURITY` around an unscoped statement is not recommended: it only helps when the migrating role
> owns the table, and a forgotten restore is caught by `Db::verify_tenant_isolation` at the next
> boot only on the fallback shape — on the `of_app`-assumable shape (this deployment's actual
> shape today) the startup check itself runs as `of_app`, which owns nothing, so `FORCE` is not
> load-bearing for that check either and a forgotten restore is not caught there at all. The loop
> above has no such gap. `TRUNCATE` against a tenant table must never appear in a migration —
> Postgres has no RLS policy class for `TRUNCATE` at all, so neither guard covers it, ever.
> `COPY ... FROM` is refused outright when RLS applies ("`COPY FROM not supported with row-level
> security`"), but runs exactly like an unscoped `INSERT` when RLS is bypassed — it needs the same
> per-org treatment as the loop above, not `TRUNCATE`'s blanket ban. A schema-only change
> (`ALTER TABLE ... ADD COLUMN`, a new `DEFAULT`, an index) is unaffected — none of this
> constrains a table's own schema, only what its rows may be read, written, or matched against.

**Revision notes.** Two rounds of PR review, both caught before merge:

- **Round 1** — an earlier draft claimed the table owner is exempt from RLS "regardless of
  `FORCE`" (backwards: `FORCE` exists specifically to remove that exemption), and prescribed a
  per-org loop with `set_config` alone and no explicit `org_id` predicate — which scopes nothing
  when RLS is bypassed, so it would have shipped a "safe pattern" that silently writes every
  org's rows under this deployment's actual connecting role.
- **Round 2** — the round-1 fix introduced a new, subtler overclaim: it said a forgotten restore
  of `ALTER TABLE ... FORCE ROW LEVEL SECURITY` is "caught at the next boot" by
  `Db::verify_tenant_isolation`, stated unconditionally. Empirically false on this deployment's
  actual shape: `verify_tenant_isolation`'s own check runs as `of_app` (`Db::begin`'s effective
  role), which owns nothing in either shape (confirmed: `pg_has_role('of_app', tracker_bindings'
  owner, 'USAGE')` → `f`), so `FORCE` is never load-bearing for *that check* — the safety net
  only exists on the fallback shape, where the effective role is the connecting role itself. The
  round-1 fix also left the `$<n>` bind-parameter syntax in a context (`Db::migrate` runs raw
  SQL) where no such binding is possible, and the test's own comments still credited `FORCE` for
  an outcome `of_app`'s plain `ENABLE`-only exposure already produces (verified: dropping `FORCE`
  from a non-owner-granted table changes nothing). The text above replaces the two-pattern
  prescription with the one pattern verified safe under every shape (the loop, with both the
  predicate and `set_config` always present together) and drops the `NO FORCE`/`FORCE` toggle to
  a discouraged alternative with its real caveat stated. Every claim in the current text was
  checked empirically against the local Postgres before this revision was committed — see the
  transcript-equivalent checks summarized in the paragraph itself (ownership queries, a `COPY
  FROM` run under both a bypassing and a non-bypassing role, the loop pattern executed end to
  end).

## §2 The regression test

`crates/of-core/tests/isolation.rs`, added after `rls_scopes_tracker_bindings` and before the
"guard on the guard" comment block that introduces the `SET LOCAL ROLE of_app`-by-hand tests below
it — this test belongs to that same family and reuses its established shape (`begin_unpinned`, `SET
LOCAL ROLE of_app`, no `app.org_id`):

```rust
/// **What 0020_rename_trigger_label_default.sql got wrong, reproduced.**
///
/// That migration relabels `tracker_bindings.trigger_label` with a bare
/// `UPDATE ... WHERE trigger_label = 'dark-factory'`. `Db::migrate` runs every
/// migration on the raw pool (`db.rs`) — never `Db::begin`, so never `SET
/// LOCAL ROLE of_app` and never `set_config('app.org_id', …)`. On this
/// deployment's actual connecting role (a superuser, confirmed against
/// `docs/deploy/fly.md`), that is invisible in the opposite direction from
/// what this test demonstrates: a superuser bypasses RLS outright, so the
/// statement would touch *every* org's matching rows, not none. `SET LOCAL
/// ROLE of_app` below drops to a role RLS actually binds — `of_app` owns
/// nothing, so it needs no `FORCE` to lose the exemption a table owner would
/// otherwise get; the FORCE-RLS-fallback deployment shape hits the same zero
/// for a related but distinct reason (that role *is* the owner, which is what
/// `FORCE` binds — see `CLAUDE.md`), and this test, run from a superuser
/// connection, can only exercise the non-owner path. Either way
/// `current_org()` stays NULL for the statement's entire lifetime, so
/// `org_id = current_org()` is never true and the UPDATE silently matches
/// zero rows, for every tenant, forever.
///
/// `savvagent/otto-factory#70`. No corrective data migration accompanies this
/// test — `docs/specs/2026-09-10-migration-org-scoped-writes-design.md`
/// (Premise corrections, dated 2026-09-10) records that this deployment's own
/// `tracker_bindings` was empty at investigation time, and that migrations
/// there currently run as the superuser described above. Neither fact is
/// re-verified by this test; it guards the deployment shape where the second
/// one stops being true.
#[sqlx::test]
async fn rls_scopes_a_migration_style_update_with_no_org_context(pool: PgPool) {
    let db = db(pool);
    let a = tenant(&db, "acme", "git@github.com:acme/api.git").await;

    // Seed a row the way a pre-0020 database would have had one: written
    // directly on the pool (bypassing `Tx`, standing in for data a migration
    // — not application code — would be rewriting).
    sqlx::query(
        "INSERT INTO tracker_bindings (org_id, repo_id, provider, external_ref, trigger_label) \
         VALUES ($1, $2, 'github', 'acme/api', 'dark-factory')",
    )
    .bind(a.org)
    .bind(a.repo)
    .execute(db.pool())
    .await
    .unwrap();

    // `of_app` owns nothing here, so it needs no `FORCE` to be bound by RLS —
    // a non-owner grantee role is never exempt. Neither superuser nor
    // BYPASSRLS, and no `app.org_id` ever set — a schema migration has no
    // tenant to set it to.
    let mut tx = db.begin_unpinned().await.unwrap();
    sqlx::query("SET LOCAL ROLE of_app")
        .execute(&mut *tx)
        .await
        .unwrap();

    // 0020's own statement, verbatim.
    let updated = sqlx::query(
        "UPDATE tracker_bindings SET trigger_label = 'otto-factory' \
         WHERE trigger_label = 'dark-factory'",
    )
    .execute(&mut *tx)
    .await
    .unwrap()
    .rows_affected();
    tx.commit().await.unwrap();

    assert_eq!(
        updated, 0,
        "a bare UPDATE against an RLS-active tenant table with no app.org_id \
         set is exactly the failure this test exists to keep visible — if \
         this starts affecting rows, something about the deployment's \
         isolation shape changed and every migration written under the old \
         assumption needs re-auditing"
    );

    // Positive control: the row is still there, still stale. The zero above
    // is the missing org context, not a missing or already-touched row —
    // the distinction CLAUDE.md's own guard-1/guard-2 split insists a test
    // in this family has to make.
    let mut tx = db.begin(a.org).await.unwrap();
    let label: String = sqlx::query_scalar("SELECT trigger_label FROM tracker_bindings")
        .fetch_one(tx.conn())
        .await
        .unwrap();
    assert_eq!(
        label, "dark-factory",
        "the row 0020 should have relabelled must still be there, untouched, \
         for the zero above to mean what this test claims it means"
    );

    // And the same statement, with org context supplied, does relabel it —
    // proving the statement is capable of matching at all, so the zero above
    // is attributable to the missing `app.org_id`, not to a typo in the seed
    // or the UPDATE's own WHERE clause.
    let updated = sqlx::query(
        "UPDATE tracker_bindings SET trigger_label = 'otto-factory' \
         WHERE trigger_label = 'dark-factory'",
    )
    .execute(tx.conn())
    .await
    .unwrap()
    .rows_affected();
    tx.commit().await.unwrap();
    assert_eq!(
        updated, 1,
        "the same statement, org-scoped, should have matched the seeded row"
    );
}
```

The negative assertion is deliberately `== 0`, not a `!=` guard that would only fire once the bug is
fixed — there is nothing to fix in 0020 (it cannot be edited) and nothing to fix in `Db::migrate`
(see Scope/Out). The test's job is to keep the failure mode itself visible and named, as the worked
example `CLAUDE.md`'s new paragraph (§1) points to, and as the thing the next contributor writing a
migration-time `UPDATE` against a tenant table should go read before assuming it works. The
read-back and the positive control at the end exist because `== 0` alone is indistinguishable from
several unrelated breakages (a seed that never landed, a dropped policy, a lost grant) — they
attribute the zero specifically to the missing `app.org_id`, which is the one claim the test exists
to prove.

## §3 Audit of applied migrations

Every file in `crates/of-core/migrations/` for a bare `UPDATE`/`DELETE FROM`/`INSERT INTO` (a
schema-only `ALTER TABLE` is not RLS-constrained and is not in scope):

```
$ grep -ln -E '^\s*(UPDATE|DELETE FROM|INSERT INTO)\b' crates/of-core/migrations/*.sql
crates/of-core/migrations/0006_billing.sql
crates/of-core/migrations/0020_rename_trigger_label_default.sql
crates/of-core/migrations/0022_user_label.sql
```

- **`0006_billing.sql`** — `INSERT INTO plans (...)`. `plans` has no `org_id` column and is absent
  from every `tenant_tables` array (`0007_rls.sql`, `0011_trackers.sql`); it carries no RLS policy at
  all. Not affected — there is no `org_id` predicate for a missing org context to fail.
- **`0022_user_label.sql`** — `UPDATE users SET label = ... WHERE label IS NULL`. `users` is
  explicitly a non-tenant table by design (the migration's own comment: "not a tenant table -- no
  org_id, absent from the tenant_tables array"), and login has to resolve a principal before an org
  is known, so it carries no RLS policy either. Not affected, for the same reason.
- **`0020_rename_trigger_label_default.sql`** — the one case: `tracker_bindings` is FORCE-RLS,
  tenant-scoped (`0011_trackers.sql:45-58`), and the `UPDATE` carries no org predicate and runs with
  no `app.org_id` set. The only migration with this shape.

No other applied migration needs remediation or a note; this section is the record of having
checked, so the next contributor with the same question does not have to re-run the same `grep`.

**On the grep's own limits.** The pattern above is anchored to line start and case-sensitive, so it
would miss DML written in lowercase or indented inside a `DO $$ ... $$` block — the idiom
`0007_rls.sql`, `0011_trackers.sql`, and `0018_rename_tenant_role.sql` all use for their `GRANT`/
`REVOKE` bodies. Re-run with a broader, case-insensitive, unanchored pattern
(`grep -linE '\b(update|delete[[:space:]]+from|insert[[:space:]]+into|merge|truncate|copy)\b'`)
during PR review turned up four more hits — `0003_jobs.sql`, `0007_rls.sql`, `0008_audit.sql`,
`0018_rename_tenant_role.sql` — all confirmed to be `GRANT`/`REVOKE` text or
`AFTER INSERT OR UPDATE OR DELETE` trigger clauses, not row-rewriting DML. The audit's conclusion
(0020 is the only affected migration) holds under the broader check too; a `DO $$` body still needs
a human read, not just a grep, if a future audit needs to redo this.

## Testing

- `cargo test -p of-core --test isolation` — the new test plus the full existing suite.
- `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`.
- No `web/` surface touched; no console/worker gates apply.

## Error Handling & Edge Cases

- **A future contributor adds a migration with this exact shape anyway.** Nothing enforces the §1
  rule automatically — it is documentation plus a worked example, not a lint. This spec accepts that
  gap explicitly rather than building migration-file static analysis for a two-migration-in-26
  occurrence rate; see Risks below.
- **The FORCE-RLS-fallback shape becomes this deployment's actual shape** (a future move to shared
  managed Postgres, per `docs/deploy/fly.md`'s own stated possibility). Nothing in this change alters
  that transition's safety either way — `Db::verify_tenant_isolation` already refuses to bind a port
  unless one of the two guard-2 shapes holds, independent of this spec. This spec only ensures that
  by the time that transition happens, the rule for writing migrations safely under it is already
  written down.

## Risks & Open Questions

- **The guard is documentation plus one worked example, not an automated check.** A lint that parses
  migration SQL for unscoped `UPDATE`/`DELETE` against tables in the `tenant_tables` arrays was
  considered and rejected as disproportionate: one occurrence of the risky shape found across 26
  migrations (§3), and a hand-written SQL parser is itself a maintenance liability. PR review raised
  this same point independently — this repository otherwise converts exactly this kind of rule into
  a failing test (`every_tool_has_a_price`, `exhaustive_over`, `the_queue_is_read_only_over_the_console`)
  — and the counter-argument is the one above: a single confirmed occurrence does not yet justify a
  parser, particularly one that would need to read `DO $$` bodies (see §3) to be trustworthy. If this
  shape recurs after this spec ships, that is evidence the documentation-only guard was insufficient
  and a follow-up should build the lint — not evidence this spec should have built it pre-emptively.
- **The follow-up issue for the `fly.toml`/`docs/deploy/fly.md` drift is filed, not fixed, in this
  PR.** Low severity (a stale comment, not a stale credential or a wrong deployed value) but real —
  tracked so it does not get lost.
- **"Production `tracker_bindings` is empty" is a claim about one specific database, not every
  database that has ever applied 0020.** A long-lived developer database, a staging instance, or a
  restore from a pre-0020 backup could still carry a stale `'dark-factory'` row — 0020 is already
  applied everywhere it has run and will never re-run, so such a row (if one exists anywhere) stays
  mislabeled permanently. Impact is cosmetic (a tracker comment reads the old product name), and
  Premise correction 1's reasoning for skipping a corrective migration is unaffected — there would
  still be nothing for this repository's own migrations to fix, since the row's home database is not
  one this repository controls or re-migrates. Noted so the "production is empty" claim is not read
  as "every database that ever ran 0020 is empty."
- **`crates/of-core/src/db.rs`'s `begin_unpinned` doc comment asserts a safety property that is false
  on this deployment's actual (superuser) connecting role** — "runs as the connecting role with no
  `app.org_id`, so tenant tables return zero rows rather than everything" is the FORCE-RLS-fallback
  outcome only; on a superuser it returns everything, which is exactly the distinction §1's revised
  paragraph now draws. Pre-existing, not introduced by this change, and out of this PR's file list
  (`CLAUDE.md` + `crates/of-core/tests/isolation.rs` only) — flagged during PR review and filed as
  `savvagent/otto-factory#112` rather than fixed here, for the same reason `fly.toml`'s drift was
  filed as `#106` instead of folded in.
