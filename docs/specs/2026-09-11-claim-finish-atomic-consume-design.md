# `claim_finish`'s claim code and ceremony become part of its own transaction

> **Status:** IMPLEMENTED — merged as `savvagent/otto-factory#164` (`546bc6393e0926a8cb486b8d1c40046c19239707`),
> closing `savvagent/otto-factory#132`. Filed during the mandatory review trio on
> `savvagent/otto-factory#131` (`docs/specs/2026-09-10-passkey-registration-atomic-audit-design.md`,
> which made `finish_registration`'s credential insert and audit write atomic and, in doing so,
> widened this gap's probability surface). Related but explicitly out of scope: `#109` tracks a
> different ordering concern (the ceremony-ownership check happening after the write) on the same
> function's `claim_finish`/`add_passkey_finish` paths — its claim-code half is now closed by this
> change.
>
> `#164`'s own review trio surfaced two more gaps this design's Risks section did not anticipate,
> both closed before merge: `claim_finish` had no throttle of its own, and a rejected request left
> no audit trace. See the PR's review-response commits for the fix (a `throttle_by_source` call and
> a best-effort `auth.claim.refused` audit write on rollback) and the two additional regression
> tests they came with.

## Goal & Success Criteria

`crates/of-web/src/routes/auth.rs`'s `claim_finish` spends two single-use secrets —
`consume_account_claim` (an autocommitted `UPDATE`) and, inside `passkeys::finish_registration`,
the WebAuthn ceremony (an autocommitted `DELETE`, via `take_ceremony`) — **before** the credential
insert and audit write's own transaction (added in `#131`) even opens. If that transaction fails
after either secret is spent — a unique-violation on the credential insert, a failed audit write, or
a failed `tx.commit()` — the account is left with a burned claim code, a burned ceremony, and no
passkey: exactly the state `passkeys::clear`'s own doc comment and `CLAUDE.md` single out as needing
a second admin-assisted reset. For an org's last owner, nobody above them can issue one.

- `claim_finish` opens one unpinned transaction and runs the claim consumption, the ceremony
  consumption, the credential insert, and the audit write all on it, committing once — after the
  claimed account and the ceremony's account are confirmed to match. Any failure at any point,
  including the ownership mismatch itself, rolls back all four together: the claim and the ceremony
  are both still there afterward, ready for a retry.
- `of_core::invites` gains `consume_account_claim_tx`, a connection-taking sibling of
  `Db::consume_account_claim` — the same shape `#87` established for `create_account_claim`/
  `create_account_claim_tx`.
- `of_auth::passkeys` gains `finish_registration_tx`, a connection-taking sibling of
  `finish_registration` that does the work without committing, so a caller that must fold another
  single-use secret into the same commit (`claim_finish`) can. `finish_registration`'s existing
  public signature is unchanged — it becomes a two-line wrapper (open a transaction, call
  `finish_registration_tx`, commit) — so `signup_finish` and `add_passkey_finish`, and every test
  that calls it, need no changes.
- As a consequence of the ceremony consumption now running on the same transaction rather than
  autocommitted beforehand, this benefits `signup_finish` and `add_passkey_finish` too: a failure
  inside `finish_registration`'s own transaction now restores the ceremony row instead of leaving it
  permanently burned for nothing. This is a free improvement of the same shape, not a second fix
  requiring its own scoping.
- `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`, and
  `cargo fmt --all --check` all pass. The existing passkey and claim test suites
  (`crates/of-auth/tests/passkeys.rs`, `crates/of-web/tests/console.rs`) continue to pass unchanged.
  Two new tests deterministically prove the fix — see §3.

## Premise corrections

None. The issue's file references match the current tree exactly: `claim_finish`
(`crates/of-web/src/routes/auth.rs:338`) calls `consume_account_claim` before
`passkeys::finish_registration`, and `finish_registration`'s `take_ceremony`
(`crates/of-auth/src/passkeys.rs:681`) deletes the ceremony row on `db.pool()` before the function's
own `db.begin_unpinned()` (line 268) opens.

## Scope

**In:**

- `of_core::invites::consume_account_claim_tx` (new, connection-taking).
- `of_auth::passkeys::finish_registration_tx` (new, connection-taking) and `take_ceremony`
  (existing, private) widened from `db: &Db` to a generic `PgExecutor`, mirroring `Entry::write`'s
  own precedent in `crates/of-core/src/audit.rs`.
- `claim_finish` restructured to open its own transaction and drive claim consumption, ceremony
  consumption, credential insert, and audit write on it, in that order, committing once after the
  ownership check passes.
