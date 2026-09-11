# `claim_finish`'s claim code and ceremony become part of its own transaction — implementation plan

**Spec:** `docs/specs/2026-09-11-claim-finish-atomic-consume-design.md` — read it first. This plan
implements it exactly.

## Goal

Closes `savvagent/otto-factory#132`: `claim_finish` (`crates/of-web/src/routes/auth.rs`) currently
spends the claim code (`consume_account_claim`, autocommitted) and — inside
`passkeys::finish_registration` — the WebAuthn ceremony (`take_ceremony`, also autocommitted) both
*before* the credential-insert-and-audit-write transaction `#131` added even opens. A failure
anywhere in that transaction, including the ceremony-ownership mismatch check that already runs
after it, burns both single-use secrets for nothing, leaving the account with no passkey and no
outstanding claim. This plan folds all four operations — claim consumption, ceremony consumption,
credential insert, audit write — into one transaction that commits only after the ownership check
passes, with two new tests proving the rollback deterministically.

## Status — 2026-09-11

Not started. Three tasks, sequential (Task 2 depends on nothing from Task 1's code but the plan
orders `of-core` before `of-auth` before `of-web` to match the dependency direction of the crates
that consume each new function; Task 3 depends on both).

## Global Constraints

- No `unwrap()` outside tests. No silent fallback on a resolution failure.
- No AI self-attribution anywhere (commits, PR body, comments, code).
- Run `cargo fmt --all` before every Rust commit.
- Every SQL statement lives in `of-core` — the two SQL statements this plan touches
  (`account_claims`'s `UPDATE`, `webauthn_ceremonies`'s `DELETE`) already do; nothing moves out of
  `of-core`/`of-auth`'s existing SQL ownership (the ceremony `DELETE` lives in `of-auth::passkeys`
  today, unchanged by this plan — `of-auth` is the auth-spine crate that already owns WebAuthn
  ceremony storage, not `of-core`; this plan does not relocate it).
- `account_claims`, `webauthn_ceremonies`, and `passkeys` are global tables with no `org_id` column
  — Load-Bearing Invariant 1 (cross-org negative test) does not apply to any task here.
- No MCP tool is added or changed — no `of-billing::classify` entry needed.
- No migration, no schema change, no out-of-band artifact (container image, console bundle,
  Cloudflare Worker, config surface) is touched by this plan.
- Tests need a real Postgres: `podman compose up -d` (host port 15433) and a `.env` with
  `DATABASE_URL` (`cp .env.example .env`).
- Errors are written for an LLM/API caller that has never read the docs — unchanged in this plan;
  no new error variant is introduced anywhere.

## File Structure

| File | Responsibility |
|---|---|
| `crates/of-core/src/invites.rs` | **Modify.** New `consume_account_claim_tx` free function; `Db::consume_account_claim` becomes a thin wrapper over it. |
| `crates/of-auth/src/passkeys.rs` | **Modify.** `take_ceremony` widened to a generic executor; new `finish_registration_tx`; `finish_registration` becomes a two-line wrapper over it. |
| `crates/of-auth/tests/passkeys.rs` | **Modify.** New test `a_forced_audit_failure_also_restores_the_ceremony`. |
| `crates/of-web/src/routes/auth.rs` | **Modify.** `claim_finish` restructured to drive one transaction across all four operations. |
| `crates/of-web/tests/console.rs` | **Modify.** New test `a_credential_collision_during_claim_finish_leaves_the_claim_code_usable`. |

## Task Order & Rationale

Task 1 (`of-core`) introduces `consume_account_claim_tx`, the primitive `claim_finish` needs to hold
the claim's consumption on a caller-supplied connection. Task 2 (`of-auth`) introduces
`finish_registration_tx` and widens `take_ceremony`, the primitives `claim_finish` needs for the
ceremony/credential/audit half, and adds the test proving `finish_registration`'s existing
self-committing wrapper now also restores the ceremony on a forced failure — provable without
touching `of-web` at all, so it lands before the `of-web` change that depends on both new functions.
Task 3 (`of-web`) wires both into `claim_finish` and adds the end-to-end regression test that
reproduces `#132`'s exact reported hazard (a credential-insert failure) and proves the claim code
survives it.

---

## Task 1 — `of-core`: `consume_account_claim_tx` ⬜

