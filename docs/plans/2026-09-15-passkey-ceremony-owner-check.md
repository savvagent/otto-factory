# Passkey ceremony-ownership check moves before the write — implementation plan

**Spec:** [`docs/specs/2026-09-15-passkey-ceremony-owner-check-design.md`](../specs/2026-09-15-passkey-ceremony-owner-check-design.md)
— read it first. This plan implements it exactly.

Goal: `add_passkey_finish` must fail a ceremony/caller mismatch before the credential is inserted
and before the `auth.passkey.registered` audit row is written, so a refused request can never
leave a record asserting it succeeded — closing the remaining half of `savvagent/otto-factory#109`
(`claim_finish`'s half was already closed by `#164`/`#132`).

## Status — 2026-09-15

Not started. Two tasks: `of-auth` (the shared function + its own tests) first, then `of-web`
(the three call sites + the HTTP-level regression test), since `of-web` cannot compile against
the new signature until `of-auth` ships it.

## Global Constraints

- No AI self-attribution anywhere (commit messages, code comments, docs, PR body).
- Run `cargo fmt --all` before every Rust commit.
- No SQL changes anywhere — `webauthn_ceremonies`, `passkeys`, and `account_claims` carry no
  `org_id` and no tenant-isolation policy (per `crates/of-web/src/routes/auth.rs`'s own comment on
  `claim_finish`'s `begin_unpinned()` call); this fix is pure application-layer ordering, so no
  cross-org test is owed and none is added.
- This touches the auth spine (`of-auth`: passkey ceremonies) directly — never fast-path eligible,
  regardless of size. The full spec + plan + critique loop already ran; this plan is the result.
- No password, no email, no recovery-code reintroduction — untouched by this change.
- Tests need a real Postgres: `podman compose up -d` and a `.env` with `DATABASE_URL`
  (`cp .env.example .env`) before `cargo test -p of-auth` / `cargo test -p of-web`.
- **Task 1 alone does not make `cargo test --workspace` pass** — `of-web`'s `auth.rs` still calls
  the old three-parameter-shorter signature until Task 2 lands. Task 1's own gate is
  `cargo test -p of-auth` (plus `cargo build --workspace` to confirm the *shape* of the breakage is
  exactly "of-web hasn't been updated yet," not something else). The full-workspace gate runs only
  after Task 2.
- Non-Negotiable Rule 6: the `code` string for `add_passkey_finish`'s existing 403 changes from the
  generic `"forbidden"` to a dedicated `"ceremony_account_mismatch"` — per the spec's Assumptions,
  this is additive/refining, not breaking (status code stays 403; `"forbidden"` was never a
  documented per-endpoint contract). No `docs/clients/matrix.md` update needed — this isn't a
  client-registration or redirect-URI concern.

## File Structure

| File | Responsibility |
|---|---|
| **Modify.** `crates/of-auth/src/error.rs` | New `AuthError::CeremonyAccountMismatch` variant: `status()` (403), `public()` message. |
| **Modify.** `crates/of-auth/src/passkeys.rs` | `finish_registration`/`finish_registration_tx` gain `expected: Option<UserId>`, checked after `take_ceremony` and after signature verification, before either write it guards. Doc comments updated. |
| **Modify.** `crates/of-auth/tests/passkeys.rs` | 7 existing call sites updated for the new parameter; one new test proving a mismatch returns `Err` before any `passkeys` row exists. |
| **Modify.** `crates/of-web/src/error.rs` | `auth_code()` gains `AuthError::CeremonyAccountMismatch => "ceremony_account_mismatch"`. |
| **Modify.** `crates/of-web/src/routes/auth.rs` | `signup_finish`/`claim_finish` pass `expected: None`; `add_passkey_finish` passes `Some(caller.user.id)` and drops its now-unreachable post-hoc check. |
| **Modify.** `crates/of-web/tests/console.rs` | New `#[sqlx::test]`: a ceremony/caller mismatch on `POST /api/me/passkeys/finish` returns 403 and writes neither a `passkeys` row nor a `PASSKEY_REGISTERED` audit row. |

## Task Order & Rationale

`of-auth` first: it owns `finish_registration`/`finish_registration_tx` and the new `AuthError`
variant, and `of-web` is a downstream consumer that cannot compile against the new signature until
it exists. Doing it in this order also means Task 1's own test (function-level, no HTTP/session
plumbing) proves the ordering fix in isolation before Task 2 proves it end-to-end through the
actual handler that was broken.

## Task 1 — `of-auth`: `expected` parameter + dedicated error variant ⬜

**Files:** `crates/of-auth/src/error.rs`, `crates/of-auth/src/passkeys.rs`,
`crates/of-auth/tests/passkeys.rs`

**Interfaces:** `finish_registration`/`finish_registration_tx` change signature (new
`expected: Option<UserId>` parameter); produces the new `AuthError::CeremonyAccountMismatch`
variant, consumed by `of-web` in Task 2.

- [ ] Add the failing test first. In `crates/of-auth/tests/passkeys.rs`, near
      `a_forced_audit_failure_rolls_back_the_credential` (which already establishes the
      start-ceremony → do_registration → assert-zero-rows pattern this test reuses), add:

      ```rust
      /// `expected`, when `Some`, must be checked before anything is written — a
      /// mismatch must not leave a live credential or an audit row for a request
      /// the caller never actually authorized. `savvagent/otto-factory#109`.
      #[sqlx::test(migrations = "../of-core/migrations")]
      async fn a_ceremony_account_mismatch_writes_nothing(pool: PgPool) {
          let db = Db::from_pool(pool);
          let webauthn = rp();
          let mut auth = authenticator();

          // Two accounts: the ceremony belongs to `owner`, but the caller
          // claims to be `other` — the substitution this check exists to catch.
          // `create_unclaimed_user` is the same primitive `start_registration`
          // itself uses to back a signup ceremony with `user: None` — a real,
          // signable-into account, with no ceremony/session plumbing needed to
          // get one for this test.
          let owner = db.create_unclaimed_user().await.unwrap().id;
          let other = db.create_unclaimed_user().await.unwrap().id;

          let ceremony = passkeys::start_registration(&db, &webauthn, Some(owner))
              .await
              .unwrap();
          let credential = auth
              .do_registration(
                  Url::parse(ORIGIN).unwrap(),
                  for_soft_token(ceremony.challenge),
              )
              .expect("the authenticator refused the registration challenge");

          let result = passkeys::finish_registration(
              &db,
              &webauthn,
              ceremony.id,
              &credential,
              Some("laptop"),
              passkeys::RegistrationVia::Add,
              Some(other),
              None,
          )
          .await;

          assert!(
              matches!(result, Err(AuthError::CeremonyAccountMismatch)),
              "expected a CeremonyAccountMismatch, got {result:?}"
          );

          let passkey_count: i64 = sqlx::query_scalar("SELECT count(*) FROM passkeys")
              .fetch_one(db.pool())
              .await
              .unwrap();
          assert_eq!(
              passkey_count, 0,
              "a ceremony/caller mismatch must not leave a credential behind"
          );

          let audit_count: i64 = sqlx::query_scalar(
              "SELECT count(*) FROM audit_events WHERE action = $1",
          )
          .bind(of_core::audit::action::PASSKEY_REGISTERED)
          .fetch_one(db.pool())
          .await
          .unwrap();
          assert_eq!(
              audit_count, 0,
              "a rejected request must not leave a row asserting it succeeded"
          );
      }
      ```

- [ ] Run `cargo test -p of-auth --test passkeys a_ceremony_account_mismatch_writes_nothing` and
      confirm it fails to compile (the `expected` parameter and `AuthError::CeremonyAccountMismatch`
      don't exist yet).
- [ ] In `crates/of-auth/src/error.rs`, add the new variant per spec §1: the `#[error(...)]` unit
      variant, its addition to `status()`'s existing `NotAMember | SsoRequired` 403 arm (now
      `NotAMember | SsoRequired | CeremonyAccountMismatch`), and its `public()` message
      (`"that ceremony belongs to a different account"`).