- **The ownership check now gates the commit, not just the response.** Today, `finish_registration`
  commits the credential and audit row internally, returns, and only then does `claim_finish` check
  `registered != user` and answer `403` — by which point the write is already durable. Under the
  restructuring above, `finish_registration_tx` does not commit; `claim_finish` does, after the
  check. A mismatch now means nothing was ever written, not "something was written and then the
  caller was told no." This is an unavoidable, and strictly safer, consequence of folding the claim
  consumption into the same transaction — not an independent design decision requiring its own
  scoping — but is called out explicitly below because it overlaps with `#109`.
- Two new deterministic tests (§3): one in `of-auth` proving a forced mid-transaction failure now
  restores the ceremony row (not just the credential/audit atomicity `#131` already tests), and one
  in `of-web` proving that a `claim_finish` failure leaves the claim code itself still usable — the
  exact hazard `#132` reports, reproduced and closed.

**Out:**

- **`#109` is not closed by this change**, and is not fully addressed by it. `#109`'s concern is
  broader than this fix: it also covers `add_passkey_finish`, which this change does not touch (it
  still calls the unchanged, self-committing `finish_registration`, and its own post-write ownership
  check is unaffected). What this change does is narrow `#109`'s claim-path half as an unavoidable
  side effect of making `claim_finish` atomic — not a deliberate, scoped fix of `#109` itself. If
  `#109` still needs its own follow-up for `add_passkey_finish` after this ships, that follow-up is
  unchanged in scope.
- **No change to `finish_registration`'s public signature.** `signup_finish`, `add_passkey_finish`,
  and every test that calls `finish_registration` directly (`crates/of-auth/tests/passkeys.rs`,
  `crates/of-web/tests/console.rs`) are untouched.
- **No change to `Db::consume_account_claim`'s or `Db::peek_account_claim`'s external behavior.**
  `consume_account_claim` becomes a thin wrapper over the new `consume_account_claim_tx` (opening
  and committing its own transaction, the same shape `create_account_claim`/`create_account_claim_tx`
  already established) rather than a single autocommitted statement — behaviorally identical on
  every path, since a single `UPDATE ... RETURNING` is already atomic on its own.
- **No new tenant table, no RLS change.** `account_claims`, `webauthn_ceremonies`, and `passkeys` are
  global tables with no `org_id` column — Load-Bearing Invariant 1 does not apply.
- **No change to `login::with_passkey`, session issuance, or the response shape** of
  `POST /api/auth/claim/finish`. The success and 403 responses are byte-identical to today's; only
  what gets durably written before an early return changes.

## Public-interface changes

| Surface | Change | Breaking? |
|---|---|---|
| `of_auth::passkeys::finish_registration` | Unchanged signature and unchanged behavior on every existing call site (`signup_finish`, `add_passkey_finish`, all tests) | No |
| `of_auth::passkeys::finish_registration_tx` (new) | Additive | No |
| `of_core::invites::consume_account_claim_tx` (new) | Additive | No |
| `of_core::invites::Db::consume_account_claim` | Unchanged signature; internal implementation now opens+commits its own transaction instead of one autocommitted statement — no observable behavior change | No |
| Console REST — `POST /api/auth/claim/finish` | Unchanged request/response shape. The one behavior change: a request that previously left a committed (but response-rejected) credential on a ceremony-ownership mismatch now leaves nothing committed. This is a bug fix, not a contract change — no client depends on the old, incorrect side effect | No |
| MCP | none | — |
| OAuth/discovery | none | — |
| Config | none | — |
| Schema | none — no migration | No |

This is not a public-interface change Non-Negotiable Rule 6 requires a version bump or a
`docs/clients/matrix.md` update for. Flagged explicitly for the architect reviewer anyway, per the
job's own instruction, because of the ownership-check-timing change noted above and its overlap with
`#109`.

## §1 `of_core::invites::consume_account_claim_tx`

`crates/of-core/src/invites.rs`, beside the existing `create_account_claim`/`create_account_claim_tx`
pair (same file, same established shape):