**Files:** `crates/of-core/src/invites.rs`
**Interfaces:** produces `pub async fn consume_account_claim_tx(conn: &mut sqlx::PgConnection, token_hash: &[u8]) -> Result<UserId>`, consumed by Task 3.

No new of-core-level test is added for this task. `consume_account_claim`/`peek_account_claim`/
`create_account_claim` have never had a dedicated of-core unit test — they are exercised exclusively
through `crates/of-web/tests/console.rs`'s reset/reclaim flows today (confirmed: no
`consume_account_claim`/`account_claim` reference anywhere under `crates/of-core/tests/` or
`crates/of-auth/tests/`). This plan follows that established precedent rather than introducing a new
test-investment shape for one function; the new function is instead exercised for real by Task 3's
end-to-end test, the same way its sibling `create_account_claim_tx` is exercised only through
`crates/of-web/tests/console.rs`'s `reset_member_passkeys` flow.

- [ ] Read `crates/of-core/src/invites.rs`'s existing `create_account_claim`/`create_account_claim_tx`
      pair (lines ~255–330) once more immediately before editing, to match its doc-comment and
      last-use-moves-the-reference style exactly.
- [ ] Add `pub async fn consume_account_claim_tx(conn: &mut sqlx::PgConnection, token_hash: &[u8]) -> Result<UserId>`
      as a free function (not a `Db` method — matching `create_account_claim_tx`), with the same
      `UPDATE account_claims SET consumed_at = now() WHERE token_hash = $1 AND consumed_at IS NULL
      AND expires_at > now() RETURNING user_id` statement `Db::consume_account_claim` already runs,
      bound against `conn` instead of `self.pool()`. Doc comment cites `savvagent/otto-factory#132`
      and names `claim_finish` as the caller, mirroring `create_account_claim_tx`'s own doc comment
      citing `#87` and `reset_member_passkeys`.
- [ ] Rewrite `Db::consume_account_claim` to open its own unpinned transaction, delegate to
      `consume_account_claim_tx(&mut tx, token_hash)`, and commit — the same shape
      `create_account_claim`/`create_account_claim_tx` already use for their pair. Keep its existing
      doc comment ("Spend a claim. One statement...") — still true: the `UPDATE ... RETURNING` is
      still the one statement that decides atomically whether the claim was live; the wrapping
      transaction adds no second statement that could race it.
- [ ] `cargo build -p of-core` — confirms the new function and the rewritten wrapper compile and that
      nothing else in `of-core` references the old body in a way that breaks.
- [ ] `cargo test -p of-core` — full crate suite must stay green (no existing test touches
      `consume_account_claim`, so this is a compile-and-no-regression check, not new coverage).
- [ ] Format and commit: `cargo fmt --all` then
      `git commit -m "of-core: add consume_account_claim_tx, a connection-taking claim consumption"`.

---

## Task 2 — `of-auth`: `finish_registration_tx`, widened `take_ceremony`, new ceremony-restoration test ⬜

**Files:** `crates/of-auth/src/passkeys.rs`, `crates/of-auth/tests/passkeys.rs`
**Interfaces:** consumes nothing from Task 1; produces `pub async fn finish_registration_tx(tx: &mut sqlx::PgConnection, webauthn: &Webauthn, ceremony: Uuid, credential: &RegisterPublicKeyCredential, nickname: Option<&str>, via: RegistrationVia, ip: Option<&str>) -> Result<UserId>`, consumed by Task 3.

- [ ] Write the failing test first. In `crates/of-auth/tests/passkeys.rs`, add
      `a_forced_audit_failure_also_restores_the_ceremony`, adjacent to the existing
      `a_forced_audit_failure_rolls_back_the_credential` (same `BEFORE INSERT ON audit_events`
      trigger technique — copy its setup verbatim). After the forced `finish_registration` call
      returns `Err`, additionally assert
      `SELECT count(*) FROM webauthn_ceremonies WHERE id = $1` (bound to the ceremony's own `id`,
      captured from `passkeys::start_registration`'s return before the call) is `1` — proving the
      ceremony row was restored by the rollback, not silently gone regardless of outcome. Under
      today's code (ceremony deleted autocommitted, before `finish_registration`'s transaction
      opens) this assertion fails with `0` even on a passing build; it must go red against the
      current implementation before Task 2's implementation steps proceed.
