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

> **A migration that rewrites tenant-table data cannot rely on a bare `UPDATE`/`DELETE`.**
> `Db::migrate` runs every migration statement on the connection pool directly — never through
> `Db::begin`, so `app.org_id` is never set and `of_app` is never assumed. On a single-role
> deployment (`SET LOCAL ROLE of_app` succeeds at request time) this is invisible: migrations run as
> a superuser or the table owner, and RLS does not constrain either regardless of `FORCE`. On the
> managed-Postgres fallback shape — the one guard 2 exists to keep honest — a migration statement
> runs under the very role `FORCE ROW LEVEL SECURITY` applies to, with `current_org()` NULL for the
> statement's entire lifetime, so `org_id = current_org()` is never true and the statement silently
> matches zero rows, for every tenant, forever. `savvagent/otto-factory#70` found this in
> `0020_rename_trigger_label_default.sql`'s relabeling `UPDATE`; `rls_scopes_a_migration_style_update_with_no_org_context`
> in `tests/isolation.rs` reproduces it. A migration that must rewrite existing tenant-table data has
> two correct shapes: wrap the statement in `ALTER TABLE <table> NO FORCE ROW LEVEL SECURITY` /
> `ALTER TABLE <table> FORCE ROW LEVEL SECURITY` (mirroring the table's own definition in
> `0007_rls.sql`/`0011_trackers.sql`, restored before the migration transaction commits — a migration
> that leaves a tenant table un-forced is a worse bug than the one it fixes), or loop over every org
> and `SELECT set_config('app.org_id', ..., true)` before each org's statement, the same way
> `Db::begin` does at request time. A schema-only change (`ALTER TABLE ... ADD COLUMN`, a new
> `DEFAULT`, an index) is unaffected — RLS only constrains `SELECT`/`INSERT`/`UPDATE`/`DELETE`
> against existing rows, never DDL.

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
/// LOCAL ROLE of_app` and never `set_config('app.org_id', …)`. On this test's
/// connecting superuser that is invisible: RLS does not constrain a superuser
/// regardless of `FORCE`. On the FORCE-RLS-fallback deployment shape (managed
/// Postgres, no `CREATEROLE` — see CLAUDE.md), migrations run under the very
/// role `FORCE ROW LEVEL SECURITY` applies to, with no org context ever set,
/// so `current_org()` is NULL for the statement's entire lifetime and
/// `org_id = current_org()` is never true: the UPDATE silently matches zero
/// rows, for every tenant, forever.
///
/// `savvagent/otto-factory#70`. No corrective data migration accompanies this
/// test — production `tracker_bindings` was confirmed empty at the time of
/// investigation, and this deployment's migrations currently run as a
/// superuser that bypasses RLS outright (`docs/deploy/fly.md`). This test is
/// the guard for the deployment shape where that stops being true.
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

    // The exact shape `Db::migrate` runs a statement under: `of_app`, and no
    // `app.org_id` ever set — a schema migration has no tenant to set it to.
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
        "a bare UPDATE against a FORCE RLS tenant table with no app.org_id set \
         is exactly the failure this test exists to keep visible — if this \
         starts affecting rows, something about the deployment's isolation \
         shape changed and every migration written under the old assumption \
         needs re-auditing"
    );
}
```

The assertion is deliberately `== 0`, not a `!=` guard that would only fire once the bug is fixed —
there is nothing to fix in 0020 (it cannot be edited) and nothing to fix in `Db::migrate` (see
Scope/Out). The test's job is to keep the failure mode itself visible and named, as the worked
example `CLAUDE.md`'s new paragraph (§1) points to, and as the thing the next contributor writing a
migration-time `UPDATE` against a tenant table should go read before assuming it works.

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
  considered and rejected as disproportionate: two occurrences of the risky shape in 26 migrations,
  both now accounted for, and a hand-written SQL parser is itself a maintenance liability. If this
  shape recurs after this spec ships, that is evidence the documentation-only guard was insufficient
  and a follow-up should build the lint — not evidence this spec should have built it pre-emptively.
- **The follow-up issue for the `fly.toml`/`docs/deploy/fly.md` drift is filed, not fixed, in this
  PR.** Low severity (a stale comment, not a stale credential or a wrong deployed value) but real —
  tracked so it does not get lost.