- [ ] In `crates/of-auth/src/passkeys.rs`, add `expected: Option<UserId>` to both
      `finish_registration` and `finish_registration_tx` per spec §2, positioned after `via` and
      before `ip` in both signatures. Add the check immediately after
      `let user_id = user_id.ok_or(AuthError::CeremonyExpired)?;` and before
      `webauthn.finish_passkey_registration(...)`. Update both functions' doc comments per the
      spec's note in Approach §2 (name `expected` and the ordering guarantee).
- [ ] Update all 7 pre-existing call sites in `crates/of-auth/tests/passkeys.rs` (every existing
      call to `passkeys::finish_registration(...)` — grep the file for `finish_registration(` to
      find all of them; the new test just added is a separate, brand-new function and is not one of
      these 7) to pass `None` in the new parameter position. None of these tests' behavior or
      assertions change — this is purely a signature-compatibility edit.
- [ ] Run `cargo test -p of-auth --test passkeys` (the whole file) and confirm every test passes,
      including the new one and all 7 pre-existing call sites unchanged in behavior.
- [ ] Run `cargo build --workspace` and confirm the *only* errors are in `of-web`: missing arguments
      to `finish_registration`/`finish_registration_tx` in `crates/of-web/src/routes/auth.rs`'s
      three call sites, and a non-exhaustive match on `AuthError` in `crates/of-web/src/error.rs`'s
      `auth_code()` (the new variant has no wildcard arm to fall back on, so the compiler catches
      this one too, not just the call sites) — i.e. the of-auth side is fully consistent and Task 2
      is the only remaining work.
