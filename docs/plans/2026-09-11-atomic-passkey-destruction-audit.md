# `passkeys::remove`/`clear`'s audit writes become atomic — implementation plan

## Goal

Make `passkeys::remove` and `passkeys::clear` (`crates/of-auth/src/passkeys.rs`) write their
destructive `DELETE` and its audit row in one transaction, and make
`of_web::routes::orgs::reset_member_passkeys`'s `auth.passkey.cleared` write atomic with the
destructive change it records instead of best-effort after commit — closing
`savvagent/otto-factory#134`.

**Spec:** `docs/specs/2026-09-11-atomic-passkey-destruction-audit-design.md` — read it first. This
plan implements it exactly. (As shipped, "exactly" needs a caveat — see the `## Status` block below
for where the implementation diverged from the steps that follow.)

## Status — 2026-09-11

**Shipped** in `savvagent/otto-factory#170` (merged). Task 1 landed as planned for `remove` and
`clear`, but the implementation diverged from this plan in two ways during the review rounds
below — recorded here rather than by rewriting the steps below, per this repo's record-as-shipped
convention:

- **The last-passkey race.** This plan (and the spec version it was drafted against) scoped `remove`'s
  count-then-delete race out as a pre-existing, separately-tracked issue. PR review reproduced the
  race directly (two concurrent `remove` calls on a two-key account could both pass the guard and
  both commit, leaving zero passkeys) and found no such follow-up issue actually existed, so the fix
  landed in this PR instead: the unlocked `SELECT count(*)` became `SELECT id ... ORDER BY id FOR
  UPDATE`, counted in Rust, plus a concurrency test proving it.
- **`reset_member_passkeys`'s second audit write.** This plan's "Rewrite `reset_member_passkeys` in
  `crates/of-web/src/routes/orgs.rs`" step (below) called for keeping the post-commit
  `auth.passkey.cleared` write and re-scoping it to the real `org_id` via a second `tx.audit(...)`
  call. PR review converged on dropping that write entirely instead: it was redundant
  with the already-atomic, already-org-scoped `org.member.passkeys_reset` row, justified only by a
  now-disproven claim that it mirrored a self-service passkey clear (no such caller exists in
  production). `remove_tx` was also simplified back into `remove` directly, since no second caller
  for it ever appeared.
- **The Global Constraints section below is now stale on this same point, and is left unedited
  below for the record.** It asserts that `reset_member_passkeys` "writes to audit_events with a
  real org_id" and that this plan "adds a dedicated cross-org check (Step 8)" for that write.
  Neither shipped: per the divergence above, the write was dropped rather than re-scoped, so there
  is no org-scoped `PASSKEY_CLEARED` row to write a cross-org negative test against. The planned
  cross-org check became moot for that reason; the test that actually shipped
  (`an_admin_can_reset_a_members_authenticator_but_gains_nothing_by_it`, per §4 below) instead
  asserts the row's *absence*, which is the correct proof for a write that no longer happens.

