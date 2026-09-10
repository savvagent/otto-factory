# Lease-resource backfill org-loop — repeat 0027's backfill safely

Goal: close `savvagent/otto-factory#119` by adding a new, forward-only migration that repeats
0027's `repo_leases.resource` backfill using the CLAUDE.md-documented per-org loop instead of
0027's `NO FORCE`/`FORCE ROW LEVEL SECURITY` toggle, idempotent against 0027's already-applied
work, plus a test proving the loop actually matches rows under the FORCE-RLS-fallback shape.

**Spec:** `docs/specs/2026-09-10-lease-resource-backfill-org-loop-design.md` — read it first.
This plan implements it exactly.

## Status — 2026-09-10

Task 1 implemented and locally green (migration + test written, `cargo test -p of-core --test
isolation` and `--test queue` pass, `cargo clippy --all-targets -- -D warnings` and `cargo fmt
--all` clean). Not yet through the spec-compliance review, code-quality review, or PR (Phase 3
steps C/E and Phase 4 of otto-factory-development, run from the parent session rather than this
plan document). PR will reference `savvagent/otto-factory#119` directly (`Closes #119`) — no
separate tracker-transition step needed on the GitHub-issue path beyond what the ship phase does.

## Global Constraints

- Every SQL statement lives in `of-core`; nothing here touches `of-mcp`/`of-web`/`of-auth`.
- Migrations are forward-only. `0027_lease_resource.sql` is never edited — only a new file.
- `0007_rls.sql` continues to run last; this task adds no entry to its `tenant_tables` array
  (`repo_leases` is already registered there).
- A tenant-table-touching migration needs its own `org_id` scoping — RLS is not available to
  supply one automatically. The per-org loop (explicit `org_id` predicate **and**
  `set_config('app.org_id', ..., true)`) is the only pattern used here.
- `cargo fmt --all` before every Rust commit. No AI self-attribution anywhere (commits, PR body,
  comments).
- Tests need a real Postgres: `podman compose up -d` and a `.env` with `DATABASE_URL`
  (`cp .env.example .env`).
- No new MCP tool, no new console route, no new `OF_*` config key — no `of-billing::classify`
  entry needed.

## File Structure

| File | Responsibility |
|---|---|
| `crates/of-core/migrations/0030_lease_resource_backfill.sql` | **Create.** The per-org-loop backfill, guarded idempotent against 0027's already-applied work. |
| `crates/of-core/tests/isolation.rs` | **Modify.** Add `rls_scopes_the_lease_resource_backfills_per_org_loop`, the positive-case mirror of the existing `rls_scopes_a_migration_style_update_with_no_org_context`. |

## Task Order & Rationale

Single task — the migration and its test are one unit of work; there is no meaningful
intermediate state between "no migration" and "migration + the test that proves it works under
the deployment shape where it matters."

## Task 1 — Add the per-org-loop backfill migration and its RLS test ✅

**Files:** `crates/of-core/migrations/0030_lease_resource_backfill.sql` (create),
`crates/of-core/tests/isolation.rs` (modify)

**Interfaces:** Consumes: the existing `orgs`/`repo_leases` schema, the `of_app` role, and the
`Db::begin_unpinned`/`Db::begin` helpers already used by the neighboring `rls_scopes_*` tests.
Produces: no new interface — this is a migration + test only.

- [x] Write the failing test first: add `rls_scopes_the_lease_resource_backfills_per_org_loop` to
      `crates/of-core/tests/isolation.rs`, placed immediately after
      `rls_scopes_a_migration_style_update_with_no_org_context` (before the "guard on the guard"
      section comment). Seed a `repo_leases` row directly on the pool with an unprefixed
      `resource = 'main'` (via `tenant(&db, ...)` for the org/repo/user, then a bound `INSERT`),
      open `db.begin_unpinned()`, `SET LOCAL ROLE of_app`, run 0030's own `DO $$ ... $$` statement
      verbatim, commit, then read the row back through `db.begin(a.org)` and assert
      `resource == "branch:main"`.
- [x] Run `cargo test -p of-core --test isolation rls_scopes_the_lease_resource_backfills_per_org_loop`
      and confirm it FAILS (the migration file does not exist yet, so 0030 has not created the
      target state — actually: since the test invokes the SQL inline rather than relying on the
      migration having run, confirm instead that it fails for the *right* reason before the
      migration file exists: it should still pass once the inline SQL is correct, since the test
      exercises the statement directly, not the migration file. If it already passes at this point,
      note that in the report — the true regression check is the negative test beside it staying
      green, not this one being red first, since there is no application code this test is
      characterizing behavior *before* rather than *after* writing.)
- [x] Write `crates/of-core/migrations/0030_lease_resource_backfill.sql` exactly as specified in
      the spec's §1 (the `DO $$ ... $$` block with the `resource NOT LIKE 'branch:%'` guard).
- [x] Run `cargo test -p of-core --test isolation` (the full suite) and confirm all tests pass,
      including the new one and the existing `rls_scopes_a_migration_style_update_with_no_org_context`
      (unaffected — different table, different statement).
- [x] Run `cargo test -p of-core --test queue` and confirm no regression (repo_leases-touching
      tests: `leases_are_per_branch`, `leases_are_per_resource_not_just_branch`,
      `reacquiring_your_own_lease_renews_it`, `only_the_holder_can_release`).
- [x] Run `cargo clippy --all-targets -- -D warnings` and `cargo fmt --all` (format, then re-check
      no diff).
- [x] Format and commit: `cargo fmt --all`, then
      `git commit -m "of-core: repeat lease-resource backfill using the per-org-loop pattern"`.

**Out-of-band reminder:** this touches `crates/of-core/migrations/`, so Phase 5 step 14 requires
confirming a fresh cluster applies cleanly (`podman compose down -v && podman compose up -d` then
`cargo test -p of-core`) before calling this done. No container image, console bundle, Cloudflare
Worker, or CI workflow change — those items are vacuously satisfied.
