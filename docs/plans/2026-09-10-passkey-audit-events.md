# Passkey audit events — real action names, no more silent drops

## Goal

Passkey registration and clearing write their own `auth.passkey.*` audit actions instead of
borrowing `auth.totp.*`; a failed write on either path reaches the logs instead of vanishing; and a
stored credential that fails to deserialize logs an `error` line naming the user, on both paths that
can hit it, with no change to what either path returns.

## Status — 2026-09-10

🚧 Not started.

**Spec:** `docs/specs/2026-09-10-passkey-audit-events-design.md` — read it first. This plan
implements it exactly. Closes `savvagent/otto-factory#76`.

## Global Constraints

- **No SQL changes, no migration.** The write path already goes through `Db::audit_global`; only
  the `Entry::new(...)` action string argument at two call sites changes. No new `INSERT`/`UPDATE`
  statement anywhere.
- **`audit_events` rows already written are never touched.** No `UPDATE` is issued against
  `audit_events` — the table has no `UPDATE` policy under `FORCE ROW LEVEL SECURITY`
  (`0008_audit.sql`), so one would fail for every role regardless. `TOTP_ENROLLED` / `TOTP_RESET`
  stay defined, re-documented as historical only.
- **No behavior change on the two deserialization-drop paths.** `finish_authentication` must still
  reach `InvalidCredentials` via `keys.is_empty()` when every stored key is corrupt (or absent);
  `update_stored_credential` must still return `Ok(())` when its one row fails to decode. Only a
  `tracing::error!` call is added to each branch.
- **Both audit writes stay best-effort.** Registration and clearing must still succeed even if the
  audit write itself fails — the log call replaces `let _ = ...`, it does not turn the write into a
  `?`.
- **No SQL leaves `of-core`.** `audit.rs`'s doc-comment edits are comment-only; the two `of-auth`
  call sites already call an existing `of-core` method (`Db::audit_global`) and add no new query.
- **No AI self-attribution** anywhere — commits, comments, docs, PR body.
- Rust gates before every commit: `cargo fmt --all`, then `cargo clippy --all-targets -- -D
  warnings` and `cargo test --workspace`. Tests need `podman compose up -d` (Postgres 16 on host
  port 15433) and a `.env` (`cp .env.example .env`); there are no database mocks.
- Out-of-band artifacts: **none touched.** No migration, no `web/` change, no container/Worker/CI
  change, no new `OF_*` key. Stated explicitly per the fast-path/out-of-band checklist rather than
  skipped.

## File Structure

| File | Responsibility |
| --- | --- |
| **Modify.** `crates/of-core/src/audit.rs` | Add `PASSKEY_REGISTERED` / `PASSKEY_CLEARED`; re-document `TOTP_ENROLLED` / `TOTP_RESET` as historical. |
| **Modify.** `crates/of-auth/src/passkeys.rs` | `finish_registration` and `clear` write the new constants and log a failed write; `finish_authentication` and `update_stored_credential` log a deserialization failure without changing behavior. |
| **Modify.** `crates/of-auth/tests/passkeys.rs` | Assert the new action constants are what registration and clearing write; assert a corrupted stored credential still reaches the existing error, not a panic or a different one. |

## Task Order & Rationale

One task. All three changes touch the same two files, in the same review pass, and none has a
dependency the others need staged first — splitting them would mean three commits each re-running
the same `cargo test -p of-auth --test passkeys` gate for no isolation benefit. Steps are ordered
failing-test-first within the task: the constant-rename assertions land first (they are the
smallest, most mechanical change), then the corrupted-credential regression test, then the
implementation that makes both pass together.

## Task 1 — Real action names, logged writes, logged decode failures

⬜

**Files:** `crates/of-core/src/audit.rs`, `crates/of-auth/src/passkeys.rs`,
`crates/of-auth/tests/passkeys.rs`

