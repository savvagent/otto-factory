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
  `expected: Option<UserId>` parameter, checked after the ceremony's stored account is read
  (`take_ceremony`) and after signature verification (`webauthn.finish_passkey_registration`), but
  before either database write it guards (the `passkeys` INSERT, the audit row) — see the Addendum
  for why the check waits for verification rather than running immediately after `take_ceremony`.
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
  carrying a message string that would have to be threaded through call sites. This does change the
  response body's `message` text for `add_passkey_finish`'s 403 — from today's bespoke "that
  ceremony belongs to another account" to `public()`'s new "that ceremony belongs to a different
  account" — a one-word wording drift. Only `code` is documented as the stable, machine-readable
  contract (`CLAUDE.md`'s Style section); `message` is prose for a human/agent to read, not branch
  on, so this is not a breaking change, just worth naming so a reviewer skimming only for the `code`
  change isn't surprised by the body text also moving.
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
///
/// Carries both accounts (post-review addition — see the Addendum) so
/// `finish_registration` can write a trace of a hijack attempt on a
/// connection independent of the transaction this error rolls back.
#[error("that ceremony belongs to a different account")]
CeremonyAccountMismatch {
    ceremony_account: UserId,
    caller_account: UserId,
},
```

Add to `status()`'s existing 403 arm:

```rust
AuthError::NotAMember | AuthError::SsoRequired | AuthError::CeremonyAccountMismatch { .. } => 403,
```

Add to `public()`:

```rust
AuthError::CeremonyAccountMismatch { .. } => "that ceremony belongs to a different account",
```

Add to `of-web`'s `auth_code()` (`crates/of-web/src/error.rs`):

```rust
AuthError::CeremonyAccountMismatch { .. } => "ceremony_account_mismatch",
```

### 2. `crates/of-auth/src/passkeys.rs`: the check, before any write

Both functions' existing doc comments (`finish_registration`'s and `finish_registration_tx`'s, at
their current locations) get a line naming `expected` and the ordering guarantee it establishes —
e.g. "`expected`, when `Some`, is checked against the ceremony's stored account after signature
verification and before anything is written; `None` skips the check entirely, for a caller (like
`signup_finish`) with no independent identity to compare against." This makes the guarantee
discoverable from the function itself, not only from `AuthError::CeremonyAccountMismatch`'s doc
comment. (Corrected by the Addendum: the check runs *after* signature verification, not
immediately after `take_ceremony` — see item 3 there for why, and `finish_registration`'s own body
below now also writes a refusal audit row on mismatch, per item 2.)

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
    match finish_registration_tx(&mut tx, webauthn, ceremony, credential, nickname, via, expected, ip)
        .await
    {
        Ok(user_id) => {
            tx.commit().await?;
            Ok(user_id)
        }
        // Post-review addition (Addendum item 2): a best-effort refusal
        // audit row, on a connection independent of `tx` (which rolls back
        // here) — see `finish_registration`'s actual doc comment for why.
        Err(AuthError::CeremonyAccountMismatch { ceremony_account, caller_account }) => {
            drop(tx);
            let entry = Entry::new(action::PASSKEY_REGISTRATION_REFUSED)
                .actor(ceremony_account)
                .detail(serde_json::json!({ "attemptedBy": caller_account.to_string() }))
                .from_request(ip, None);
            if let Err(e) = db.audit_global(entry).await {
                tracing::error!(error = %e, "failed to write audit event for a refused passkey registration");
            }
            Err(AuthError::CeremonyAccountMismatch { ceremony_account, caller_account })
        }
        Err(e) => Err(e),
    }
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

    // Verification before the `expected` check, not after — deliberately,
    // per Addendum item 3: checking first would let the check (and, since
    // item 2, the audit row it writes) be probed with an unsigned credential
    // body, since the ceremony survives the rollback either way.
    let passkey = webauthn
        .finish_passkey_registration(credential, &state)
        .map_err(webauthn_failed)?;

    if let Some(expected) = expected {
        if user_id != expected {
            return Err(AuthError::CeremonyAccountMismatch {
                ceremony_account: user_id,
                caller_account: expected,
            });
        }
    }
    // ...unchanged from here: the INSERT and the audit write.
}
```

`take_ceremony`'s `DELETE ... RETURNING` still runs first, unconditionally. It does **not**,
however, mean the ceremony is consumed on a mismatch: `finish_registration` propagates the error
with `?` before `tx.commit()`, so the whole transaction — the DELETE included — rolls back with
everything else, exactly the property `#132` established for this transaction. A mismatch leaves
the ceremony exactly as redeemable as it was before the attempt; see the corrected Error Handling
note below.

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
- **A caller retries after a `CeremonyAccountMismatch`.** Corrected from an earlier draft of this
  spec, which claimed the ceremony was already consumed at this point — it is not.
  `finish_registration`'s `?` propagates the error before `tx.commit()`, so the transaction
  (including `take_ceremony`'s DELETE) rolls back, and the ceremony survives exactly as `#132`'s
  fix intends: `crates/of-auth/tests/passkeys.rs`'s `a_forced_audit_failure_also_restores_the_ceremony`
  proves the identical mechanism on this same function. A retry with the same `ceremony_id` — by
  the ceremony's real owner, say, after a substitution attempt was refused — hits the `expected`
  check again with the ceremony intact, not `AuthError::CeremonyExpired`. This is the better
  behavior: a hijack attempt does not get to burn the legitimate owner's in-flight ceremony.

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

- **This grep-based check was wrong, and the addendum below is the correction.** An earlier draft
  claimed `grep -rn "'forbidden'\|\"forbidden\"" web/src` (one hit, an unrelated generic-403 test)
  proved nothing in `web/src` cared about the `code` string changing. Review caught that this is
  the wrong tool for the job: `web/src/lib/errors.ts`'s `KNOWN` map branches on the code via a
  **bare object key** (`forbidden: () => m.error_forbidden()`), which that grep pattern cannot
  match at all. See the Addendum for what this actually meant and how it was fixed.

## Addendum — console translation gap, an audit trail for the refusal, and its ordering (post-review)

Two things review found that this document's original text got wrong or left incomplete:

1. **The console silently lost translation for this exact error.** Before this change,
   `add_passkey_finish`'s 403 carried `code: "forbidden"`, which `web/src/lib/errors.ts`'s `KNOWN`
   map already translates (`m.error_forbidden()`, in all six locales). The new
   `"ceremony_account_mismatch"` code had no entry, so `messageFor()` fell through to the server's
   English `message` for every non-English locale — a real regression, reachable in an ordinary
   (non-attack) scenario: two open tabs, sign out of one account and into another, then finish the
   stale tab's pending ceremony. **Fixed:** added `error_ceremony_account_mismatch` to all six
   `web/messages/*.json` catalogs and a matching entry in `errors.ts`'s `KNOWN` map.
2. **A hijack attempt left no trace anywhere.** `CeremonyAccountMismatch` was originally a unit
   variant: the function that raises it discards both the ceremony's real account and the caller's
   claimed one the moment it constructs the error, and since the whole point of the fix is that
   nothing commits on this path, a genuine substitution attempt against `add_passkey_finish`
   produced neither a `passkeys` row, an audit row, nor a log line — nothing an operator or the
   ceremony's real owner could ever find. **Fixed:** `CeremonyAccountMismatch` now carries
   `{ ceremony_account, caller_account }`; `finish_registration` (not `finish_registration_tx` —
   see that function's own doc comment for why the wrapper is where this belongs) writes a
   best-effort `auth.passkey.registration_refused` audit row on a connection independent of the
   rolled-back transaction, mirroring `claim_finish`'s existing `note_claim_refused` pattern for
   the identical reason: the write that would prove the attempt happened is exactly the write this
   fix prevents from being on the same transaction as the attempt.
3. **Adding that audit write turned an ordering choice this document didn't discuss into a real
   cost.** The `expected` check originally ran immediately after `take_ceremony`, before signature
   verification — review (independently, twice) pointed out that this lets an authenticated caller
   probe a live ceremony id with an unsigned, syntactically-valid credential body: the ceremony
   survives the rollback either way, so nothing capped how many times this 403 could be produced
   for free. Before item 2's fix that was merely an unusual reachability note; after it, each free
   probe also appends an audit row, making the cost of an unbounded probe an unbounded log. **Fixed:**
   the check now runs after `webauthn.finish_passkey_registration` succeeds, restoring the property
   this path had before the fix — reaching it requires an authenticator to have actually signed the
   challenge — and matching `claim_finish`'s own ordering (it checks after `finish_registration_tx`
   returns, for the same reason). This does not weaken the fix itself: the check still runs before
   both database writes it guards (the `passkeys` INSERT and the audit row), which is the actual
   guarantee issue #109 asked for.

**Out of scope for this PR, filed as a separate issue:** independent security review also found
that WebAuthn registration ceremonies are discriminated only by `kind = "register"`, not by which
flow (signup / add-passkey / claim) created them — so `POST /api/auth/signup/finish`, which is
unauthenticated and always passes `expected: None`, can redeem a ceremony started by `claim_start`
or `add_passkey_start`. This is pre-existing (not introduced by this PR, and not made worse by it),
but it is the same threat model issue #109 is about, reachable through the one endpoint this PR
does not touch. Closing it needs ceremonies to carry which flow minted them (e.g. a `kind` per flow,
or a separate `flow` column), which is a larger, separate design — tracked as its own issue rather
than folded into this one.