```rust
impl Db {
    /// Spend a claim. One statement, so two requests racing the same code
    /// cannot both win.
    pub async fn consume_account_claim(&self, token_hash: &[u8]) -> Result<UserId> {
        let mut tx = self.begin_unpinned().await?;
        let user = consume_account_claim_tx(&mut tx, token_hash).await?;
        tx.commit().await?;
        Ok(user)
    }
}

/// The same claim consumption as [`Db::consume_account_claim`], but run
/// against a connection the caller already holds a transaction on — so a
/// caller that must also finish a credential registration in the same
/// commit (`of_web::routes::auth::claim_finish`) can fold both single-use
/// secrets into one transaction. Before this existed, the claim was spent
/// by an autocommitted statement *before* the credential/audit transaction
/// even opened, so a failure anywhere in that transaction burned the claim
/// for nothing — see `savvagent/otto-factory#132`.
pub async fn consume_account_claim_tx(
    conn: &mut sqlx::PgConnection,
    token_hash: &[u8],
) -> Result<UserId> {
    let user: Option<UserId> = sqlx::query_scalar(
        "UPDATE account_claims SET consumed_at = now() \
         WHERE token_hash = $1 AND consumed_at IS NULL AND expires_at > now() \
         RETURNING user_id",
    )
    .bind(token_hash)
    .fetch_optional(conn)
    .await?;

    user.ok_or(Error::InviteInvalid)
}
```

`conn: &mut sqlx::PgConnection` (a concrete type), not a generic `PgExecutor`, matching
`create_account_claim_tx`'s own signature in this file rather than `Entry::write`'s generic-executor
shape in `of-core::audit` — the difference is deliberate: this function has exactly one call site
shape (a caller already holding a `&mut sqlx::PgConnection`, whether from a pinned `Tx::conn()` or an
unpinned `Transaction`'s `DerefMut`), while `Entry::write` genuinely needs to run on either a pool
reference or a connection reference from different callers. Matching the file's existing idiom for
the existing idiom's reason, not introducing a second one.

## §2 `of_auth::passkeys`: `take_ceremony` widened, `finish_registration_tx` added

`crates/of-auth/src/passkeys.rs`. `take_ceremony` (private, two existing call sites) moves from
`db: &Db` to a generic executor, mirroring `Entry::write`'s precedent from `#131`:

```rust
async fn take_ceremony<'e, T, E>(conn: E, id: Uuid, kind: &str) -> Result<(Option<UserId>, T)>
where
    T: serde::de::DeserializeOwned,
    E: sqlx::PgExecutor<'e>,
{
    let row: Option<(Option<UserId>, serde_json::Value)> = sqlx::query_as(
        "DELETE FROM webauthn_ceremonies \
         WHERE id = $1 AND kind = $2 AND expires_at > now() \
         RETURNING user_id, state",
    )
    .bind(id)
    .bind(kind)
    .fetch_optional(conn)
    .await?;

    let (user, state) = row.ok_or(AuthError::CeremonyExpired)?;
    let state = serde_json::from_value(state)
        .map_err(|e| AuthError::Config(format!("could not read a ceremony: {e}")))?;
    Ok((user, state))
}
```

`finish_authentication` (unaffected otherwise) passes `db.pool()` in place of `db`:

```rust
let (_, state): (Option<UserId>, DiscoverableAuthentication) =
    take_ceremony(db.pool(), ceremony, "authenticate").await?;
```

`finish_registration` splits into a connection-taking half and a two-line committing wrapper:

```rust
/// The connection-taking half of [`finish_registration`], for a caller that
/// must fold another single-use secret's consumption into the same commit —
/// `of_web::routes::auth::claim_finish` runs the claim's own consumption on
/// this same connection, so a failure anywhere in this function restores
/// the claim rather than having already burned it. See
/// `savvagent/otto-factory#132`.
///
/// Does not commit. The caller opens the transaction this runs on and
/// decides when (or whether) to commit it — [`finish_registration`] commits
/// immediately after; `claim_finish` commits only once it has also checked
/// that the ceremony's account matches the claim's.
pub async fn finish_registration_tx(
    tx: &mut sqlx::PgConnection,
    webauthn: &Webauthn,
    ceremony: Uuid,
    credential: &RegisterPublicKeyCredential,
    nickname: Option<&str>,
    via: RegistrationVia,
    ip: Option<&str>,
) -> Result<UserId> {
    let (user_id, state): (Option<UserId>, PasskeyRegistration) =
        take_ceremony(&mut *tx, ceremony, "register").await?;
    let user_id = user_id.ok_or(AuthError::CeremonyExpired)?;

    let passkey = webauthn
        .finish_passkey_registration(credential, &state)
        .map_err(webauthn_failed)?;

    let credential_id = passkey.cred_id().as_ref().to_vec();
    let encoded = serde_json::to_value(&passkey)
        .map_err(|e| AuthError::Config(format!("could not store a passkey: {e}")))?;

    sqlx::query(
        "INSERT INTO passkeys (user_id, credential_id, credential, nickname) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id)
    .bind(&credential_id)
    .bind(&encoded)
    .bind(nickname)
    .execute(&mut *tx)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
            AuthError::CredentialAlreadyRegistered
        }
        _ => AuthError::from(e),
    })?;

    Db::audit_global_on(
        tx,
        Entry::new(action::PASSKEY_REGISTERED)
            .actor(user_id)
            .detail(serde_json::json!({ "via": via.as_str() }))
            .from_request(ip, None),
    )
    .await?;

    Ok(user_id)
}

