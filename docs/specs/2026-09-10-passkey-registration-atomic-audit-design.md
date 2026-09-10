# `finish_registration`'s credential insert and audit write become atomic

> **Status:** APPROVED — closes `savvagent/otto-factory#108`, filed during the review of `#107`
> (`docs/specs/2026-09-10-passkey-registration-audit-via-design.md`, which added `via`/`ip` to the
> same function without changing its transactional shape). Related: `#109` tracks a different
> ordering concern in the same function (audit write vs. the ceremony-ownership check), left
> untouched here.

## Goal & Success Criteria

`finish_registration` (`crates/of-auth/src/passkeys.rs`) writes the new `passkeys` row and its
`auth.passkey.registered` audit row in one transaction, so the two either both commit or both roll
back. Today they are two independent, autocommitted statements: the credential `INSERT` runs on
`db.pool()`, and the audit write is best-effort — its failure is only logged
(`if let Err(e) = ... tracing::error!(...)`). A transient error between the two leaves a live
credential on the account with no audit row. That gap matters most on the `via = "claim"` path,
which is the one event `#88` exists to make attributable: it is what proves who actually completed
an admin-assisted account takeover.

- `finish_registration` opens one unpinned transaction, runs the credential `INSERT` and the audit
  `INSERT` on it, and commits once, after both succeed.
- A failure on either write now aborts the whole ceremony and returns `Err` to the caller, instead
  of the audit failure being swallowed. This is a deliberate behavior change from `#107`'s spec,
  which declared the audit write's best-effort-ness out of scope — `#108` is exactly the follow-up
  that reconsiders it.
- `of-core::audit` gains a transaction-capable global-audit write (`Db::audit_global_on`), used only
  by this call site; `Db::audit_global` and `Db::audit_for_org` keep their existing pool-based,
  best-effort behavior for every other caller.
- `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`, and
  `cargo fmt --all --check` all pass. The existing registration tests
  (`registration_writes_the_passkey_registered_action`, `registration_records_which_flow_wrote_it`,
  and every test that calls `finish_registration` on the happy path) continue to pass unchanged,
  since committing both writes together produces the same end state the two independent
  autocommitted writes produced on success.

## Premise corrections

None. The issue's file reference (`crates/of-auth/src/passkeys.rs`'s `finish_registration`) matches
the current tree exactly (lines 243–301 as of this writing, already carrying the `via`/`ip`
parameters `#107` added — the issue predates that change but the structure it describes, an
autocommitted `INSERT` followed by a best-effort audit write, is unchanged by it).

## Scope

**In:**

- `finish_registration` opens `db.begin_unpinned()` and runs both writes on it, committing once.
- `crates/of-core/src/audit.rs` gains `Db::audit_global_on`, a transaction-capable sibling of
  `Db::audit_global` for exactly this "commit atomically with another change" case.
- The credential-insert error mapping (unique-violation → `AuthError::CredentialAlreadyRegistered`)
  is preserved unchanged, just re-targeted at the transaction's connection instead of the pool.
- Existing tests updated only as needed to keep compiling; no new test is required beyond what's
  noted under Testing below.

**Out:**

