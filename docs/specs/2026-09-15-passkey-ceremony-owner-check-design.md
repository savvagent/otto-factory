# Passkey ceremony-ownership check moves before the write design

> **Status:** DRAFT — closes the `add_passkey_finish` half of issue #109: a ceremony/caller
> mismatch must fail before the credential insert and its audit row, not after.

## Premise corrections

The issue describes the hazard in both `claim_finish` and `add_passkey_finish`. It no longer holds
for `claim_finish`: `savvagent/otto-factory#164` (closing `#132`) already restructured that handler
to defer `tx.commit()` until after the ownership check (`crates/of-web/src/routes/auth.rs:415`,
`if registered != user { drop(tx); ...; return Err(...) }`, checked before the `tx.commit().await?`
a few lines below). Confirmed by reading the current code, not just the issue thread: the credential
INSERT and the `auth.passkey.registered` audit row both happen on `tx`, and a mismatch drops `tx`
unc­ommitted — nothing durable survives a refusal on that path today.

`add_passkey_finish` (`crates/of-web/src/routes/auth.rs:614`) still has the exact hazard the issue
names: it calls `passkeys::finish_registration`, which opens its own transaction and **commits
immediately** inside the function (see that function's own doc comment: "commits immediately after
— for `signup_finish` and `add_passkey_finish`, which have nothing else to fold into the same
commit"), and only *after* that commit does the handler check
`if registered != caller.user.id { return Err(ApiError::forbidden(...)) }`. The credential and its
audit row are already durable by the time that check runs. This spec fixes only this half — the
`claim_finish` half is already closed and is explicitly out of scope (see Scope).

## Scope

**In:**

- `crates/of-auth/src/passkeys.rs`: `finish_registration` and `finish_registration_tx` gain an
  `expected: Option<UserId>` parameter, checked immediately after the ceremony's stored account is
  read (`take_ceremony`) and before anything is written (`webauthn.finish_passkey_registration`, the
  `passkeys` INSERT, the audit row).
- `crates/of-auth/src/error.rs`: a new `AuthError::CeremonyAccountMismatch` variant, with its own
  `status()` (403), `public()` message, and `auth_code()` entry — not a reuse of `CeremonyExpired`
  (see Assumptions for why a shared/generic status code was rejected).
- `crates/of-web/src/routes/auth.rs`: `add_passkey_finish` passes `expected: Some(caller.user.id)`
  and drops its own now-redundant post-hoc `if registered != caller.user.id` check, since a mismatch
  can no longer produce an `Ok` result to check. `signup_finish` passes `expected: None` (no identity
  to compare against — a fresh signup ceremony has no caller session at all). `claim_finish` passes
  `expected: None` to `finish_registration_tx` — its own existing post-hoc check and rollback
  discipline are already correct and are not touched (see Scope: Out).
- Every existing call site of `finish_registration`/`finish_registration_tx` across
  `crates/of-auth/tests/passkeys.rs` (7 call sites) updated for the new parameter.
- A new regression test in `crates/of-web/tests/console.rs`: a ceremony started by one account,
  finished while authenticated as a different account, asserting `403`, no new `passkeys` row for
  the credential, and no `PASSKEY_REGISTERED` audit row for the attempt.
- A new focused test in `crates/of-auth/tests/passkeys.rs` exercising `finish_registration`'s
  `expected` parameter directly: a mismatch returns `Err` before any row exists in `passkeys`.

**Out:**

- `claim_finish` and its `note_claim_refused`/`CLAIM_REFUSED` audit path. It is already fixed by
  `#164`/`#132`, with better failure attribution than a bare `expected` check could give it for
  free: on a mismatch, `claim_finish` attributes the refusal to `registered` (the ceremony's actual
  owner, whose ownership a real signature just proved) rather than to `user` (the account merely
  named by the claim code) — see `crates/of-web/src/routes/auth.rs:417-421`'s own comment.
  Routing `claim_finish` through the new `expected` check would collapse that richer attribution
  into a generic error unless `AuthError::CeremonyAccountMismatch` carried the mismatched
  `UserId` as data for `claim_finish` to inspect — a second unit of complexity spent re-deriving a
  distinction `claim_finish` already makes correctly on its own. `claim_finish` passes
  `expected: None` purely so the function signature stays callable; nothing about its behavior,
  audit attribution, or the correctness `#164` already established changes.
- Adding a "registration refused" audit event for `add_passkey_finish`'s mismatch case, mirroring
  `claim_finish`'s `note_claim_refused`. The issue's complaint is specifically that a *false*
  success record gets written for a refused request — stopping that write is the fix. A *true*
  refusal record is a real, separate enhancement (worth its own ticket if wanted), not implied by
  this bug report, and adding one here would be scope creep.
- Any change to the WebAuthn ceremony/ceremony-storage schema, `take_ceremony`'s single-use
  semantics, or the unique index on `passkeys.credential_id`. The ceremony row is already deleted by
  `take_ceremony`'s `DELETE ... RETURNING` regardless of what happens next (single-use, unconditional
  today) — that behavior is unchanged.