**Interfaces:** Consumes `of_core::audit::{action, Entry}`, `of_core::Db::audit_global` (both
pre-existing, unchanged signatures). Produces two new `pub const` action strings in
`of_core::audit::action`; no new function, no new public type.

- [ ] Add a helper to `crates/of-auth/tests/passkeys.rs` alongside the existing `login_failures`:

  ```rust
  /// How many rows this account has under a given action — the same query shape
  /// as `login_failures`, parameterized so it covers the new passkey actions too.
  async fn audit_rows(db: &Db, user: UserId, action: &str) -> i64 {
      sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE action = $1 AND actor_user_id = $2")
          .bind(action)
          .bind(user)
          .fetch_one(db.pool())
          .await
          .unwrap()
  }
  ```

  (`login_failures` may now delegate to this or stay as its own one-liner — either is fine; do not
  delete `login_failures`, other tests call it by name.)

- [ ] In `a_passkey_creates_an_account_and_signs_back_into_it`, after `register_new` returns `user`,
      add:

  ```rust
  assert_eq!(
      audit_rows(&db, user, of_core::audit::action::PASSKEY_REGISTERED).await,
      1,
      "registering the account's first key must write auth.passkey.registered"
  );
  ```

  This is a **failing test right now** — `PASSKEY_REGISTERED` does not exist yet. Run it to confirm
  the failure is "no such constant" (a compile error), not a runtime assertion failure:
  `cargo test -p of-auth --test passkeys a_passkey_creates_an_account_and_signs_back_into_it` — expect
  a compile failure naming `PASSKEY_REGISTERED`.

- [ ] In `clearing_passkeys_leaves_no_way_in`, after `passkeys::clear(&db, user, None).await.unwrap()`,
      add:

  ```rust
  assert_eq!(
      audit_rows(&db, user, of_core::audit::action::PASSKEY_CLEARED).await,
      1,
      "clearing an account's keys must write auth.passkey.cleared"
  );
  ```

  Same expected failure mode (compile error) for now.

- [ ] Add a new test for the corrupted-credential regression, placed near
      `a_known_credential_with_a_bad_signature_is_still_invalid_credentials`:

  ```rust
  /// A stored credential's `credential` JSON can stop deserializing — a
  /// webauthn-rs upgrade changing the wire shape is the realistic cause. The
  /// account must still get a real answer, not a panic, when that key is the
  /// only one it has.
  #[sqlx::test(migrations = "../of-core/migrations")]
  async fn a_corrupted_stored_credential_is_invalid_not_a_panic(pool: PgPool) {
      let db = Db::from_pool(pool);
      let mut auth = authenticator();
      let user = register_new(&db, &mut auth).await;
      let ids = credential_ids(&db, user).await;

      sqlx::query("UPDATE passkeys SET credential = $1 WHERE user_id = $2 AND credential_id = $3")
          .bind(serde_json::json!({ "not": "a passkey" }))
          .bind(user)
          .bind(&ids[0])
          .execute(db.pool())
          .await
          .unwrap();

      match sign_in(&db, &mut auth, &ids[0]).await {
          Err(AuthError::InvalidCredentials) => {}
          other => panic!("a corrupted stored credential answered {other:?}"),
      }
  }
  ```

  Run it now: `cargo test -p of-auth --test passkeys a_corrupted_stored_credential_is_invalid_not_a_panic`
  — this one already passes against today's code (the `.ok()` in `filter_map` already drops the row
  silently), which is expected: it is a **regression guard for the rewrite below**, not a test of
  new behavior. Confirm it passes before touching `passkeys.rs`, so a later failure is unambiguously
  the rewrite's fault.