A second, independent `security-auditor` re-review (blind to spec/plan/PR-body) then verified the
`FOR UPDATE` fix empirically and confirmed the audit-write consolidation loses no observable
coverage. Three documentation-accuracy corrections from that round (plus a small deadlock-ordering
hardening) landed in the same PR, touching doc comments in `crates/of-core/src/audit.rs` (the
`PASSKEY_CLEARED` action's doc comment), `crates/of-auth/src/passkeys.rs` (`clear`'s lockout warning
and its actor-attribution note — already listed in the File Structure table below), and
`crates/of-web/src/routes/auth.rs` (`note_claim_refused`'s comment) — the first and third of which
touch files this plan's File Structure table never lists. Full review history is on the PR.

## Global Constraints

These hold for every step below:

- No AI self-attribution anywhere (commits, PR body, comments, docs).
- Run `cargo fmt --all` before every Rust commit.
- Every SQL statement stays where it already lives: `crates/of-auth/src/passkeys.rs` (not a tenant
  table — `passkeys` has no `org_id`) and `crates/of-web/src/routes/orgs.rs` (already inside a
  pinned `Tx`, using `Tx::audit`, not a new raw SQL statement outside `of-core` — `Tx::audit` and
  `Db::audit_global_on` are both `of-core` functions; nothing this plan adds is a bare SQL string
  outside `of-core`).
- Tests need a real Postgres: `podman compose up -d` (port 15433) and a `.env` with `DATABASE_URL`
  (`cp .env.example .env`).
- No tenant table is added or touched — `passkeys` has no `org_id` column, and `audit_events` is an
  existing tenant table whose RLS policy (`0008_audit.sql`) is unchanged by this plan. No cross-org
  negative test applies to `remove`/`clear` (global, no org). §3's `reset_member_passkeys` change
  writes to `audit_events` with a real `org_id` inside an already-pinned `Tx`, using the same
  `Tx::audit` every other org-scoped audit write in this codebase already uses — no new isolation
  surface, so no new cross-org test is required, but the existing `an_admin_can_reset_a_members_authenticator_but_gains_nothing_by_it`
  test (already exercising this org) is updated per Task 1 Step 6, and Task 1 adds a dedicated
  cross-org check (Step 8) confirming the now-org-scoped row is invisible to a *different* org's
  audit trail — the same shape every other `Tx::audit` write is expected to satisfy.
- No MCP tool is added — no `of-billing::classify` entry needed.
- No migration — no schema change.
- No breaking change to a public interface (Non-Negotiable Rule 6) — every change here is
  behavioral (an error path that used to swallow now propagates; one audit row's `org_id` column
  changes from always-`NULL` to the real org for one write path), documented in the spec's
  "Public-interface changes" table and restated in the PR body for the architect reviewer.

## File Structure

| File | Responsibility |
|---|---|
| **Modify.** `crates/of-auth/src/passkeys.rs` | `remove` opens `db.begin_unpinned()` and runs the last-passkey check, the `DELETE`, and the `auth.passkey.removed` audit write on it via a new private `remove_tx` helper; `clear` moves its existing audit write inside its existing transaction, before `commit`. |
| **Modify.** `crates/of-web/src/routes/orgs.rs` | `reset_member_passkeys` moves its post-commit best-effort `auth.passkey.cleared` write into its existing pinned transaction as a second `tx.audit(...)` call, real `org_id`, before `tx.commit()`; the post-commit block is deleted. |
| **Modify.** `crates/of-auth/tests/passkeys.rs` | Two new forced-failure tests (`remove`, `clear`), mirroring `#131`'s `a_forced_audit_failure_rolls_back_the_credential`. |
| **Modify.** `crates/of-web/tests/console.rs` | One updated assertion (`an_admin_can_reset_a_members_authenticator_but_gains_nothing_by_it`, now org-scoped) plus two new tests: a forced-failure rollback test for the whole `reset_member_passkeys` transaction, and a cross-org visibility check confirming the now-org-scoped row is invisible to a different org. |

## Task Order & Rationale

One task. `remove`, `clear`, and `reset_member_passkeys` all use the same two primitives
(`Db::audit_global_on`, `Tx::audit`) and the same forced-failure test technique, and the fix for one
does not depend on the fix for another compiling — but splitting them into separate commits would
leave `#134` only partially closed at each intermediate commit, and the PR is small enough (three
call sites, ~150 lines including tests) that one task reviews as one coherent unit, matching how
`#131` shipped its own two-file change as one task.

## Task 1 — Atomic destruction + audit writes, all three call sites ✅/🚧/⬜: ✅

**Files:** `crates/of-auth/src/passkeys.rs`, `crates/of-web/src/routes/orgs.rs`,
`crates/of-auth/tests/passkeys.rs`, `crates/of-web/tests/console.rs`

**Interfaces:**
- Consumes: `of_core::db::Db::begin_unpinned` (existing), `of_core::audit::Db::audit_global_on`
  (existing, takes `&mut Unpinned` per `#165`), `of_core::audit::Tx::audit` (existing).
- Produces: no new public function signatures — `passkeys::remove_tx` is a new private helper, not
  exported.

Steps:

- [ ] **Baseline: run the existing suites to confirm the starting point is green.**
  `podman compose up -d` (skip if already running), `cp .env.example .env` if no `.env` exists yet,
  then `cargo test -p of-auth --test passkeys` and `cargo test -p of-web --test console`. Expected:
  all tests pass (this establishes the before-state).

- [ ] **Rewrite `passkeys::remove` in `crates/of-auth/src/passkeys.rs`.**
  Replace the `db.pool()`-executed count check, `DELETE`, and best-effort
  `db.audit_global(...)` call (wrapped in `if let Err(e) = ... { tracing::error!(...) }`) with the
  `remove`/`remove_tx` split from spec §1: `remove` opens `let mut tx = db.begin_unpinned().await?;`,
  calls `remove_tx(&mut tx, user, key, ip).await?`, then `tx.commit().await?; Ok(())`. `remove_tx`
  (private, `async fn remove_tx(conn: &mut Unpinned, user: UserId, key: Uuid, ip: Option<&str>) ->
  Result<()>`) runs `SELECT count(*) FROM passkeys WHERE user_id = $1` on `conn.conn()`, the same
  `LastPasskey` check, the same `DELETE` (on `conn.conn()` instead of `db.pool()`), the same
  zero-`rows_affected` → `UnknownCredential` check, then `Db::audit_global_on(conn, Entry::new(action::PASSKEY_REMOVED)...)
  .await?` (propagate via `?`, no more `tracing::error!` swallow). Exact code per spec §1, including
  the comment explaining why the delete and its audit row are now one transaction.

- [ ] **Rewrite `passkeys::clear` in the same file.**
  Move the existing post-commit `if let Err(e) = db.audit_global(...) { tracing::error!(...) }`
  block to run as `Db::audit_global_on(&mut tx, Entry::new(action::PASSKEY_CLEARED)...).await?`
  immediately after `clear_tx(tx.conn(), user).await?` and before `tx.commit().await?`. Exact code
  per spec §2. Update `clear_tx`'s doc comment to drop the now-stale claim that the caller writes
  its audit record "separately, after commit" — `clear`'s own call now writes it before commit;
  `reset_member_passkeys`'s write is addressed in the next step.

- [ ] **Compile check.** `cargo build -p of-auth`. Expected: compiles clean.

- [ ] **Rewrite `reset_member_passkeys` in `crates/of-web/src/routes/orgs.rs`.**
  Add a second `tx.audit(Entry::new(action::PASSKEY_CLEARED)...)` call immediately after the
  existing `tx.audit(Entry::new(action::MEMBER_PASSKEYS_RESET)...)` call, before `tx.commit()`.
  Delete the post-commit `if let Err(e) = state.db.audit_global(...) { tracing::error!(...) }` block
  entirely. Exact code and the replacement comment per spec §3 (explaining why a NULL-org write is
  no longer possible on this pinned connection, and why the row is now org-scoped instead).

- [ ] **Compile check.** `cargo build -p of-web`. Expected: compiles clean.

- [ ] **Update the existing test assertion in `crates/of-web/tests/console.rs`.**
  In `an_admin_can_reset_a_members_authenticator_but_gains_nothing_by_it`, change the `cleared`
  query from `WHERE action = $1 AND org_id IS NULL` to `WHERE action = $1 AND org_id = $2` bound to
  the acme org's id (fetch via `h.db.get_org_by_slug("acme").await.unwrap().unwrap().id`, computed
  once near the top of the test where `org` is already available for `add_member`). Update the
  preceding comment to explain the row is now org-scoped, per spec §4. Run
  `cargo test -p of-web --test console an_admin_can_reset_a_members_authenticator_but_gains_nothing_by_it`
  — expected: passes against the new query.

