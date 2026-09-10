# Lease-resource backfill org-loop — repeat 0027's backfill safely

Goal: close `savvagent/otto-factory#119` by adding a new, forward-only migration that repeats
0027's `repo_leases.resource` backfill using the CLAUDE.md-documented per-org loop instead of
0027's `NO FORCE`/`FORCE ROW LEVEL SECURITY` toggle, idempotent against 0027's already-applied
work, plus a test proving the loop actually matches rows under the FORCE-RLS-fallback shape.

**Spec:** `docs/specs/2026-09-10-lease-resource-backfill-org-loop-design.md` — read it first.
This plan implements it exactly.

## Status — 2026-09-10

Task 1 implemented, reviewed, and corrected. PR #135 opened, `Closes #119`. The mandatory review
trio (Phase 4 step 8) found a **Critical** bug in the original design before merge:
`security-auditor`, `architect-reviewer`, and `pr-review-toolkit:code-reviewer` independently
flagged that the `resource NOT LIKE 'branch:%'` guard would silently rewrite every live
non-branch lease (`deploy:staging`, a migration lock, etc.) in every org, since `resource` has
been free-form — not branch-only — since 0027 itself shipped. Fixed by rebinding the migration's
predicate to provenance (`acquired_at` before `0027`'s `installed_on`) instead of value shape;
see the spec's Risks & Open Questions for the full history. Re-verified green after the fix:
`cargo test -p of-core --test isolation` (30 passed, including a test that now also asserts a
post-0027 `deploy:staging` row stays untouched) and `--test queue` (65 passed), `cargo clippy
--all-targets -- -D warnings`, `cargo fmt --all --check`. Awaiting the trio's re-review of the
fix before merge.

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

- [x] Write `rls_scopes_the_lease_resource_backfills_per_org_loop` in
      `crates/of-core/tests/isolation.rs`, placed immediately after
      `rls_scopes_a_migration_style_update_with_no_org_context` (before the "guard on the guard"
      section comment). Seed two `repo_leases` rows directly on the pool (via `tenant(&db, ...)`
      for the org/repo/user, then bound `INSERT`s): an unprefixed `resource = 'main'` with
      `acquired_at` backdated to before `_sqlx_migrations`'s `installed_on` for version 27
      (a genuinely pre-0027 row), and an unprefixed `resource = 'deploy:staging'` with a normal
      `acquired_at` (a legitimate post-0027 free-form lease). Open `db.begin_unpinned()`, `SET
      LOCAL ROLE of_app`, run 0030's migration file via
      `include_str!("../migrations/0030_lease_resource_backfill.sql")` (not a copied string, so
      the test can't drift from what ships), commit, then read both rows back through
      `db.begin(a.org)` and assert the first is now `branch:main` while the second is still
      `deploy:staging` unchanged.
- [x] Write `crates/of-core/migrations/0030_lease_resource_backfill.sql` per the spec's §1: the
      `DO $$ ... $$` block bounded by `resource NOT LIKE 'branch:%' AND acquired_at < cutoff`,
      where `cutoff` is `_sqlx_migrations.installed_on` for version 27 — **not** a shape-only
      guard. (The original shape-only guard shipped in this same PR, was caught as Critical by
      the mandatory review trio, and was corrected to this provenance-bounded form before merge
      — see the spec's Risks & Open Questions.)
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
