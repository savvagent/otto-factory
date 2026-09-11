# `Db::audit_global_on` takes `&mut Unpinned` — implementation plan

## Goal

Make it a compile error, not a runtime-checked or doc-warned misuse, to call
`Db::audit_global_on` with a pinned `Tx`'s connection (`savvagent/otto-factory#133`). Add an
`Unpinned` type mirroring `Tx`'s own shape, have `Db::begin_unpinned` return it, and narrow
`Db::audit_global_on`'s parameter to `&mut Unpinned`.

**Spec:** `docs/specs/2026-09-11-audit-global-on-unpinned-design.md` — read it first. This plan
implements it exactly.

## Status — 2026-09-11

Shipped in PR #165 (branch `core/audit-global-unpinned`), merged as
`b906ee614063054ba2bc5f9984a7530bf69440c9`. CI on that merge commit is green (rust + web, run
against master). One task, complete: `cargo test --workspace`,
`cargo clippy --all-targets -- -D warnings`, and `cargo fmt --all --check` all pass.

Seven reviewers ran against PR #165: the mandatory trio (rust-pro, architect-reviewer,
security-auditor) plus four conditional reviewers (code-reviewer, pr-test-analyzer,
comment-analyzer, type-design-analyzer) and the automated Copilot reviewer. They converged on
two Important findings, each independently raised by architect-reviewer, security-auditor, and
code-reviewer, with the second also independently raised by pr-test-analyzer: the type change
alone left a residual gap — a caller could still pin an `Unpinned` by hand via `conn()` and
`set_config('app.org_id', …)` before passing it to `audit_global_on`, which still compiled — and
deleting the old runtime test dropped the only coverage of `audit_events_append`'s
`org_id IS NULL` branch under a pinned transaction. Both were closed in the same PR:
`audit_global_on` now re-checks `app.org_id` at call time and refuses if the transaction has
been pinned since it was opened, and a DB-level policy test
(`a_pinned_transaction_cannot_append_a_null_org_audit_row`) plus a focused test of the new
runtime guard (`audit_global_on_refuses_a_transaction_pinned_after_it_was_opened`) were added to
`crates/of-core/tests/isolation.rs`. Three of the trio's applied Suggestions also landed in the
same PR: `Unpinned` re-exported at the crate root (`pub use db::{Db, Tx, Unpinned}` in `lib.rs`),
`audit_global_on` importing `Unpinned` directly instead of fully-qualifying
`crate::db::Unpinned`, and a module-doc update to `db.rs` naming `Unpinned` alongside `Tx`.

PR #164 (`finish_registration_tx`, landed concurrently) *extracted* `finish_registration_tx` out
of `finish_registration`, moving the existing sole call to `audit_global_on` into the new
function rather than adding a second call site — `finish_registration` becomes a two-line
wrapper over it. That extracted function still used the pre-refactor generic-executor signature
when #164 merged, which conflicted with this PR's `Unpinned` type change once #165's branch was
synced with master; it was adapted to `&mut Unpinned` in the same PR's merge-conflict
resolution, along with two further call sites that broke for the same reason:
`Db::consume_account_claim` (`crates/of-core/src/invites.rs`) and
`of_web::routes::auth::claim_finish` (`crates/of-web/src/routes/auth.rs`), both switched to
`tx.conn()`.

## Global Constraints

These hold for every step below:

- No AI self-attribution anywhere (commits, PR body, comments, docs).
- Run `cargo fmt --all` before every Rust commit.
- Every SQL statement stays exactly where it already lives — this task changes no SQL text at all,
  only which Rust value each existing query's `.execute(...)`/`.fetch_one(...)`/`.fetch_all(...)`
  call is fed.
- Tests need a real Postgres: `podman compose up -d` (port 15433) and a `.env` with `DATABASE_URL`
  (`cp .env.example .env`).
- No tenant table is added or touched — `Unpinned` only ever carries a `NULL`-org, unpinned
  connection. No cross-org negative test applies.
- No MCP tool is added — no `of-billing::classify` entry needed.
- No migration — no schema change.
- This is a backward-compatible internal refactor per Non-Negotiable Rule 6 (no MCP tool, console
  route, OAuth/discovery endpoint, config key, or migration changes shape) — flagged to the
  architect reviewer per the spec's Public-interface changes table regardless.

## File Structure