/// Finish registering, and return the account the key now belongs to.
///
/// Opens its own transaction and commits immediately — for `signup_finish`
/// and `add_passkey_finish`, which have nothing else to fold into the same
/// commit. `claim_finish` uses [`finish_registration_tx`] directly instead.
pub async fn finish_registration(
    db: &Db,
    webauthn: &Webauthn,
    ceremony: Uuid,
    credential: &RegisterPublicKeyCredential,
    nickname: Option<&str>,
    via: RegistrationVia,
    ip: Option<&str>,
) -> Result<UserId> {
    let mut tx = db.begin_unpinned().await?;
    let user_id =
        finish_registration_tx(&mut tx, webauthn, ceremony, credential, nickname, via, ip).await?;
    tx.commit().await?;
    Ok(user_id)
}
```

`&mut *tx` inside `finish_registration_tx` reborrows the `&mut sqlx::PgConnection` parameter, the
same idiom `Db::begin` already uses (`crates/of-core/src/db.rs`) and `#131`'s spec already recorded
for this exact function. The final call (`Db::audit_global_on(tx, ...)`) moves the parameter rather
than reborrowing, since it is the function's last use of it — mirroring `create_account_claim_tx`'s
own last-use convention in `of-core::invites`.

## §3 `claim_finish`

`crates/of-web/src/routes/auth.rs`:

```rust
/// `POST /api/auth/claim/finish` — register the new passkey and sign in.
///
/// The code is spent here rather than at `start`, so an interrupted ceremony
/// does not burn somebody's only way back into their account.
///
/// The claim consumption, the ceremony consumption, the credential insert,
/// and the audit write all share one transaction (`savvagent/otto-factory#132`):
/// before this, the claim was spent by an autocommitted `UPDATE` and the
/// ceremony by `finish_registration`'s own autocommitted `DELETE`, both
/// *before* the credential/audit transaction `#131` added even opened. A
/// failure anywhere after either point — including the credential insert
/// itself, the audit write, `tx.commit()`, or this function's own
/// ceremony-ownership check below — used to leave (or would have left) the
/// account with a burned claim, a burned ceremony, and no passkey: for an
/// org's last owner, nobody above them can issue a second claim. Now any
/// such failure rolls back everything, and the claim and ceremony are both
/// still there for a retry.
pub async fn claim_finish(
    State(state): State<AppState>,
    parts: Parts,
    Json(req): Json<FinishClaim>,
) -> ApiResult<Response> {
    let ip = client_ip(&parts, &state.config);

    let mut tx = state.db.begin_unpinned().await?;
    let user =
        of_core::invites::consume_account_claim_tx(&mut tx, &hash_claim(&req.code)).await?;
    let registered = passkeys::finish_registration_tx(
        &mut tx,
        &state.webauthn,
        req.ceremony_id,
        &req.credential,
        req.nickname.as_deref(),
        passkeys::RegistrationVia::Claim,
        ip.as_deref(),
    )
    .await?;

    // The ceremony was started against the claimed account; if these
    // disagree, something has been substituted and the safe answer is to
    // refuse — before anything commits, so a mismatch rolls back the claim
    // consumption and the credential/audit writes together instead of
    // leaving them durable for a request that gets rejected.
    if registered != user {
        return Err(ApiError::forbidden(
            "that claim code is not for this ceremony",
        ));
    }

    tx.commit().await.map_err(of_core::Error::from)?;

    let opened = login::with_passkey(&state.db, user, ip.as_deref()).await?;
    signed_in_response(&state, opened).await
}
```

`tx.commit().await.map_err(of_core::Error::from)?` — `db.begin_unpinned()` returns a raw
`sqlx::Transaction`, so its `.commit()` is sqlx's own method returning `sqlx::Result<()>`, not
`of_core::Result<()>`. `ApiError` has no `From<sqlx::Error>` (deliberately — see `of-web/src/error.rs`'s
module doc: a database error is never handed to a caller verbatim), only `From<of_core::Error>`.
Mapping through `of_core::Error::from` (`#[from] sqlx::Error` already exists on it) reuses that
existing, already-safe conversion rather than adding a new one.

Every other fallible step in the new body (`begin_unpinned`, `consume_account_claim_tx`,
`finish_registration_tx`, `login::with_passkey`) already returns a type with an existing `?`-
compatible conversion into `ApiResult` — unchanged from today.

