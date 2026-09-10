# Passkey registration audit: distinguish signup/claim/add, and carry an IP — implementation plan

**Goal:** `finish_registration` writes an `auth.passkey.registered` audit row that names which of its
three callers wrote it (`"signup"` / `"add"` / `"claim"`) and carries the caller's IP, matching what
every neighboring auth event already does. Closes `savvagent/otto-factory#88`.

**Spec:** `docs/specs/2026-09-10-passkey-registration-audit-via-design.md` — read it first. This plan
implements it exactly.

## Status — 2026-09-10

Approved for implementation. No blocking issues from plan review; advisory note: the `add_passkey_finish` (the "add" path) is the one call site gaining a genuinely new `parts: Parts` extractor rather than just a new trailing argument, and no test asserts its specific via/ip values -- the implementer should double-check argument order there with extra care since a transposed via/ip would only be caught by review, not by any test in this plan.

## Global Constraints

- No `unwrap()` outside tests; no silent fallback on a resolution failure.
- Every SQL statement lives in `of-core` — this change adds none, and must not add any elsewhere.
- Run `cargo fmt --all` before every Rust commit.
- Tests need a real Postgres: `podman compose up -d` and a `.env` with `DATABASE_URL` (`cp
  .env.example .env` if not already present in the worktree).
- No AI self-attribution anywhere — commit messages, PR body, code comments.
- This touches the auth spine (`of-auth::passkeys`), so both the audit write's best-effort-ness and
  the account-resolution/ceremony-ownership checks around it must stay byte-for-byte behaviorally
  unchanged except for the two new fields on the `Entry`.

## File Structure

| File | Responsibility |
|---|---|
| **Modify.** `crates/of-auth/src/passkeys.rs` | `finish_registration` gains `via: &str, ip: Option<&str>` parameters; its audit write gains `.detail(...)` and `.from_request(...)`. |
| **Modify.** `crates/of-web/src/routes/auth.rs` | Three call sites (`signup_finish`, `add_passkey_finish`, `claim_finish`) pass `via` and `ip`; `add_passkey_finish` gains a `parts: Parts` extractor. |
| **Modify.** `crates/of-auth/tests/passkeys.rs` | Three existing direct calls to `finish_registration` updated for the new signature; one new test asserting `detail.via` for the claim path. |

## Task Order & Rationale

One task. The three files change together — `finish_registration`'s signature change and its three
callers are not independently compilable, so there is no meaningful intermediate commit between "old
signature" and "new signature, all callers updated."

## Task 1 — Thread `via` and `ip` through `finish_registration` and its three callers

**Files:** `crates/of-auth/src/passkeys.rs`, `crates/of-web/src/routes/auth.rs`,
`crates/of-auth/tests/passkeys.rs`

**Interfaces:**
- Consumes: `of_core::audit::{Entry, action::PASSKEY_REGISTERED}` (existing), `crate::state::client_ip`
  (existing, `of-web`).