- **No change to `#109`** (the ordering of the audit write relative to `claim_finish`'s /
  `add_passkey_finish`'s post-registration ownership check) — separate, already-filed issue.
- **No change to `Db::audit_global`'s or `Db::audit_for_org`'s existing signature or behavior.**
  Every other caller of either (`login.rs`'s failed-login audit, org invitation flows, etc.) keeps
  today's pool-based, best-effort semantics; this change adds a new method beside them rather than
  changing what they do.
- ~~**No fault-injection test proving the rollback under a genuine mid-transaction failure.**~~
  **Superseded during PR review** (see Risks & Open Questions below) — this claim was wrong. A
  `BEFORE INSERT` trigger on `audit_events` forces the second statement to fail deterministically,
  with no mock, and `a_forced_audit_failure_rolls_back_the_credential`
  (`crates/of-auth/tests/passkeys.rs`) does exactly that. The original reasoning here (no
  fault-injection scaffolding exists elsewhere in the suite) was true and irrelevant — a trigger is
  real Postgres behavior, not scaffolding.
- **No change to `finish_registration`'s public parameter list.** `via`/`ip`/`nickname` etc. are
  unchanged; only the internal write strategy changes.

## Public-interface changes

| Surface | Change | Breaking? |
|---|---|---|
| `of_auth::passkeys::finish_registration` (crate-public fn, used only within the workspace) | Same signature; on failure of either write, now returns `Err` where it previously returned `Ok` with a logged, swallowed audit-write failure | Behavioral, not a signature change — see Error Handling below. Every in-workspace caller (`signup_finish`, `add_passkey_finish`, `claim_finish`) already propagates `finish_registration`'s `Result` via `?` today, so no caller code changes; only what triggers the existing error path changes |
| `of_core::audit::Db::audit_global_on` (new, crate-public within the workspace) | Additive — a new function, no existing caller touched | No |
| Audit trail / SIEM export | None. Rows written on a successful registration are byte-identical to today's | No |
| MCP | none | — |
| Console REST | none — `POST /api/auth/signup/finish`, `POST /api/me/passkeys/finish`, `POST /api/auth/claim/finish` keep their existing request/response shapes. A caller that previously received `200` when the audit write silently failed now receives the endpoint's existing 500-mapping for `AuthError::Db`/`AuthError::Core` in that (rare, transient) case instead — a strictly more honest response, not a new response shape | No |
| OAuth/discovery | none | — |
| Config | none | — |
| Schema | none — `passkeys` and `audit_events` are existing tables, no migration | No |

This is not the kind of public-interface change Non-Negotiable Rule 6 requires a version bump or a
`docs/clients/matrix.md` update for: no tool, route, schema, or config surface changes shape. It is
called out at length anyway because the *behavior* change (a rare-case 500 instead of a silent
success) is exactly the kind of thing an architect reviewer should see named rather than discover in
the diff.

## §1 `Db::audit_global_on` — a transaction-capable sibling of `Db::audit_global`

`crates/of-core/src/audit.rs` currently duplicates the same nine-column `INSERT_SQL` binding three
times: `Tx::audit` (org-scoped, on the transaction's own connection), `Db::audit_global` (no org,
autocommit on the pool), `Db::audit_for_org` (org given directly, autocommit on the pool — used by
the control plane before a tenant `Tx` exists). None of the three can run on an arbitrary caller-
supplied connection, which is exactly what `finish_registration` now needs: the same connection its
credential `INSERT` is already using.

Factor the binding logic into one private helper on `Entry`, generic over the executor, and add one
new public entry point that takes an explicit connection:

```rust
impl Entry {
    /// Bind and run this event's INSERT against any executor — the pool, for a
    /// best-effort global or org write, or an open transaction, for a write
    /// that must commit or roll back with something else.
    async fn write<'e, E>(self, org: Option<OrgId>, conn: E) -> Result<()>
    where
        E: sqlx::PgExecutor<'e>,
    {
        sqlx::query(INSERT_SQL)
            .bind(org)
            .bind(self.actor_user_id)
            .bind(self.actor_label)
            .bind(self.action)
            .bind(self.target_type)
            .bind(self.target_id)
            .bind(self.ip)
            .bind(self.user_agent)
            .bind(self.detail.unwrap_or_else(|| serde_json::json!({})))
            .execute(conn)
            .await?;
        Ok(())
    }
}

impl Tx<'_> {
    pub async fn audit(&mut self, e: Entry) -> Result<()> {
        let org = self.org();
        e.write(Some(org), self.conn()).await
    }
    // audit_trail unchanged
}

impl Db {
    pub async fn audit_global(&self, e: Entry) -> Result<()> {
        e.write(None, &self.pool).await
    }

    pub async fn audit_for_org(&self, org: OrgId, e: Entry) -> Result<()> {
        e.write(Some(org), &self.pool).await
    }

    /// Record a global (no-org) event on a connection the caller already
    /// holds open — typically a transaction that also carries the change the
    /// event describes, so both commit or roll back together.
    ///
    /// Unlike [`Self::audit_global`], a failure here is **not** swallowed: it
    /// propagates to the caller, who is expected to let it abort the
    /// transaction. Use this only when a lost audit row would be worse than
    /// failing the whole operation — [`crate`]'s own `Db::audit_global` doc
    /// comment explains why the *pool* variant is deliberately best-effort
    /// for the ordinary login/enrollment path; this is the exception for a
    /// caller that decided the tradeoff the other way.
    pub async fn audit_global_on<'e, E>(conn: E, e: Entry) -> Result<()>
    where
        E: sqlx::PgExecutor<'e>,
    {
        e.write(None, conn).await
    }
}
```

`sqlx::PgExecutor<'e>` (re-exported from `sqlx-postgres`, `sqlx` 0.8.6 — confirmed present in this
workspace's locked version) is implemented for `&PgPool` and `&mut PgConnection`. It is *not*
implemented directly for `&mut Transaction<'_, Postgres>` — sqlx-core 0.8.6 comments that blanket
impl out ("fails to compile due to lack of lazy normalization") — so a caller reaches it through
`Transaction`'s `DerefMut` instead, exactly as `&mut *tx` does in §2 below and already does in this
crate's own `Db::begin`/`Db::verify_tenant_isolation`. `Tx::audit`, `Db::audit_global`,
`Db::audit_for_org`, and the new `Db::audit_global_on` all resolve to the same one INSERT site
instead of four independent copies of it. This is a refactor of existing private machinery; no
other crate calls `Entry::write` directly, and the three existing public methods keep their exact
signatures.

## §2 `finish_registration`

`crates/of-auth/src/passkeys.rs`:

```rust
pub async fn finish_registration(
    db: &Db,
    webauthn: &Webauthn,
    ceremony: Uuid,
    credential: &RegisterPublicKeyCredential,
    nickname: Option<&str>,
    via: RegistrationVia,
    ip: Option<&str>,
) -> Result<UserId> {
    let (user_id, state): (Option<UserId>, PasskeyRegistration) =
        take_ceremony(db, ceremony, "register").await?;
    let user_id = user_id.ok_or(AuthError::CeremonyExpired)?;

    let passkey = webauthn
        .finish_passkey_registration(credential, &state)
        .map_err(webauthn_failed)?;

    let credential_id = passkey.cred_id().as_ref().to_vec();
    let encoded = serde_json::to_value(&passkey)
        .map_err(|e| AuthError::Config(format!("could not store a passkey: {e}")))?;

    // The credential and its audit row commit together: a live credential
    // with no audit row is exactly the gap #108 exists to close, most of all
    // on the `claim` path, which is the one event that proves who actually
    // completed an admin-assisted takeover (#88).
    let mut tx = db.begin_unpinned().await?;

    // The unique index on credential_id is the real guard: an authenticator
    // must not be registrable twice, to two accounts, which is what the
    // exclude-credentials list asks for politely and this enforces.
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
        &mut *tx,
        Entry::new(action::PASSKEY_REGISTERED)
            .actor(user_id)
            .detail(serde_json::json!({ "via": via.as_str() }))
            .from_request(ip, None),
    )
    .await?;

    tx.commit().await?;

    Ok(user_id)
}
```

`&mut *tx` mirrors the exact pattern `Db::begin` already uses in this crate's sibling `of-core`
(`crates/of-core/src/db.rs`'s `SET LOCAL ROLE` statement runs the same way against an unpinned
`Transaction<'static, Postgres>`), so this introduces no new idiom to the workspace.

`db.begin_unpinned()` (`of-core::Db::begin_unpinned`) is the right primitive: `passkeys` and
`audit_events` with a `NULL` org are both reachable from the control plane before any tenant `Tx`
exists — the same reason `take_ceremony` above it already reads `webauthn_ceremonies` off
`db.pool()` directly rather than through a pinned transaction. Neither table this function touches
is tenant-scoped by `org_id` in the sense Load-Bearing Invariant 1 means (`passkeys` has no
`org_id` column at all; the `audit_events` row here is written with `org_id = NULL`, which is the
documented shape for the unpinned control plane per `0008_audit.sql`'s own comment on
`audit_events_tenant_isolation`). This is not a case Load-Bearing Invariant 1's "new tenant table
needs a cross-org negative test" rule applies to — no tenant table is added or touched.

## §3 Test updates

No test needs new arguments — `finish_registration`'s signature is unchanged. The existing suite in
`crates/of-auth/tests/passkeys.rs` exercises the change without modification:

- `registration_writes_the_passkey_registered_action` and `registration_records_which_flow_wrote_it`
  already assert both the `passkeys` row and the `audit_events` row exist after a successful
  registration — under the rewritten implementation those two assertions are now proof that the
  *same* transaction produced both rows, not two independent statements that happened to both
  succeed.
- Every other test that calls `finish_registration` (`register_new`, the two-devices test, the
  credential-naming tests) continues to exercise the happy path unchanged.

No test is added or removed. See Scope/Out above for why a synthetic partial-failure test is not in
scope.

## Testing

- `cargo test -p of-auth --test passkeys` — full existing suite, unmodified, must stay green.
- `cargo test --workspace` — `crates/of-core/tests/` has no dedicated `audit.rs` suite; audit
  writes are exercised incidentally through the tests that already call `Tx::audit` /
  `Db::audit_global` / `Db::audit_for_org` (e.g. membership and login flows), so the full workspace
  run is what confirms the `Entry::write` refactor changed no existing caller's behavior. Nothing
  outside `of-auth::passkeys::finish_registration` calls the new `Db::audit_global_on`.
- `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`.

## Error Handling & Edge Cases

- **A failed audit write now fails the whole ceremony.** This is the deliberate behavior change
  named above: the caller (`signup_finish`, `add_passkey_finish`, `claim_finish`) already propagates
  `finish_registration`'s `Result` via `?`, so this surfaces as the existing 500-class error mapping
  for `AuthError::Db`/`AuthError::Core` — no new HTTP status code, no new response shape, just a
  correctly-returned failure instead of a silently-swallowed one. A caller retries the whole
  ceremony from `start_registration`, the same recovery path any other mid-registration failure
  already requires (e.g. `webauthn_failed`).
- **`db.begin_unpinned()`'s own failure** (e.g. pool exhaustion) surfaces via the existing
  `of_core::Error` → `AuthError::Core` `#[from]` conversion already present in
  `crates/of-auth/src/error.rs` — no new error variant needed.
- **The unique-violation mapping for a re-registered credential is unaffected.** It still short-
  circuits before the audit write is attempted (the `?` after `.map_err(...)` returns early), so a
  `CredentialAlreadyRegistered` rejection still writes no audit row at all — unchanged from today,
  and out of scope for `#108` (that failure never reached the audit write in the first place, on
  either the old or new code).
- **`tx.commit()`'s own failure** is a normal `sqlx::Error`, converted via the existing
  `#[from] sqlx::Error` arm on `AuthError`.

## Risks & Open Questions

Three findings from PR #131's mandatory review trio (architect-reviewer and security-auditor,
independently convergent), triaged rather than silently absorbed or dismissed:

- **`claim_finish`'s claim code and ceremony are consumed outside this transaction.**
  `consume_account_claim` (autocommitted) and `take_ceremony`'s ceremony `DELETE` (on `db.pool()`,
  before this function's transaction opens) both run before the now-atomic credential+audit write.
  A failure inside that write — newly possible for an audit-write failure, previously only possible
  for the credential `INSERT` itself — can leave a member with no passkey and no outstanding claim.
  This hazard predates this PR (a raw `INSERT` failure already hit it); this PR widens its
  probability surface without introducing a new bug class. Fixing it fully means giving
  `consume_account_claim` a connection-taking form and running it on the same transaction, a
  signature change to `finish_registration` (three callers) this spec did not scope and which
  overlaps with `#109`'s already-tracked ordering concern on the same function. Filed as
  `savvagent/otto-factory#132` rather than folded in here.
- **`Db::audit_global_on` compiles against a pinned `Tx` connection.** Doc-warned in this PR
  (`crates/of-core/src/audit.rs`) and covered by a new regression test
  (`audit_global_on_refuses_a_pinned_connection` in `crates/of-core/tests/isolation.rs`, proving the
  misuse is rejected under RLS enforcement rather than merely documented). The stronger fix —
  making the misuse unrepresentable at the type level, e.g. a newtype around `begin_unpinned`'s
  `Transaction` — is filed as `savvagent/otto-factory#133`; the blast radius is small today (one
  caller) and grows with every future one, so it is worth doing, just not urgently.
- **`passkeys::remove`/`clear`'s own audit writes stay best-effort**, unaffected by this PR's fix —
  `#108`'s scope was `finish_registration` only. A destroyed credential with no audit row is at
  least as attacker-interesting as a created one with no audit row. Filed as
  `savvagent/otto-factory#134`.

This PR's own fix is verified, not merely argued: `a_forced_audit_failure_rolls_back_the_credential`
(`crates/of-auth/tests/passkeys.rs`) installs a `BEFORE INSERT` trigger that forces the audit write
to fail and asserts the credential insert rolls back with it — real Postgres behavior, not a mock,
superseding this spec's original claim (Scope/Out, above) that no deterministic fault-injection test
was possible in this codebase. That claim was wrong; the security-auditor's review supplied the
technique.