| File | Responsibility |
|---|---|
| **Modify.** `crates/of-core/src/db.rs` | Add `Unpinned` beside `Tx`; change `Db::begin_unpinned`'s return type from `Result<Transaction<'static, Postgres>>` to `Result<Unpinned>`. |
| **Modify.** `crates/of-core/src/audit.rs` | Narrow `Db::audit_global_on`'s parameter from `E: sqlx::PgExecutor<'e>` to `&mut crate::db::Unpinned`; update its doc comment. |
| **Modify.** `crates/of-auth/src/passkeys.rs` | `finish_registration`: `&mut *tx` → `tx.conn()` for the credential insert; `&mut *tx` → `&mut tx` for the `audit_global_on` call. One other `begin_unpinned` site in this file (no query, `commit` only) needs no change beyond the inferred type. |
| **Modify.** `crates/of-auth/src/tokens.rs` | Two `begin_unpinned` sites: `&mut *tx` → `tx.conn()` at each query call. |
| **Modify.** `crates/of-auth/src/ratelimit.rs` | Two `begin_unpinned` sites: `&mut *tx` → `tx.conn()` at each query/helper call. |
| **Modify.** `crates/of-core/src/orgs.rs` | One `begin_unpinned` site: `&mut *tx` → `tx.conn()` at its `fetch_one` and `execute` calls. |
| **Modify.** `crates/of-core/src/invites.rs` | One `begin_unpinned` site (`commit` only) — no change beyond the inferred type. |
| **Modify.** `crates/of-core/tests/isolation.rs` | Nine `begin_unpinned` sites: `&mut *tx` → `tx.conn()` at each query call. Remove `audit_global_on_refuses_a_pinned_connection`; replace with an explanatory comment at the same location. |

## Task Order & Rationale

One task. `Unpinned`'s definition, `begin_unpinned`'s new return type, `audit_global_on`'s new
parameter type, and every call site touched by the type change all have to land together — the
crate does not compile in any intermediate state where only some of them have moved, since
`begin_unpinned`'s callers and `audit_global_on`'s one caller are both downstream of the same type.

## Task 1 — `Unpinned` type, narrowed `audit_global_on`, and every call site ✅/🚧/⬜: ✅

**Files:** `crates/of-core/src/db.rs`, `crates/of-core/src/audit.rs`, `crates/of-auth/src/
passkeys.rs`, `crates/of-auth/src/tokens.rs`, `crates/of-auth/src/ratelimit.rs`,
`crates/of-core/src/orgs.rs`, `crates/of-core/src/invites.rs`, `crates/of-core/tests/isolation.rs`

**Interfaces:**
- Consumes: `sqlx::Transaction<'static, Postgres>` (existing, from `self.pool.begin()`).
- Produces: `of_core::db::Unpinned` (new, crate-workspace-public: `conn()`, `commit()`,
  `rollback()`); `Db::begin_unpinned() -> Result<Unpinned>` (return type changed, same name);
  `Db::audit_global_on(conn: &mut Unpinned, e: Entry) -> Result<()>` (parameter type changed, same
  name).

Steps:

- [x] **Baseline: confirm the starting point is green.**
  `podman compose up -d` (skip if already running), `cp .env.example .env` if no `.env` exists yet,
  then `cargo test -p of-core --test isolation` and `cargo test -p of-auth --test passkeys`.
  Expected: both pass, including the still-present
  `audit_global_on_refuses_a_pinned_connection` (this step establishes the before-state; it is
  removed later in this same task).

- [x] **Add `Unpinned` to `crates/of-core/src/db.rs`, beside `Tx`.**
  Exact shape per spec §1: `pub struct Unpinned { tx: Transaction<'static, Postgres> }` with
  `pub fn conn(&mut self) -> &mut sqlx::PgConnection`, `pub async fn commit(self) -> Result<()>`,
  `pub async fn rollback(self) -> Result<()>` — no `Deref`/`DerefMut` impl (spec's Scope/Out and
  Assumptions explain why). Full doc comment per spec §1.

- [x] **Change `Db::begin_unpinned`'s return type.**
  `pub async fn begin_unpinned(&self) -> Result<Unpinned> { Ok(Unpinned { tx:
  self.pool.begin().await? }) }`. Its existing doc comment (the deployment-shape warning) carries
  over unchanged.

- [x] **Compile-check `of-core` alone to see the exact break list.**
  `cargo build -p of-core`. Expected: fails at every call site the type change touches inside this
  crate (`crates/of-core/src/orgs.rs`, `crates/of-core/src/invites.rs`,
  `crates/of-core/src/audit.rs`'s `audit_global_on` — not yet updated) plus
  `crates/of-core/tests/isolation.rs` (test target, checked separately). This is expected and is how
  the remaining steps are scoped — do not treat it as a regression.

- [x] **Narrow `Db::audit_global_on` in `crates/of-core/src/audit.rs`.**
  `pub async fn audit_global_on(conn: &mut crate::db::Unpinned, e: Entry) -> Result<()> {
  e.write(None, conn.conn()).await }`. Replace the doc comment with spec §2's version (explains the
  type-level fix, contrasts with the previous generic signature and with `audit_global`'s best-effort
  semantics). Remove the now-unused `sqlx::PgExecutor` import from this file if nothing else in it
  still names it directly (check: `Entry::write`'s own signature still uses it, so the import stays
  — confirm rather than assume).

- [x] **Fix `crates/of-core/src/orgs.rs`.**
  At the one `begin_unpinned` site (~line 196), change `.fetch_one(&mut *tx)` and
  `.execute(&mut *tx)` to `.fetch_one(tx.conn())` and `.execute(tx.conn())`. `tx.commit()` is
  unchanged.