- [ ] **Add `a_forced_audit_failure_rolls_back_a_passkey_removal` to `crates/of-auth/tests/passkeys.rs`.**
  Mirror `a_forced_audit_failure_rolls_back_the_credential`'s technique: `CREATE FUNCTION` +
  `CREATE TRIGGER ... BEFORE INSERT ON audit_events ... WHEN (NEW.action = 'auth.passkey.removed')
  EXECUTE FUNCTION ...` that `RAISE EXCEPTION`. Register a user with two passkeys (mirror
  `removing_a_passkey_writes_the_passkey_removed_action`'s two-device setup). Call `passkeys::remove`
  on one of them; assert `Err`; assert `passkeys::list(&db, user).await.unwrap().len() == 2` (both
  keys survive — the `DELETE` rolled back). Run
  `cargo test -p of-auth --test passkeys a_forced_audit_failure_rolls_back_a_passkey_removal` —
  expected: passes.

- [ ] **Add `a_forced_audit_failure_rolls_back_a_passkey_clear` to the same file.**
  Same technique, trigger on `NEW.action = 'auth.passkey.cleared'`. Register a user with one
  passkey via `register_new`. Call `passkeys::clear(&db, user, user, None)`; assert `Err`; assert
  `passkeys::has_credential(&db, user).await.unwrap()` is still `true` (the clearing `DELETE` rolled
  back). Run `cargo test -p of-auth --test passkeys a_forced_audit_failure_rolls_back_a_passkey_clear`
  — expected: passes.

- [ ] **Add `a_forced_audit_failure_rolls_back_an_admin_assisted_reset` to `crates/of-web/tests/console.rs`.**
  Same trigger technique, on `NEW.action = 'auth.passkey.cleared'` (this is the write §3 makes
  atomic with the rest of `reset_member_passkeys`'s transaction). Set up an org with an owner and a
  member the same way `an_admin_can_reset_a_members_authenticator_but_gains_nothing_by_it` does.
  `POST /api/orgs/acme/members/{user}/reset-passkeys`; assert a `5xx` response (the existing
  500-class mapping for a propagated DB error — check the exact status the harness's `ApiError`
  conversion produces for a `sqlx`/`of_core::Error` and assert that). Then assert nothing committed:
  the member's original passkey is still present (`passkeys::has_credential`), no session was
  revoked (an existing live session, if any, still resolves), no claim row exists for that user
  (`SELECT count(*) FROM account_claims WHERE user_id = $1` or the equivalent existing table/helper
  — check `of_core::invites` for the actual claim-storage shape used by
  `create_account_claim_tx`), and no `org.member.passkeys_reset` row was written
  (`action_count`-style query). This is what proves the whole transaction — `clear_tx`, session
  revoke, claim insert, and both audit writes — is one atomic unit, not three independent ones that
  happen to run in sequence. Run
  `cargo test -p of-web --test console a_forced_audit_failure_rolls_back_an_admin_assisted_reset` —
  expected: passes.

- [ ] **Add a cross-org visibility check to `crates/of-web/tests/console.rs`.**
  Extend or add alongside `an_admin_can_reset_a_members_authenticator_but_gains_nothing_by_it`: with
  a second org (`org_with_owner(&h, "widgets", &second_owner)`) that has no relationship to the
  reset member, confirm `GET /api/orgs/widgets/audit?actionPrefix=auth.passkey.cleared` (signed in
  as `widgets`'s owner) returns zero rows even though the acme-org reset above wrote one — proving
  the org-scoped `Tx::audit` write from §3 does not leak across orgs. This is the negative test the
  Global Constraints section calls for given §3 changes an existing row's `org_id` from always-`NULL`
  (globally inert) to a real org.

- [ ] **Run the affected suites.**
  `cargo test -p of-auth --test passkeys` and `cargo test -p of-web --test console`. Expected: all
  pass, including the four new/updated tests above.

- [ ] **Run the full workspace suite.**
  `cargo test --workspace`. Expected: green — confirms no other caller of `Db::audit_global`,
  `Db::audit_global_on`, or `Tx::audit` regressed.

- [ ] **Lint and format.**
  `cargo clippy --all-targets -- -D warnings`, then `cargo fmt --all`.

- [ ] **Format and commit.**
  `cargo fmt --all` (again, to catch anything the lint step's fixes touched), then:
  ```
  git add crates/of-auth/src/passkeys.rs crates/of-web/src/routes/orgs.rs \
          crates/of-auth/tests/passkeys.rs crates/of-web/tests/console.rs
  git commit -m "fix(of-auth): make passkey removal and clearing atomic with their audit writes"
  ```

**Out-of-band artifacts:** none touched — no container image, console bundle, Cloudflare Worker, or
migration change in this task. (Vacuously satisfied, not skipped.)

## Record-as-shipped

Done, in this commit: Task 1's marker is flipped to ✅ above, the `## Status` block above records
what actually shipped (including where it diverged from this plan's original steps), and the
spec's `> **Status:**` is flipped to IMPLEMENTED with the merged PR number.