- [ ] `cargo clippy -p of-auth --all-targets -- -D warnings` and `cargo fmt --all`.
- [ ] Commit: `git commit -m "of-auth: check ceremony ownership before any registration write"`.

## Task 2 — `of-web`: the three call sites + the HTTP-level regression test ⬜

**Files:** `crates/of-web/src/error.rs`, `crates/of-web/src/routes/auth.rs`,
`crates/of-web/tests/console.rs`

**Interfaces:** consumes `of_auth::passkeys::finish_registration`'s new signature and
`AuthError::CeremonyAccountMismatch` (Task 1); produces no new interface — `add_passkey_finish`
keeps its existing route, method, and response shapes (403 on mismatch, 204 on success).

- [ ] Add the failing test first. In `crates/of-web/tests/console.rs`, near the existing
      `add_passkey_finish_records_the_add_flow_and_its_ip` (that test builds its harness by hand
      because it needs a custom `client_ip_header`; this one doesn't, so it uses the plainer
      `common::harness(pool)` helper other tests in this file already use), add:

      ```rust
      /// The `add_passkey_finish` half of `savvagent/otto-factory#109`: a ceremony
      /// started by one account, finished while authenticated as a different one,
      /// must be refused before the credential and its audit row exist — not
      /// after they've already committed.
      #[sqlx::test(migrations = "../of-core/migrations")]
      async fn add_passkey_finish_refuses_a_ceremony_started_by_another_account(pool: PgPool) {
          let h = common::harness(pool);

          // `onboard` itself drives a real signup ceremony for each account, so
          // each already has exactly one `passkeys` row and one
          // `PASSKEY_REGISTERED` audit row before the mismatch attempt below —
          // the zero-row assertions this test cares about must be scoped to
          // `other` and expect that pre-existing one, not a bare table count
          // (which `add_passkey_finish_records_the_add_flow_and_its_ip` already
          // gets right by filtering on `actor_user_id`; this test follows the
          // same discipline).
          let owner = onboard(&h, "owner@acme.test").await;
          let other = onboard(&h, "other@acme.test").await;

          // The ceremony is started while authenticated as `owner` ...
          let started = Call::post("/api/me/passkeys/start")
              .with_session(&owner.session)
              .send(&h.router)
              .await;
          started.expect(StatusCode::OK);

          let mut challenge: webauthn_rs::prelude::CreationChallengeResponse =
              serde_json::from_value(started.body["challenge"].clone()).unwrap();
          if let Some(selection) = challenge.public_key.authenticator_selection.as_mut() {
              selection.require_resident_key = false;
              selection.resident_key = None;
          }
          let mut device = common::authenticator();
          let credential = device
              .do_registration(
                  webauthn_rs::prelude::Url::parse(common::PUBLIC_URL).unwrap(),
                  challenge,
              )
              .expect("the authenticator refused the registration challenge");

          // ... but finished while authenticated as `other`.
          let finished = Call::post("/api/me/passkeys/finish")
              .with_session(&other.session)
              .json(serde_json::json!({
                  "ceremonyId": started.body["ceremonyId"].as_str().unwrap(),
                  "credential": credential,
              }))
              .send(&h.router)
              .await;
          finished.expect(StatusCode::FORBIDDEN);
          assert_eq!(finished.error_code(), Some("ceremony_account_mismatch"));

          // Scoped to `owner` — the account whose ceremony was substituted,
          // and the account `finish_registration_tx` would have written the
          // leaked credential/audit row under (it reads the account to write
          // from the ceremony's own stored `user_id`, never from the caller
          // who happened to call `finish`). Scoping this to `other` instead
          // — an earlier draft of both this test and this plan did — checks
          // nothing: `other`'s own counts were never affected by the pre-fix
          // bug either, so that version passed unmodified against the
          // vulnerable code (verified empirically during review).
          let passkey_count: i64 = sqlx::query_scalar("SELECT count(*) FROM passkeys WHERE user_id = $1")
              .bind(owner.user)
              .fetch_one(h.db.pool())
              .await
              .unwrap();
          assert_eq!(
              passkey_count, 1,
              "a ceremony/caller mismatch must not leave a second credential behind on `owner`, \
               the ceremony's real account"
          );

          let audit_count: i64 = sqlx::query_scalar(
              "SELECT count(*) FROM audit_events WHERE action = $1 AND actor_user_id = $2",
          )
          .bind(of_core::audit::action::PASSKEY_REGISTERED)
          .bind(owner.user)
          .fetch_one(h.db.pool())
          .await
          .unwrap();
          assert_eq!(
              audit_count, 1,
              "a refused request must not add a second row asserting `owner` completed a \
               registration nobody but the ceremony's substituted caller attempted"
          );

          // Post-review addition: the refusal itself still leaves a trace,
          // on a connection independent of the one that rolled back, naming
          // both the ceremony's real account and who attempted to finish it.
          let refusal: (of_core::ids::UserId, serde_json::Value) = sqlx::query_as(
              "SELECT actor_user_id, detail FROM audit_events WHERE action = $1",
          )
          .bind(of_core::audit::action::PASSKEY_REGISTRATION_REFUSED)
          .fetch_one(h.db.pool())
          .await
          .unwrap();
          assert_eq!(refusal.0, owner.user);
          assert_eq!(
              refusal.1["attemptedBy"],
              serde_json::json!(other.user.to_string())
          );
      }
      ```

      `onboard`'s return type (`common::Account`) has `.user: UserId` and `.session: String` fields
      — confirmed against `crates/of-web/tests/common/mod.rs`, used exactly this way above.
- [ ] Run the new test and confirm it currently fails to compile (Task 1's signature change means
      every call site in `auth.rs` is currently broken, so nothing in this crate compiles yet —
      expected at this point in the plan).
- [ ] In `crates/of-web/src/error.rs`, add `AuthError::CeremonyAccountMismatch =>
      "ceremony_account_mismatch",` to `auth_code()`'s match (compiler will refuse to build without
      it — the match has no wildcard arm).
- [ ] In `crates/of-web/src/routes/auth.rs`:
      - `signup_finish` (~line 201): add `None,` in the new parameter position (after
        `passkeys::RegistrationVia::Signup,`, before `ip.as_deref(),`).
      - `claim_finish` (~line 387): add `None,` to its `finish_registration_tx` call, same
        position. Do not touch anything else in this function — its own `if registered != user`
        check, `drop(tx)`, and `note_claim_refused` calls stay exactly as they are.
      - `add_passkey_finish` (~line 614): pass `Some(caller.user.id)` in that position; delete the
        `let registered = ` binding (no longer needed) and the now-unreachable
        `if registered != caller.user.id { return Err(ApiError::forbidden(...)) }` block per spec
        §3's code sample.
- [ ] Run `cargo build --workspace` and confirm it compiles clean.
- [ ] Run the new test again and confirm it passes.
- [ ] Run `cargo test -p of-web --test console` (the whole file) and confirm no regression — in
      particular `add_passkey_finish_records_the_add_flow_and_its_ip` (the success path must be
      completely unaffected) and every `claim_finish`-related test (`a_credential_collision_during_claim_finish_leaves_the_claim_code_usable`,
      `a_forced_audit_failure_during_claim_finish_also_restores_the_claim`, and any other test whose
      name contains `claim`).
- [ ] Run `cargo test --workspace` (the full suite) and confirm everything passes.
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo fmt --all`.
- [ ] Commit: `git commit -m "of-web: pass the caller's identity through to the ceremony check"`.