- Any change to `signup_finish`'s behavior. It already has no caller identity to compare against
  (unauthenticated, first-touch); `expected: None` is a no-op for it, matching today exactly.

## Assumptions

- **A dedicated `AuthError` variant, not a reuse of `CeremonyExpired`.** `CeremonyExpired` maps to
  HTTP 400 (`crates/of-auth/src/error.rs`'s `status()`, `_ => 400` catch-all) and code
  `ceremony_expired`. Both existing handlers return 403 today via `ApiError::forbidden(...)`
  (`crates/of-web/src/error.rs`'s `forbidden()` — `StatusCode::FORBIDDEN`, code `"forbidden"`).
  Moving the check into `of-auth` must not silently turn a 403 into a 400 for a legitimate caller
  hitting this path — `Error::code()` is documented in `CLAUDE.md` as "the stable machine-readable
  branch point," and status codes are the coarser half of that same contract. `CeremonyAccountMismatch`
  is added to `status()`'s existing 403 arm (alongside `NotAMember`/`SsoRequired`) and given its own
  `auth_code()` entry (`ceremony_account_mismatch`), matching this file's established one-variant-one-code
  style rather than a special case.
- **The stable `code` value changes from the generic `"forbidden"` to the new, more specific
  `"ceremony_account_mismatch"`.** This is judged acceptable, not a Non-Negotiable-Rule-6 breaking
  change: `"forbidden"` was never a bespoke code for this failure — it is `ApiError::forbidden`'s
  shared generic string, used by unrelated call sites elsewhere in `of-web` for whatever 403 they
  return. Going from a shared generic code to a dedicated, more specific one is the same direction
  `of-auth`'s own `auth_code()` doc comment already argues for ("Separate codes, because these say
  *what to do*..."), and it only fires on an actual ceremony-substitution attempt — not a path a
  well-behaved client exercises in normal operation.
- **`expected` is `Option<UserId>`, not a required parameter, because one real caller has nothing to
  compare against.** `signup_finish`'s ceremony has no session and no claim — there is no "expected"
  identity independent of the ceremony itself, so `None` is the honest value, not a placeholder.
- **The message text is shared across both real 403 sites** (`add_passkey_finish` now, and any
  future caller that adopts `expected`) rather than parameterized per-caller. This repo's own
  `public()` doc comment already argues for collapsing distinct causes into one string where the
  distinction doesn't change what the caller should do (start over / use the right account) —
  the same reasoning applies here, and it keeps the new variant a plain unit variant rather than one
  carrying a message string that would have to be threaded through call sites.
- **No `Eq`/`PartialEq` gap.** `UserId` (`of_core::ids`, via the `uuid_id!` macro) already derives
  `PartialEq, Eq`, so `user_id != expected` needs no new trait bound anywhere in the call chain.

## Goal & Success Criteria

Goal: a ceremony/caller mismatch on `add_passkey_finish` fails before the credential is inserted and
before the audit row is written, so a refused request can never be misread later as a completed
registration.

Success criteria:

1. `add_passkey_finish` returns `403` on a ceremony/caller mismatch, exactly as it does today —
   the HTTP status is unchanged for the caller.
2. On that mismatch, no row is inserted into `passkeys` for the credential the request presented.
3. On that mismatch, no `auth.passkey.registered` audit row is written for the attempt.
4. `signup_finish`'s and `claim_finish`'s existing behavior is bit-for-bit unchanged — both pass
   `expected: None`/keep their own logic, and every existing test involving either continues to
   pass unmodified.
5. Every pre-existing call site of `finish_registration`/`finish_registration_tx` compiles against
   the new signature with the correct `expected` value for its own semantics (`None` for six of the
   seven `of-auth` test call sites and for `signup_finish`, `Some(caller.user.id)` only for
   `add_passkey_finish`).
6. `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --all
   --check` all pass.

## Approach

### 1. `crates/of-auth/src/error.rs`: a dedicated 403 variant

```rust
/// The ceremony's stored account does not match what the caller expected — a
/// substituted or hijacked ceremony (see `finish_registration`'s `expected`
/// parameter). Checked before anything the ceremony completing would write,
/// so a mismatch fails before the credential insert and its audit row exist
/// at all, not just before they commit. `savvagent/otto-factory#109`.
#[error("that ceremony belongs to a different account")]
CeremonyAccountMismatch,
```

Add to `status()`'s existing 403 arm:

```rust
AuthError::NotAMember | AuthError::SsoRequired | AuthError::CeremonyAccountMismatch => 403,
```

Add to `public()`:

```rust
AuthError::CeremonyAccountMismatch => "that ceremony belongs to a different account",
```

Add to `of-web`'s `auth_code()` (`crates/of-web/src/error.rs`):

```rust
AuthError::CeremonyAccountMismatch => "ceremony_account_mismatch",
```

### 2. `crates/of-auth/src/passkeys.rs`: the check, before any write

```rust
pub async fn finish_registration(
    db: &Db,
    webauthn: &Webauthn,
    ceremony: Uuid,
    credential: &RegisterPublicKeyCredential,
    nickname: Option<&str>,
    via: RegistrationVia,
    expected: Option<UserId>,
    ip: Option<&str>,
) -> Result<UserId> {
    let mut tx = db.begin_unpinned().await?;
    let user_id = finish_registration_tx(
        &mut tx, webauthn, ceremony, credential, nickname, via, expected, ip,
    )
    .await?;
    tx.commit().await?;
    Ok(user_id)
}

pub async fn finish_registration_tx(
    conn: &mut Unpinned,
    webauthn: &Webauthn,
    ceremony: Uuid,
    credential: &RegisterPublicKeyCredential,
    nickname: Option<&str>,
    via: RegistrationVia,
    expected: Option<UserId>,
    ip: Option<&str>,
) -> Result<UserId> {
    let (user_id, state): (Option<UserId>, PasskeyRegistration) =
        take_ceremony(conn.conn(), ceremony, "register").await?;
    let user_id = user_id.ok_or(AuthError::CeremonyExpired)?;

    if let Some(expected) = expected {
        if user_id != expected {
            return Err(AuthError::CeremonyAccountMismatch);
        }
    }

    let passkey = webauthn
        .finish_passkey_registration(credential, &state)
        .map_err(webauthn_failed)?;
    // ...unchanged from here: the INSERT and the audit write.
}
```

`take_ceremony`'s `DELETE ... RETURNING` still runs first and is unconditional — the ceremony is
consumed (single-use) whether or not `expected` matches, exactly as today.

### 3. `crates/of-web/src/routes/auth.rs`: the three call sites

- `signup_finish` (line ~201): add `None,` before the `ip.as_deref()` argument. No other change.
- `claim_finish` (line ~387): add `None,` to its `finish_registration_tx` call, in the same
  position. Its own `if registered != user { ... }` check below (line ~415), the `drop(tx)`, and
  `note_claim_refused` all stay exactly as they are.
- `add_passkey_finish` (line ~614): pass `Some(caller.user.id)` in that position, and delete the
  now-unreachable post-hoc check:

  ```rust
  pub async fn add_passkey_finish(
      State(state): State<AppState>,
      caller: CurrentUser,
      parts: Parts,
      Json(req): Json<FinishRegistration>,
  ) -> ApiResult<Response> {
      let ip = client_ip(&parts, &state.config);
      passkeys::finish_registration(
          &state.db,
          &state.webauthn,
          req.ceremony_id,
          &req.credential,
          req.nickname.as_deref(),
          passkeys::RegistrationVia::Add,
          Some(caller.user.id),
          ip.as_deref(),
      )
      .await?;

      Ok(http::StatusCode::NO_CONTENT.into_response())
  }
  ```

  The return value no longer needs binding or comparing — `finish_registration` cannot return `Ok`
  with a mismatched account any more, so there is nothing left to check.

## Error Handling & Edge Cases

- **A mismatch races a concurrent, legitimate finish of the same ceremony.** `take_ceremony`'s
  `DELETE ... RETURNING` is the linearization point — only one caller ever observes a non-empty
  result for a given ceremony id, so there is no window where two calls both see the row and one
  "wins" after writes have started. Unaffected by this change; already true today.
- **`expected` is `Some` but the ceremony's stored `user_id` genuinely matches.** No behavior change
  from today — proceeds exactly as before.
- **A caller retries after a `CeremonyAccountMismatch`.** The ceremony row is already gone
  (`take_ceremony` deleted it on the first attempt), so a retry with the same `ceremony_id` hits
  `AuthError::CeremonyExpired`, not a second mismatch — same as any other single-use ceremony
  failure today.

## Testing Approach

- **`crates/of-auth/tests/passkeys.rs`**: a new test starting a registration ceremony for one
  account and calling `finish_registration` with `expected: Some(<a different account>)`, asserting
  the call returns `Err` and that `passkeys::count` for both accounts is unchanged (nothing was
  inserted). This isolates the ordering fix at the function level, independent of HTTP/session
  plumbing.
- **`crates/of-web/tests/console.rs`**: a new `#[sqlx::test]` mirroring the existing
  `add_passkey_finish_records_the_add_flow_and_its_ip` fixture (same harness/session pattern):
  start a passkey-add ceremony as account A, then call `POST /api/me/passkeys/finish` authenticated
  as account B with A's ceremony id and a credential registered against it. Assert `403`, assert no
  row in `passkeys` for that credential's `credential_id`, and assert no
  `auth.passkey.registered` audit row exists for either account from this attempt (query
  `audit_events` the same way the existing IP-recording test does).
- Full suite: `cargo test --workspace` (needs `podman compose up -d` + `.env` with `DATABASE_URL`),
  `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`.

## Risks & Open Questions

- **Checked, not a risk**: the `code` value for this 403 changes from `"forbidden"` to
  `"ceremony_account_mismatch"` for `add_passkey_finish` specifically (see Assumptions).
  `grep -rn "'forbidden'\|\"forbidden\"" web/src` finds exactly one hit
  (`web/src/lib/poll-fatal.test.ts:40`), a generic `fatalApiFailure`-for-any-403 test unrelated to
  this endpoint or its code string. Nothing in `web/src` branches on `"forbidden"` specifically.