- [ ] `cargo test -p of-auth --test passkeys a_forced_audit_failure_also_restores_the_ceremony` —
      confirm it fails (red) against the unmodified source, for exactly the reason above (ceremony
      count is `0`, not `1`).
- [ ] In `crates/of-auth/src/passkeys.rs`, widen `take_ceremony`'s signature from
      `async fn take_ceremony<T: serde::de::DeserializeOwned>(db: &Db, id: Uuid, kind: &str)` to
      `async fn take_ceremony<'e, T, E>(conn: E, id: Uuid, kind: &str) -> Result<(Option<UserId>, T)>
      where T: serde::de::DeserializeOwned, E: sqlx::PgExecutor<'e>` — mirroring
      `crates/of-core/src/audit.rs`'s `Entry::write<'e, E>` precedent from `#131` exactly (same
      bound, same lifetime naming). Body unchanged except `.fetch_optional(db.pool())` becomes
      `.fetch_optional(conn)`.
- [ ] Update `finish_authentication`'s call site: `take_ceremony(db, ceremony, "authenticate")`
      becomes `take_ceremony(db.pool(), ceremony, "authenticate")`.
- [ ] Add `pub async fn finish_registration_tx(tx: &mut sqlx::PgConnection, webauthn: &Webauthn,
      ceremony: Uuid, credential: &RegisterPublicKeyCredential, nickname: Option<&str>, via:
      RegistrationVia, ip: Option<&str>) -> Result<UserId>`, moving `finish_registration`'s existing
      body into it verbatim except: (a) `take_ceremony(db, ceremony, "register")` becomes
      `take_ceremony(&mut *tx, ceremony, "register")`; (b) the `let mut tx = db.begin_unpinned().await?;`
      line and the trailing `tx.commit().await?;` are removed — this function does not open or
      commit a transaction, it runs on the caller's; (c) the credential-insert `.execute(&mut *tx)`
      and `Db::audit_global_on(&mut *tx, ...)` calls keep reborrowing `tx` exactly as today except
      the audit call's `&mut *tx` becomes a bare `tx` (its last use in the function, matching
      `create_account_claim_tx`'s last-use-moves-the-reference convention — see Task 1). Doc comment
      per the spec §2, citing `#132` and naming `claim_finish` as the caller that needs this half
      split out.
- [ ] Rewrite `finish_registration` to a two-line wrapper: open `db.begin_unpinned()`, call
      `finish_registration_tx(&mut tx, webauthn, ceremony, credential, nickname, via, ip).await?`,
      `tx.commit().await?`, return the user id. Doc comment updated per spec §2 to note the split.
- [ ] `cargo test -p of-auth --test passkeys a_forced_audit_failure_also_restores_the_ceremony` —
      confirm it now passes (green).
- [ ] `cargo test -p of-auth --test passkeys` — full suite, including
      `a_forced_audit_failure_rolls_back_the_credential` and every test that calls
      `passkeys::finish_registration` directly (`register_new` and its callers) — must stay green
      unchanged, since `finish_registration`'s signature and observable behavior on success are
      identical.
- [ ] `cargo test -p of-auth` — full crate suite.
- [ ] `cargo clippy -p of-auth --all-targets -- -D warnings` — the new generic bound on
      `take_ceremony` is the one place clippy could plausibly object (needless lifetime, unused
      type param); confirm it is clean, matching `Entry::write`'s already-clean precedent.
- [ ] Format and commit: `cargo fmt --all` then
      `git commit -m "of-auth: split finish_registration into a connection-taking half"`.

---

## Task 3 — `of-web`: atomic `claim_finish`, end-to-end regression test ⬜

**Files:** `crates/of-web/src/routes/auth.rs`, `crates/of-web/tests/console.rs`
**Interfaces:** consumes `of_core::invites::consume_account_claim_tx` (Task 1) and
`of_auth::passkeys::finish_registration_tx` (Task 2).