- [x] **Fix `crates/of-core/src/invites.rs`.**
  At the one `begin_unpinned` site (~line 261), no query runs on `tx` directly in this file — confirm
  by re-reading the current call site; if so, only the inferred type of `tx` changes and no line
  needs editing beyond leaving `tx.commit()` as-is. If a query call was missed during spec drafting,
  apply the same `&mut *tx` → `tx.conn()` substitution.

- [x] **Compile-check `of-core` again.**
  `cargo build -p of-core`. Expected: clean. `cargo test -p of-core --test isolation` will still fail
  to *compile* at this point, because `audit_global_on_refuses_a_pinned_connection`'s scenario is now
  a type error — expected, fixed two steps below.

- [x] **Fix `crates/of-auth/src/passkeys.rs`.**
  Two `begin_unpinned` sites. In `finish_registration` (~lines 268–299): change
  `.execute(&mut *tx)` (the credential insert) to `.execute(tx.conn())`; change
  `Db::audit_global_on(&mut *tx, ...)` to `Db::audit_global_on(&mut tx, ...)`; `tx.commit()`
  unchanged. The second site (~line 590, `commit` only, no query) needs no edit beyond the inferred
  type.

- [x] **Fix `crates/of-auth/src/tokens.rs`.**
  Two `begin_unpinned` sites (~lines 344–368 and ~480–482). Apply `&mut *tx` → `tx.conn()` wherever
  a query executes on `tx`; leave `tx.commit()` calls unchanged.

- [x] **Fix `crates/of-auth/src/ratelimit.rs`.**
  Two `begin_unpinned` sites (~lines 115–131 and ~242–253). Apply the same substitution, including
  the `count_failures(&mut *tx, bucket, policy.window_secs)` call at the second site, which becomes
  `count_failures(tx.conn(), bucket, policy.window_secs)` — `count_failures`'s own signature is
  generic over `PgExecutor` and needs no change.

- [x] **Compile-check the whole workspace.**
  `cargo build --workspace`. Expected: clean except for `crates/of-core/tests/isolation.rs`, fixed
  next.

- [x] **Fix the nine `begin_unpinned` sites in `crates/of-core/tests/isolation.rs`.**
  Apply `&mut *tx` → `tx.conn()` at each query call (lines ~710, 774, 865, 973, 1138, 1181, 1202,
  1243, 1285 as of the spec's drafting — re-locate by searching for `begin_unpinned` rather than
  trusting line numbers, since earlier edits in this task may have shifted them). Leave
  `tx.commit()`/`tx.rollback()` calls unchanged.

- [x] **Remove `audit_global_on_refuses_a_pinned_connection` and replace it with the explanatory
  comment.**
  Delete the test function (its scenario — `of_core::Db::audit_global_on(tx.conn(), ...)` where
  `tx: Tx` — no longer typechecks: `Tx::conn()` yields `&mut sqlx::PgConnection`, and
  `audit_global_on` now requires `&mut Unpinned`). Insert the comment from spec's Testing section at
  the same location, naming `savvagent/otto-factory#133` and explaining that the misuse is now a
  compile error rather than a runtime-checked one.

- [x] **Compile-check the whole workspace including tests.**
  `cargo build --workspace --tests`. Expected: clean.

- [x] **Run the affected test suites.**
  `cargo test -p of-core --test isolation` and `cargo test -p of-auth --test passkeys`. Expected:
  same tests pass as the baseline run, minus the removed test — no assertion in any surviving test
  changes, since every substitution reaches the identical `&mut sqlx::PgConnection` the old deref
  did.

- [x] **Run the full workspace suite.**
  `cargo test --workspace`. Expected: green — this confirms no other test file (`crates/of-auth/
  tests/passkeys.rs`, `crates/of-auth/tests/tokens.rs` if present, etc.) depends on
  `begin_unpinned`'s old concrete return type in a way this task's changes broke.

- [x] **Lint and format.**
  `cargo clippy --all-targets -- -D warnings`, then `cargo fmt --all`.

- [x] **Format and commit.**
  `cargo fmt --all` (again, to catch anything the lint step's fixes touched), then:
  ```
  git add crates/of-core/src/db.rs crates/of-core/src/audit.rs crates/of-core/src/orgs.rs \
    crates/of-core/src/invites.rs crates/of-core/tests/isolation.rs \
    crates/of-auth/src/passkeys.rs crates/of-auth/src/tokens.rs crates/of-auth/src/ratelimit.rs
  git commit -m "of-core: make Db::audit_global_on take an Unpinned transaction"
  ```

**Out-of-band artifacts:** none touched — no container image, console bundle, Cloudflare Worker, or
migration change in this task. (Vacuously satisfied, not skipped.)

## Record-as-shipped

Done — this commit (docs PR #167, following #165's merge) is that record: the spec's
`> **Status:**` is flipped to IMPLEMENTED, and this plan's `## Status` block above carries the
merge commit and the review-trio outcome, per Phase 4 step 12 of `otto-factory-development`.