## Error Handling & Edge Cases

- **A credential unique-violation, a failed audit write, or a failed `tx.commit()`** — every failure
  mode named in `#132`'s report — now rolls back the claim consumption and the ceremony consumption
  along with the credential/audit writes. The caller sees the same error mapping as today (whatever
  `AuthError`/`CoreError` the failure produces); what changes is that a retry from `claim/start` with
  the same code works, instead of failing with "that claim code is not valid."
- **A ceremony-ownership mismatch** (`registered != user`) now also rolls back the claim and any
  credential/audit write that happened before the check — see Scope, above, on why this is in scope
  as an unavoidable consequence rather than a `#109` fix.
- **`CeremonyExpired`** (the ceremony was never started, already used, or timed out) still surfaces
  as before, but now via a rollback of the claim consumption too — previously the claim was already
  spent by the time `finish_registration` even looked at the ceremony, so a client that raced
  `claim/start`+`claim/finish` against an already-expired ceremony would come away having burned the
  claim code for nothing even though nothing was ever registered. That gap closes as a side effect of
  this fix; it was not separately reported, but it is the same bug pattern (`#132`'s hazard) inside a
  more mundane trigger.
- **`begin_unpinned()`'s own failure** surfaces via the existing `of_core::Error` → `ApiError`
  conversion, unchanged.

## Testing

- `cargo test -p of-auth --test passkeys` — full existing suite, unmodified, must stay green,
  including `a_forced_audit_failure_rolls_back_the_credential` (`#131`'s own atomicity proof, still
  valid unchanged since `finish_registration` degrades to `finish_registration_tx` + commit).
- **New**, `crates/of-auth/tests/passkeys.rs`: `a_forced_audit_failure_also_restores_the_ceremony` —
  the same `BEFORE INSERT` trigger technique as `a_forced_audit_failure_rolls_back_the_credential`,
  additionally asserting `SELECT count(*) FROM webauthn_ceremonies WHERE id = $1` is `1` after the
  forced failure (previously it would have been `0` regardless of outcome, since `take_ceremony` ran
  autocommitted before this function's transaction existed at all). This is what proves §2's part of
  the fix independent of `claim_finish`.
- **New**, `crates/of-web/tests/console.rs`, beside the existing claim tests:
  `a_credential_collision_during_claim_finish_leaves_the_claim_code_usable` — resets a member's
  passkeys, starts a claim ceremony, drives a software authenticator through registration to obtain
  a real `RegisterPublicKeyCredential` *without* submitting it yet, inserts a colliding `passkeys` row
  under a different (already-registered) account using that exact `credential_id` (forcing the
  eventual `INSERT` to hit the unique-violation → `CredentialAlreadyRegistered` path — the literal
  failure mode `#132`'s report names as the pre-`#131` hazard), then submits `claim/finish` and
  asserts it fails. The test then asserts `claim/start` with the *same code* still succeeds — the
  claim was not burned — and completes a full reclaim with a fresh authenticator and a fresh ceremony
  to prove the account is actually recoverable end to end, not merely that the code still "looks"
  valid.
- `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`.

## Risks & Open Questions

- **`#109` is intentionally left half-open** — see Scope/Out. Flagging again here because it is the
  single most likely point of reviewer pushback: "why not just fix `#109` too, you're already
  touching the ordering." Answer: `add_passkey_finish` is untouched by this change and still has
  `#109`'s full hazard; folding it in here would silently widen this PR's scope beyond what `#132`'s
  issue asked for, which `otto-factory-development`'s own Stop & Escalate condition 11 treats as a
  violation, not a bonus. If `#109` needs its own PR after this merges, it needs its own PR — its
  scope is unaffected by this one landing first.
- **`consume_account_claim`'s new wrapper-transaction shape** (open, delegate, commit) adds one
  transaction round-trip over the previous single autocommitted statement — a cost for whichever
  future caller reaches for it, not a cost paid today: `claim_finish` now calls
  `consume_account_claim_tx` directly, and there is no other call site. `consume_account_claim`
  is kept for API symmetry with `peek_account_claim` and `create_account_claim`/
  `create_account_claim_tx`, and because a connection-taking primitive with no autocommit wrapper
  would be inconsistent with every other `_tx` pair in this file. Negligible cost, not worth a
  special case.
- **No migration, no new table, no RLS surface.** Confirmed against `account_claims`,
  `webauthn_ceremonies`, and `passkeys`' actual schema (`crates/of-core/migrations/0010_passkeys.sql`)
  — none carry an `org_id` column.
