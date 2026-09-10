# `finish_registration`'s credential insert and audit write become atomic — implementation plan

## Goal

Make `finish_registration` (`crates/of-auth/src/passkeys.rs`) write its `passkeys` row and its
`auth.passkey.registered` audit row in one transaction, so a transient DB error between the two can
no longer leave a live credential with no audit row (`savvagent/otto-factory#108`).

**Spec:** `docs/specs/2026-09-10-passkey-registration-atomic-audit-design.md` — read it first. This
plan implements it exactly.

## Status — 2026-09-10

Shipped in `savvagent/otto-factory#131`. One task, complete — plus two hardening commits added
during PR review (a doc warning + regression test on `Db::audit_global_on`, and a deterministic
rollback test forcing the audit write to fail). Three follow-up issues filed from review findings
outside this plan's scope: `#132`, `#133`, `#134` — see the spec's Risks & Open Questions.

## Global Constraints

These hold for every step below:

- No AI self-attribution anywhere (commits, PR body, comments, docs).
- Run `cargo fmt --all` before every Rust commit.
- Every SQL statement in `of-core` stays in `of-core`; the one raw `INSERT INTO passkeys` this task
  touches already lives in `of-auth` today (not a tenant table, not `of-core`'s domain) and stays
  there unchanged in shape — only its executor changes from the pool to a transaction connection.
- Tests need a real Postgres: `podman compose up -d` (port 15433) and a `.env` with `DATABASE_URL`
  (`cp .env.example .env`).
- No tenant table is added or touched by this change — `passkeys` has no `org_id` column, and the
  `audit_events` row here is written with `org_id = NULL`, the documented unpinned-control-plane
  shape. No cross-org negative test applies.
- No MCP tool is added — no `of-billing::classify` entry needed.
- No migration — no schema change.

## File Structure

| File | Responsibility |
|---|---|
| **Modify.** `crates/of-core/src/audit.rs` | Factor `Entry`'s INSERT binding into a private, executor-generic helper; add `Db::audit_global_on` beside the existing `audit_global`/`audit_for_org`. |
| **Modify.** `crates/of-auth/src/passkeys.rs` | `finish_registration` opens one unpinned transaction, runs the credential insert and the audit write on it, commits once. |

No test file needs new arguments — `finish_registration`'s signature is unchanged, so
`crates/of-auth/tests/passkeys.rs`'s five existing call sites compile as-is.

## Task Order & Rationale

One task. The `of-core::audit` refactor and the `of-auth::passkeys` rewrite are done together
because the second cannot compile without the first (`Db::audit_global_on` does not exist yet), and
splitting them into two commits would leave an intermediate commit with a private-only refactor that
compiles but serves no caller — not a meaningful checkpoint to pause at.

## Task 1 — Atomic credential + audit write ✅/🚧/⬜: ✅

**Files:** `crates/of-core/src/audit.rs`, `crates/of-auth/src/passkeys.rs`

**Interfaces:**
- Consumes: `of_core::db::Db::begin_unpinned` (existing), `sqlx::PgExecutor` (sqlx 0.8.6, confirmed
  vendored).
- Produces: `of_core::audit::Db::audit_global_on<'e, E: sqlx::PgExecutor<'e>>(conn: E, e: Entry) ->
  Result<()>` (new, additive, crate-workspace-public).

Steps:

- [ ] **Baseline: run the existing suite to confirm the starting point is green.**
  `podman compose up -d` (skip if already running), `cp .env.example .env` if no `.env` exists yet,
  then `cargo test -p of-auth --test passkeys`. Expected: all tests pass (this establishes the
  before-state; no test changes are made in this task).

- [ ] **Refactor `crates/of-core/src/audit.rs`: extract `Entry::write`.**
  Add a private `async fn write<'e, E>(self, org: Option<OrgId>, conn: E) -> Result<()> where E:
  sqlx::PgExecutor<'e>` on `impl Entry` that runs today's `INSERT_SQL` binding (currently duplicated
  across `Tx::audit`, `Db::audit_global`, `Db::audit_for_org`) against the given `org` and executor.
  Rewrite `Tx::audit`, `Db::audit_global`, and `Db::audit_for_org` to call `e.write(...)` instead of
  each running its own copy of the query. Exact signatures per spec §1.

- [ ] **Add `Db::audit_global_on` in the same file.**
  `pub async fn audit_global_on<'e, E>(conn: E, e: Entry) -> Result<()> where E:
  sqlx::PgExecutor<'e>` — calls `e.write(None, conn).await`. Doc comment per spec §1 (contrasts with
  `audit_global`'s best-effort, pool-based semantics).

- [ ] **Compile check the of-core refactor in isolation.**
  `cargo build -p of-core`. Expected: compiles clean — this is a pure refactor of existing private
  machinery plus one additive function, so no caller elsewhere in the workspace should need a
  change. `cargo build --workspace` to confirm no other crate's use of `Tx::audit` /
  `Db::audit_global` / `Db::audit_for_org` broke (none should, since their signatures are unchanged).

- [ ] **Rewrite `finish_registration` in `crates/of-auth/src/passkeys.rs`.**
  Replace the `db.pool()`-executed `INSERT INTO passkeys` and the subsequent best-effort
  `db.audit_global(...)` call (wrapped in `if let Err(e) = ... { tracing::error!(...) }`) with: open
  `let mut tx = db.begin_unpinned().await?;`, run the credential insert on `.execute(&mut *tx)`
  (same SQL text, same unique-violation error mapping), run
  `Db::audit_global_on(&mut *tx, Entry::new(...)...).await?` (propagate via `?`, no more
  `tracing::error!` swallow), then `tx.commit().await?;` before `Ok(user_id)`. Exact code per spec
  §2, including the comment explaining why the two writes are now one transaction.

- [ ] **Run the affected test suite.**
  `cargo test -p of-auth --test passkeys`. Expected: same tests pass as the baseline run — in
  particular `registration_writes_the_passkey_registered_action` and
  `registration_records_which_flow_wrote_it`, which assert both rows exist after a successful
  registration, now passing against the transactional write instead of two independent ones.

- [ ] **Run the full workspace suite.**
  `cargo test --workspace`. Expected: green. This is what confirms the `Entry::write` refactor did
  not change behavior for any other caller of `Tx::audit`, `Db::audit_global`, or
  `Db::audit_for_org` (login failures, org invitations, etc.) — none of those call sites change in
  this task, so this is a regression check, not new coverage.

- [ ] **Lint and format.**
  `cargo clippy --all-targets -- -D warnings`, then `cargo fmt --all`.

- [ ] **Format and commit.**
  `cargo fmt --all` (again, to catch anything the lint step's fixes touched), then:
  ```
  git add crates/of-core/src/audit.rs crates/of-auth/src/passkeys.rs
  git commit -m "of-auth: make finish_registration's credential insert and audit write atomic"
  ```

**Out-of-band artifacts:** none touched — no container image, console bundle, Cloudflare Worker, or
migration change in this task. (Vacuously satisfied, not skipped.)

## Record-as-shipped

After merge, per house style: flip this plan's Task 1 marker to ✅ and the spec's `> **Status:**` to
IMPLEMENTED, in a follow-up worktree + PR, per `otto-factory-development`'s Phase 4 step 12.