- Produces: `of_auth::passkeys::finish_registration`'s new signature (`..., via: &str, ip:
  Option<&str>`), consumed by its three callers in `of-web`.

Steps:

- [ ] **Write the failing test first.** In `crates/of-auth/tests/passkeys.rs`, add a new test near
      `registration_writes_the_passkey_registered_action` (~line 383):

  ```rust
  /// The claim path's row is the one that matters most: it is what proves who
  /// actually walked through the door after an admin-assisted reset (#88).
  #[sqlx::test(migrations = "../of-core/migrations")]
  async fn registration_records_which_flow_wrote_it(pool: PgPool) {
      let db = Db::from_pool(pool);
      let webauthn = rp();
      let mut auth = authenticator();
      let user = register_new(&db, &mut auth).await;

      let ceremony = passkeys::start_registration(&db, &webauthn, Some(user))
          .await
          .unwrap();
      let credential = auth
          .do_registration(
              Url::parse(ORIGIN).unwrap(),
              for_soft_token(ceremony.challenge),
          )
          .expect("the authenticator refused the registration challenge");
      passkeys::finish_registration(
          &db,
          &webauthn,
          ceremony.id,
          &credential,
          None,
          "claim",
          Some("203.0.113.7"),
      )
      .await
      .unwrap();

      let via: serde_json::Value = sqlx::query_scalar(
          "SELECT detail->'via' FROM audit_events \
           WHERE action = $1 AND actor_user_id = $2 \
           ORDER BY created_at DESC LIMIT 1",
      )
      .bind(of_core::audit::action::PASSKEY_REGISTERED)
      .bind(user)
      .fetch_one(db.pool())
      .await
      .unwrap();
      assert_eq!(via, serde_json::json!("claim"));

      let ip: Option<String> = sqlx::query_scalar(
          "SELECT ip FROM audit_events \
           WHERE action = $1 AND actor_user_id = $2 \
           ORDER BY created_at DESC LIMIT 1",
      )
      .bind(of_core::audit::action::PASSKEY_REGISTERED)
      .bind(user)
      .fetch_one(db.pool())
      .await
      .unwrap();
      assert_eq!(ip.as_deref(), Some("203.0.113.7"));
  }
  ```

  This will not compile yet — `finish_registration` does not take `via`/`ip`. That is expected; the
  next steps make it compile and pass.

- [ ] Also update the three existing direct calls in the same file so the crate compiles once the
      signature changes:
  - Line 92 (`register_new`, used pervasively by other tests): `passkeys::finish_registration(db,
    &webauthn, ceremony.id, &credential, Some("laptop"), "signup", None)`.
  - Line 173 (second-key registration in the two-devices test): `passkeys::finish_registration(&db,
    &webauthn, ceremony.id, &credential, Some("phone"), "add", None)`.
  - Line 628 (credential-naming test): `passkeys::finish_registration(&db, &webauthn, first.id,
    &credential, None, "signup", None)`.

- [ ] Run `cargo test -p of-auth --test passkeys` and confirm it fails to compile (the signature
      doesn't exist yet) — this is the "red" of red-green.

- [ ] **Implement.** In `crates/of-auth/src/passkeys.rs`, change `finish_registration`'s signature
      (currently ~line 216) to:

  ```rust
  pub async fn finish_registration(
      db: &Db,
      webauthn: &Webauthn,
      ceremony: Uuid,
      credential: &RegisterPublicKeyCredential,
      nickname: Option<&str>,
      via: &str,
      ip: Option<&str>,
  ) -> Result<UserId> {
  ```

  and change the audit write (currently ~line 255-264) to:

  ```rust
  if let Err(e) = db
      .audit_global(
          Entry::new(action::PASSKEY_REGISTERED)
              .actor(user_id)
              .detail(serde_json::json!({ "via": via }))
              .from_request(ip, None),
      )
      .await
  {
      tracing::error!(
          error = %e,
          user_id = %user_id,
          "failed to write audit event for passkey registration"
      );
  }
  ```

  Nothing else in the function body changes.

- [ ] In `crates/of-web/src/routes/auth.rs`:
  - `signup_finish` (~line 198): change the `finish_registration` call to pass `"signup"` and
    `ip.as_deref()` as the two new trailing arguments. `ip` is already computed at the top of the
    function.
  - `claim_finish` (~line 281): same shape — pass `"claim"` and `ip.as_deref()`. `ip` is already
    computed at the top of the function.
  - `add_passkey_finish` (~line 455): add a `parts: Parts` parameter to the function signature
    (after `caller: CurrentUser`, before `Json(req)`, matching the precedent at
    `crates/of-web/src/routes/orgs.rs:503-506`), add `let ip = client_ip(&parts, &state.config);` as
    the first line of the body, and pass `"add"` and `ip.as_deref()` to the `finish_registration`
    call.

- [ ] Run `cargo test -p of-auth --test passkeys` — all tests, including the new one, pass.

- [ ] Run `cargo test -p of-web --test console` — confirm the existing signup / add-passkey /
      claim-finish endpoint tests still pass unchanged (they assert status and response shape, not
      audit detail).

- [ ] Run `cargo test --workspace` — full suite green.

- [ ] Run `cargo clippy --all-targets -- -D warnings`.

- [ ] **Format and commit.** `cargo fmt --all`, then:

  ```
  git add crates/of-auth/src/passkeys.rs crates/of-web/src/routes/auth.rs crates/of-auth/tests/passkeys.rs
  git commit -m "of-auth: record which flow registered a passkey, and its IP"
  ```

**Out-of-band artifacts:** none touched — no `Dockerfile`/`fly.toml` change, no `web/` change, no
`web/worker/` change, no migration, no `.github/workflows/` change, no `OF_*` config change. State
this explicitly at Phase 5 rather than skipping it.

**Tenant isolation / metering:** not applicable — no tenant table touched, no MCP tool added.

**Breaking-change step:** not applicable — `finish_registration` is an in-workspace function with
every caller updated in this same commit; see the spec's Public-interface changes table.

## ✅ / 🚧 / ⬜ marker

⬜ Task 1 — not started.