- [ ] Write the failing test first. In `crates/of-web/tests/console.rs`, add
      `a_credential_collision_during_claim_finish_leaves_the_claim_code_usable`, adjacent to the
      existing claim tests (`a_reset_account_cannot_be_claimed_without_the_code` and the reclaim
      happy-path test above it). Sequence:
      1. `onboard` an owner (`rob`), create org `acme`, `onboard` a member (`bob`), add `bob` with
         `Role::Member`.
      2. `Call::post("/api/orgs/acme/members/{bob}/reset-passkeys")` as `rob`, capture the returned
         claim `code`.
      3. `Call::post("/api/auth/claim/start")` with the code, capture the ceremony challenge.
      4. Build a fresh `common::authenticator()` and drive `auth.do_registration(...)` directly
         (mirroring `common::unregistered_credential`'s pattern) against the softened challenge to
         obtain a real `RegisterPublicKeyCredential` — **without** POSTing it to `claim/finish` yet.
      5. Extract `credential.raw_id.as_ref().to_vec()` and, via `h.db.pool()`, `INSERT INTO passkeys
         (user_id, credential_id, credential, nickname) VALUES ($1, $2, $3, $4)` binding `rob.user`
         (an already-registered, unrelated account), that exact raw credential id, a placeholder
         `serde_json::json!({})` for the opaque `credential` column, and a nickname — forcing the
         unique index on `credential_id` to reject `claim_finish`'s own insert of the same id with
         `AuthError::CredentialAlreadyRegistered`, the literal pre-`#131` failure mode `#132`'s
         report names.
      6. `Call::post("/api/auth/claim/finish")` with the code, the ceremony id, and the credential —
         assert it does **not** return `200` (any 4xx from `CredentialAlreadyRegistered`'s mapping
         is correct; assert on `!= StatusCode::OK` rather than pinning an exact code, matching this
         test file's existing style at `a_reset_account_cannot_be_claimed_without_the_code`).
      7. **The assertion this test exists for:** `Call::post("/api/auth/claim/start")` with the
         *same* code again — assert it returns `200`. Under today's code this fails (the code was
         already burned by step 6's `claim_finish` call before the credential insert was even
         attempted); this is the red bar to clear.
      8. Complete a full reclaim to prove actual end-to-end recoverability, not just that the code
         "looks" valid: a fresh `common::authenticator()`, drive `common::finish_registration` (the
         existing test helper) against the new `claim/start` challenge from step 7, assert `200`,
         and assert the returned `user.id` equals `bob.user` — the same shape as the existing
         reclaim happy-path test just above this one in the file.
- [ ] `cargo test -p of-web --test console a_credential_collision_during_claim_finish_leaves_the_claim_code_usable`
      — confirm it fails (red) against the unmodified `claim_finish`, specifically at step 7's
      assertion (the retried `claim/start` returns non-`200`).
- [ ] In `crates/of-web/src/routes/auth.rs`, rewrite `claim_finish` per spec §3: open
      `let mut tx = state.db.begin_unpinned().await?;`, call
      `of_core::invites::consume_account_claim_tx(&mut tx, &hash_claim(&req.code)).await?` for
      `user`, call `passkeys::finish_registration_tx(&mut tx, &state.webauthn, req.ceremony_id,
      &req.credential, req.nickname.as_deref(), passkeys::RegistrationVia::Claim, ip.as_deref())
      .await?` for `registered`, keep the existing `if registered != user { return
      Err(ApiError::forbidden(...)) }` check unchanged in wording but now positioned before the
      commit, then `tx.commit().await.map_err(of_core::Error::from)?;`, then the existing
      `login::with_passkey` + `signed_in_response` tail unchanged. Update the function's doc comment
      per spec §3.
- [ ] `cargo test -p of-web --test console a_credential_collision_during_claim_finish_leaves_the_claim_code_usable`
      — confirm it now passes (green).
- [ ] `cargo test -p of-web --test console` — full suite, including
      `a_reset_account_cannot_be_claimed_without_the_code` and the reclaim happy-path test, must
      stay green unchanged (the success and 403 response shapes are byte-identical to today's).
- [ ] `cargo test --workspace` — everything, one final pass.
- [ ] `cargo clippy --all-targets -- -D warnings`.
- [ ] `cargo fmt --all --check`.
- [ ] Format and commit: `cargo fmt --all` then
      `git commit -m "of-web: make claim_finish's claim, ceremony, credential, and audit writes atomic"`.

---

## Out-of-band artifacts

None touched by this plan (vacuously satisfied, per `SKILL.md` Phase 5 step 14): no `Dockerfile`,
`fly.toml`, `web/`, `web/worker/`, migration, or `.github/workflows/` change. `cargo test --workspace`
plus `cargo clippy`/`cargo fmt` are the complete verification surface for this plan.
