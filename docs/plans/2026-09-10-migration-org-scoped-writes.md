# Migration-time unscoped writes against tenant tables — implementation plan

Goal: document, in `CLAUDE.md`, why a migration that rewrites tenant-table data cannot rely on a
bare `UPDATE`/`DELETE`, and add a regression test reproducing the exact failure the originating
issue found in `0020_rename_trigger_label_default.sql` — closing `savvagent/otto-factory#70`.

**Spec:** `docs/specs/2026-09-10-migration-org-scoped-writes-design.md` — read it first. This plan
implements it exactly.

---

## Status — 2026-09-10

Not started.

---

## Global Constraints

These hold for the (single) task in this plan:

- No AI self-attribution anywhere — commits, comments, docs, PR body.
- Run `cargo fmt --all` before every Rust commit.
- No SQL statement is added outside `of-core` — this plan adds a test, not a migration; no new
  migration file is created (spec Scope/Out — there is no data to correct).
- Tests need a real Postgres: `podman compose up -d` (Postgres 16 on host port 15433) and a `.env`
  with `DATABASE_URL` (`cp .env.example .env`).
- No tenant table is added or changed, so no `0007_rls.sql`/`0011_trackers.sql` registration step
  applies. The new test exercises existing RLS policies (`tracker_bindings_tenant_isolation`); it
  does not create one.
- No MCP tool is added, so no `of-billing::classify` step applies.
- No public-interface change of any kind (spec's Public-interface changes table: all "none").
- No `web/` surface touched — no `npm run check`/`lint`/`test`/`build` gate applies to this plan.

## File Structure

| File | Responsibility |
|---|---|
| **Modify.** `CLAUDE.md` | New paragraph in "Tenant isolation — the rule that outranks convenience", after the existing "A privilege granted to `of_app` is not a protection" paragraph. |
| **Modify.** `crates/of-core/tests/isolation.rs` | New regression test `rls_scopes_a_migration_style_update_with_no_org_context`, placed after `rls_scopes_tracker_bindings` and before the "guard on the guard" comment block. |

No migration file, no `of-core/src` change, no other crate touched.

## Task Order & Rationale

One task. The two files change together and neither is meaningful alone: the `CLAUDE.md` paragraph
names the test by its function name, and the test's own doc comment is what a reader lands on after
following that reference — writing one without the other leaves a dangling citation.

---

## Task 1 — Document the rule and add the regression test

**Files:** `CLAUDE.md`, `crates/of-core/tests/isolation.rs`.

**Interfaces:** none produced or consumed — this task adds a test to an existing test binary and a
paragraph to an existing doc. No function signature, tool, route, or schema changes.

- [ ] Confirm current line context before editing: `grep -n "A privilege granted to \`of_app\`" CLAUDE.md`
      and `grep -n "rls_scopes_tracker_bindings\|guard on the guard" crates/of-core/tests/isolation.rs`
      — insert relative to what these actually return, not the line numbers cited in the spec (the
      spec's citations were read against the tree at spec-writing time and may have drifted by a
      line or two since).
- [ ] Write the failing test first, in `crates/of-core/tests/isolation.rs`, exactly as spec §2's
      code block (`rls_scopes_a_migration_style_update_with_no_org_context`), inserted immediately
      after `rls_scopes_tracker_bindings` and before the `// ---... The guard on the guard. ...---`
      comment block that follows it. Use the file's existing `Tenant`/`tenant()`/`db()`/
      `begin_unpinned()` helpers exactly as the neighboring `rls_scopes_tracker_connections`/
      `rls_scopes_tracker_bindings` tests do — do not add new helpers.
- [ ] Run `cargo test -p of-core --test isolation rls_scopes_a_migration_style_update_with_no_org_context`
      — expect it to compile and **pass immediately** (this is a regression/reproduction test, not a
      TDD red step: the bug it demonstrates already exists in already-applied migration 0020 and in
      `Db::migrate`'s current unscoped-connection shape, neither of which this plan changes — see
      spec Scope/Out. A failure here means either the test's SQL is wrong or `of_app`/FORCE RLS is
      not configured the way the surrounding `rls_scopes_*` tests assume in this environment; either
      way, stop and investigate rather than proceeding.)
- [ ] Run `cargo test -p of-core --test isolation` (the whole file) — expect all existing tests to
      keep passing; confirm no accidental interference from the new test's direct `INSERT` on
      `db.pool()` (it uses its own fresh `#[sqlx::test]` database, so none is expected, but confirm).
- [ ] Add the `CLAUDE.md` paragraph exactly as spec §1's blockquote, as a new paragraph immediately
      after the existing "**A privilege granted to `of_app` is not a protection.**" paragraph and
      before the "Note that ordinary cross-org tests pass on the strength of guard 1 alone."
      paragraph that currently follows it. Reference the test by its exact function name
      (`rls_scopes_a_migration_style_update_with_no_org_context`) so the citation resolves.
- [ ] Re-read the edited `CLAUDE.md` section once, start to finish, to confirm the new paragraph
      reads as a continuation of the existing "Tenant isolation" section's register (short
      declarative sentences, bolded lead-in) rather than as a stylistic outlier — no automated gate
      checks this, it is a manual proofread.
- [ ] `cargo clippy --all-targets -- -D warnings` (unscoped — this is the only Rust change in the
      plan, so there is no earlier-task compile boundary to respect). `cargo fmt --all`.
- [ ] `cargo test --workspace` — expect all pass (confirms the new test doesn't interact badly with
      anything elsewhere in the suite; the change surface is one test function and one doc
      paragraph, so this is a cheap final confirmation, not an expected source of new failures).
- [ ] Commit: `git commit -m "core: document and test migration-time unscoped writes against tenant tables"`.

---

## Rule for this task

Done when: the new test passes, `cargo test --workspace` is clean, `cargo clippy --all-targets -- -D
warnings` is clean, `cargo fmt --all --check` is clean, and the `CLAUDE.md` paragraph correctly names
the test that backs it. No cross-org negative test applies beyond the new test itself — it does not
add a tenant table or a tenant-scoped function, it reproduces a known RLS interaction using the
existing `tracker_bindings_tenant_isolation` policy.