- [ ] In `crates/of-core/src/audit.rs`, in `pub mod action`, replace:

  ```rust
  pub const TOTP_ENROLLED: &str = "auth.totp.enrolled";
  pub const TOTP_RESET: &str = "auth.totp.reset";
  ```

  with:

  ```rust
  /// Historical only. TOTP was removed from this product; rows with this action
  /// predate `PASSKEY_REGISTERED` and are not rewritten. Nothing writes this
  /// constant anymore.
  pub const TOTP_ENROLLED: &str = "auth.totp.enrolled";
  /// Historical only, for the same reason as `TOTP_ENROLLED`. Superseded by
  /// `PASSKEY_CLEARED`.
  pub const TOTP_RESET: &str = "auth.totp.reset";
  pub const PASSKEY_REGISTERED: &str = "auth.passkey.registered";
  pub const PASSKEY_CLEARED: &str = "auth.passkey.cleared";
  ```

- [ ] In `crates/of-auth/src/passkeys.rs`, `finish_registration`, replace:

  ```rust
  let _ = db
      .audit_global(Entry::new(action::TOTP_ENROLLED).actor(user_id))
      .await;
  ```

  with:

  ```rust
  if let Err(e) = db
      .audit_global(Entry::new(action::PASSKEY_REGISTERED).actor(user_id))
      .await
  {
      tracing::error!(error = %e, user_id = %user_id, "failed to write audit event for passkey registration");
  }
  ```

- [ ] In the same file, `clear`, replace:

  ```rust
  let _ = db
      .audit_global(
          Entry::new(action::TOTP_RESET)
              .actor(user)
              .from_request(ip, None),
      )
      .await;
  ```

  with:

  ```rust
  if let Err(e) = db
      .audit_global(
          Entry::new(action::PASSKEY_CLEARED)
              .actor(user)
              .from_request(ip, None),
      )
      .await
  {
      tracing::error!(error = %e, user_id = %user, "failed to write audit event for passkey clear");
  }
  ```

- [ ] In `finish_authentication`, replace:

  ```rust
  let keys: Vec<DiscoverableKey> = stored
      .into_iter()
      .filter_map(|raw| serde_json::from_value::<Passkey>(raw).ok())
      .map(|p| DiscoverableKey::from(&p))
      .collect();
  ```

  with:

  ```rust
  let keys: Vec<DiscoverableKey> = stored
      .into_iter()
      .filter_map(|raw| match serde_json::from_value::<Passkey>(raw) {
          Ok(passkey) => Some(passkey),
          Err(e) => {
              tracing::error!(
                  error = %e,
                  user_id = %user_id,
                  "stored passkey credential failed to deserialize; skipping it"
              );
              None
          }
      })
      .map(|p| DiscoverableKey::from(&p))
      .collect();
  ```

  (`user_id` is already bound above this point via the `let Some(user_id) = owner else { ... }`
  earlier in the function — no new binding needed.)

- [ ] In `update_stored_credential`, replace:

  ```rust
  let Some(raw) = raw else { return Ok(()) };
  let Ok(mut passkey) = serde_json::from_value::<Passkey>(raw) else {
      return Ok(());
  };
  ```

  with:

  ```rust
  let Some(raw) = raw else { return Ok(()) };
  let mut passkey = match serde_json::from_value::<Passkey>(raw) {
      Ok(passkey) => passkey,
      Err(e) => {
          tracing::error!(
              error = %e,
              user_id = %user,
              "stored passkey credential failed to deserialize during a sign-counter update; skipping it"
          );
          return Ok(());
      }
  };
  ```

- [ ] Run `cargo test -p of-auth --test passkeys` — all tests in the file, including the three
      touched/added above, must pass. In particular re-confirm
      `a_corrupted_stored_credential_is_invalid_not_a_panic` still passes after the `filter_map`
      rewrite (this is the regression the rewrite could introduce if the match arms were
      transposed).
- [ ] Run `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all`.
- [ ] `git commit -m "of-auth: name passkey audit events for real, stop dropping failed writes silently"`

## Out-of-band verification

Vacuously satisfied — no migration, no `web/` change, no container image, Cloudflare Worker, or CI
workflow change, no new `OF_*` config key. Stated explicitly rather than skipped.