## Final gate (both tasks)

- [x] `cargo test --workspace`
- [x] `cargo clippy --all-targets -- -D warnings`
- [x] `cargo fmt --all --check`

Originally: no `web/` change in this plan, since nothing under `web/src` was believed to read
`AuthError`'s `code` string for this specific path. **Corrected post-review — see the addendum
below**: that check (a grep for the literal string `"forbidden"`) missed `web/src/lib/errors.ts`'s
bare-object-key lookup, and a `web/` change was in fact required.

## Addendum — console translation gap, an audit trail for the refusal, and its ordering (post-review) ✅

Review of the PR found three gaps this plan's original two tasks didn't cover, all now fixed in
the same PR (not deferred to a follow-up, since all three are small and directly related to what
Task 2 shipped):

- [x] **Console translation.** Added `error_ceremony_account_mismatch` to all six
      `web/messages/{en,es,de,fr,it,hi}.json` catalogs and a matching entry in
      `web/src/lib/errors.ts`'s `KNOWN` map, so `add_passkey_finish`'s 403 stays translated in
      every locale instead of falling back to the server's English message (which is what
      happened for the new, more specific `code` this plan introduced). Verified with
      `npm run check` (message-catalog completeness), `npm run lint`, and `npm test`.
- [x] **An audit trail for the refusal.** `AuthError::CeremonyAccountMismatch` changed from a unit
      variant to `{ ceremony_account, caller_account }`, and `finish_registration` (the wrapper,
      not `finish_registration_tx`) now writes a best-effort `auth.passkey.registration_refused`
      audit row on a connection independent of the rolled-back transaction when it catches this
      specific error — mirroring `claim_finish`'s existing `note_claim_refused` pattern, for the
      same reason: the one write that would prove a hijack attempt happened is exactly the write
      this fix prevents from ever committing. New action constant:
      `of_core::audit::action::PASSKEY_REGISTRATION_REFUSED`. Both new tests
      (`crates/of-auth/tests/passkeys.rs`'s `a_ceremony_account_mismatch_writes_nothing` and
      `crates/of-web/tests/console.rs`'s `add_passkey_finish_refuses_a_ceremony_started_by_another_account`)
      were extended to assert on the two account fields and, at the HTTP level, on the new audit
      row and the response's `error.code`.
- [x] **Moved the check after signature verification.** Adding the audit write above turned an
      unresolved ordering question into a real cost: the `expected` check originally ran
      immediately after `take_ceremony`, before `webauthn.finish_passkey_registration`, which let
      an authenticated caller probe a live ceremony id with an unsigned credential body — the
      ceremony survives the rollback either way, so nothing capped how many times this 403 (now
      also an audit row) could be produced. Moved the check to after signature verification
      succeeds, restoring the pre-fix property that reaching this path requires an authenticator to
      have actually signed the challenge, and matching `claim_finish`'s own ordering. The check
      still runs before both writes it guards (the `passkeys` INSERT and the audit row), which is
      the actual guarantee issue #109 needs — only its position relative to verification changed.

Also filed as a separate, out-of-scope follow-up issue (not folded into this plan): registration
ceremonies are discriminated only by `kind = "register"`, not by which flow created them, so
`signup_finish` can redeem a ceremony started by `claim_start` or `add_passkey_start`. Pre-existing,
not introduced or worsened by this plan's changes — see the spec's own Addendum for the full
description.
