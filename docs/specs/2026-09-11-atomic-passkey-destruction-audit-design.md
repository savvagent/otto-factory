# `passkeys::remove` and `passkeys::clear`'s audit writes become atomic

> **Status:** IMPLEMENTED — shipped in `savvagent/otto-factory#170` (merged as
> `83d9763ddf6aaa4afb8982079f7858a90ad30d5c`), closing `savvagent/otto-factory#134`. Two rounds of
> PR review changed the shape from the original draft below: the last-passkey count-then-delete
> race in `remove` (originally scoped out as separately tracked) was reproduced and fixed in the
> same PR, and `reset_member_passkeys`'s second audit write was dropped entirely rather than kept
> and re-scoped, per the corrected §3, Assumptions, and Risks sections below. A second, independent
> `security-auditor` re-review then verified the fix empirically and confirmed no audit coverage
> was lost.
>
> **One follow-up remains open.** Dropping the redundant `org_id = NULL` write (§3) was justified
> because `Tx::audit_trail` filters by `org_id`, so a global row was already invisible to any
> org-scoped reader — but that same fact is exactly what `savvagent/otto-factory#176` ("Global
> (`org_id IS NULL`) audit rows are permanently invisible to any reader") names as an unresolved
> gap for *every* global audit row this codebase writes, not only the one this PR removed. This
> design does not close `#176`; it only stopped adding to the pile the row it deleted would
> otherwise have kept adding to. Originally filed during the review of `#131`
> (`docs/specs/2026-09-10-passkey-registration-atomic-audit-design.md`, which made
> `finish_registration`'s credential INSERT and its `auth.passkey.registered` audit write atomic
> and left the destructive half of the same forensic chain — `remove`/`clear` — untouched, per that
> PR's own Risks & Open Questions). Builds directly on `#131`'s pattern and on
> `savvagent/otto-factory#165` (`docs/specs/2026-09-11-audit-global-on-unpinned-design.md`), which
> made `Db::audit_global_on` take `&mut of_core::Unpinned` instead of a generic `PgExecutor` bound.

## Goal & Success Criteria

`passkeys::remove` and `passkeys::clear` (`crates/of-auth/src/passkeys.rs`) currently write their
destructive change (a `DELETE` from `passkeys`) and their audit row as two independent statements,
with the audit write best-effort — its failure is only logged
(`if let Err(e) = ... { tracing::error!(...) }`). `remove` does not even open a transaction: both
statements run autocommitted on `db.pool()`. A transient error on the audit write leaves a
destroyed credential with no audit row — the same gap `#131` closed for credential *creation*, and
arguably a more attacker-interesting one here, since removing/clearing a passkey is the
persistence step after a session takeover (evicting the legitimate owner, per `#89`) and clearing
is the admin-assisted-recovery path that names who initiated it (`#88`).

A third, related gap lived in `of_web::routes::orgs::reset_member_passkeys`: after its own
already-atomic, org-scoped `tx.audit(MEMBER_PASSKEYS_RESET)` write commits, it wrote a *second*,
best-effort, `org_id = NULL` `auth.passkey.cleared` row on `db.pool()`, attributed to the admin. That
write was loseable, and (per §3 below) it was structurally incapable of being folded into the pinned
transaction that precedes it while staying `org_id = NULL` — RLS's `audit_events_append` policy
(`WITH CHECK (current_org() IS NULL OR org_id = current_org())`, `0008_audit.sql`) rejects a
`NULL`-org insert on a connection pinned to a non-null org whenever RLS applies. The issue's own fix
sketch proposed giving it the real `org_id` and writing it through `Tx::audit` instead — but that
row was redundant with `MEMBER_PASSKEYS_RESET` in the first place, justified only by a claim (traced
to a comment that turned out to be wrong — see §3) that it mirrored a self-service clear which does
not exist in production. §3 below drops the row entirely instead: the simpler fix, and the one that
actually matches what this codebase does today.

A fourth, unrelated-but-adjacent gap was found and fixed during PR review, not by this section's
original plan: `remove`'s last-passkey check (`count(*)` then a separate `DELETE`) is a genuine
check-then-act race — see the corrected Scope/Out entry below for the full account of why it is
fixed here rather than deferred.

- `passkeys::remove` opens `db.begin_unpinned()`, runs the last-passkey check (via `SELECT id ...
  FOR UPDATE`, not an unlocked `count(*)` — see the corrected §1 below), the `DELETE`, and the
  `auth.passkey.removed` audit write on it, and commits once — the same shape `#131` gave
  `finish_registration`.
- `passkeys::clear` keeps its existing `db.begin_unpinned()` / `clear_tx` / `tx.commit()` shape, but
  moves its `auth.passkey.cleared` audit write from a post-commit, best-effort `db.audit_global(...)`
  call to `Db::audit_global_on(&mut tx, ...)` **before** `tx.commit()`.
- `of_web::routes::orgs::reset_member_passkeys` deletes its post-commit, best-effort, `org_id = NULL`
  `auth.passkey.cleared` write outright — no replacement write, org-scoped or otherwise. See the
  corrected §3 below.
- A failure on any of `remove`'s or `clear`'s audit writes now aborts the whole operation and returns
  `Err`, instead of being swallowed into `tracing::error!` while the destructive change still commits.
  This is a deliberate, `#131`-precedented behavior change — see "Public-interface changes" below.
- `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`, and
  `cargo fmt --all --check` all pass. Existing tests exercising the happy path
  (`the_last_passkey_cannot_be_removed`, `clearing_passkeys_leaves_no_way_in`,
  `clearing_writes_the_passkey_cleared_action`, `removing_a_passkey_writes_the_passkey_removed_action`,
  `an_admin_can_reset_a_members_authenticator_but_gains_nothing_by_it`) continue to pass — the last
  one needs a small, deliberate assertion change (§4) reflecting that `reset_member_passkeys` no
  longer writes an `auth.passkey.cleared` row at all, which is the point of the fix.
- New forced-failure tests (mirroring `#131`'s `a_forced_audit_failure_rolls_back_the_credential`)
  prove the rollback under a genuine mid-transaction failure, real Postgres behavior via a `BEFORE
  INSERT` trigger, not a mock: one for `remove`, one for `clear`, one for `reset_member_passkeys`.
  Plus one concurrency test (`tokio::join!` on two concurrent `remove` calls) proving the `FOR UPDATE`
  fix actually closes the last-passkey race.

## Premise corrections

None. The issue's file references (`crates/of-auth/src/passkeys.rs`'s `remove` and `clear`,
`of_web::routes::orgs::reset_member_passkeys`'s `clear_tx` call) match the current tree exactly. One
clarification worth recording explicitly because it shapes §3's design: `reset_member_passkeys`
calls `passkeys::clear_tx` directly, not `passkeys::clear` — it already writes its own org-scoped,
already-atomic `MEMBER_PASSKEYS_RESET` audit row via `tx.audit(...)` inside the same pinned
transaction as the `clear_tx` DELETE. The gap this issue names in that function is specifically the
*second*, separate, best-effort `auth.passkey.cleared` global mirror it writes after commit — not
the `MEMBER_PASSKEYS_RESET` row, which was already atomic before this change.

## Scope

**In:**

- `passkeys::remove` (`crates/of-auth/src/passkeys.rs`): wrap the last-passkey count check, the
  `DELETE`, and the `auth.passkey.removed` audit write in one `begin_unpinned` transaction.
- `passkeys::clear` (same file): move its existing audit write inside its existing transaction.
- `of_web::routes::orgs::reset_member_passkeys` (`crates/of-web/src/routes/orgs.rs`): drop its
  post-commit `auth.passkey.cleared` write entirely, replacing the post-commit `db.audit_global(...)`
  call with nothing — see the corrected Assumptions/Risks entries below for why this is dropped
  rather than moved org-scoped, which is what this section originally proposed.
- The last-passkey count-then-delete race in `remove` (`SELECT count(*)` then a separate `DELETE`,
  unserialized against a concurrent `remove` on the same account): closed by replacing the aggregate
  with `SELECT id FROM passkeys WHERE user_id = $1 FOR UPDATE`, counted in Rust — the same pattern
  `Tx::count_owners_for_update` already uses for the structurally identical last-owner invariant
  (`crates/of-core/src/orgs.rs`). See the corrected Scope/Out and Risks entries below for why this
  ended up in scope after all.
- New forced-failure tests for `remove`, `clear`, and `reset_member_passkeys`, per `#131`'s
  established technique, plus a concurrency test (`tokio::join!` on two concurrent `remove` calls
  against a two-key account) proving the `FOR UPDATE` fix actually closes the race.
- The one existing test (`an_admin_can_reset_a_members_authenticator_but_gains_nothing_by_it`),
  updated for the dropped redundant write: it now asserts no `auth.passkey.cleared` row exists for
  this call site at all, rather than asserting one does (org-scoped or not) — see the corrected §3
  below.

**Out:**

- **No change to `passkeys::remove`'s or `passkeys::clear`'s public signatures.** Same parameters,
  same `Result<()>` / `Result<u64>` return types.
- ~~No fix for the last-passkey count check's TOCTOU race~~ — **corrected during review; the race is
  now fixed, in this PR (moved to Scope/In above).** This section originally proposed leaving it
  alone on the reasoning that moving the count and the `DELETE` onto one transaction's connection
  narrows the window without closing it, and that a follow-up issue had been filed for it per this
  repo's precedent for out-of-scope findings. Both halves of that reasoning were wrong: no such issue
  was ever actually filed (there was no `#132`/`#133`/`#134`-shaped follow-up for this race — the
  highest issue number in the tracker at review time was `#169`, nothing about it), and PR review
  reproduced the race empirically — two concurrent `remove` calls naming different keys on a two-key
  account, both reading `remaining == 2` under `READ COMMITTED`, both committing, leaving zero
  passkeys, a permanent lockout on a product with no email recovery, directly reachable by a hijacked
  session under the same threat model `#89` names. Once reproduced and confirmed live, deferring it
  further behind an issue that did not exist was not defensible, and the fix turned out to be cheap
  specifically because this PR had already put `remove`'s check, delete, and audit write on one
  transaction: swapping `SELECT count(*)` for `SELECT id ... FOR UPDATE` inside that same transaction
  is a few-line change, not a new one. Fixed directly instead of filing anything.
- **No change to `Db::audit_global`, `Db::audit_for_org`, or `Tx::audit`'s existing signatures or
  behavior.** Every other caller keeps today's semantics; this change only changes which primitive
  three specific call sites use.
- **No change to `MEMBER_PASSKEYS_RESET`'s write** — it was already atomic and org-scoped before
  this change (`tx.audit(...)`, inside `reset_member_passkeys`'s existing transaction). §3 only
  touches the *second*, best-effort `auth.passkey.cleared` write in the same function — and, per the
  corrected §3 below, removes it rather than re-scoping it.
- **No new MCP tool, console route, OAuth/discovery endpoint, config key, or migration.** `passkeys`
  and `audit_events` are existing tables; no schema changes.
- **The three constraints in `CLAUDE.md`:** unaffected. This is an internal hardening of an
  existing auth-spine code path, anchored on no repo, adds no workflow opinion, and is not
  client-specific.

## Public-interface changes

| Surface | Change | Breaking? |
|---|---|---|
| `of_auth::passkeys::remove` (crate-public fn, workspace-internal caller only: `of_web::routes::auth::remove_passkey`) | Same signature. On an audit-write failure, now returns `Err` (rolling back the `DELETE`) where it previously returned `Ok(())` with a logged, swallowed audit-write failure and a *committed* delete. Also, per the corrected TOCTOU fix above, the last-passkey check now takes `SELECT ... FOR UPDATE` row locks instead of an unlocked `count(*)`: a second concurrent `remove` on the same account now blocks briefly until the first commits or rolls back, rather than running unserialized against it | Behavioral, not a signature change. The one caller already propagates `remove`'s `Result` via `?`, so no caller code changes; only what triggers the existing error path changes (plus the new, briefly-blocking lock wait on true concurrent removal, which only ever affects two requests racing to remove keys from the *same* account), and the *outward* effect (a passkey either both gets removed and gets audited, or neither, and the account can never be left with zero passkeys) is now consistent with what an admin reviewing the trail would expect |
| `of_auth::passkeys::clear` (crate-public fn, called only from `crates/of-auth/tests/passkeys.rs` today — see Assumptions) | Same signature. On an audit-write failure, now returns `Err` (rolling back the `DELETE`) where it previously returned `Ok(u64)` with a logged, swallowed audit-write failure and a *committed* delete | Behavioral, not a signature change. No production caller exists today; if one is added later it must already handle `Result` |
| `of_web::routes::orgs::reset_member_passkeys` (HTTP handler behind `POST /api/orgs/{org}/members/{user}/reset-passkeys`) | Same request/response shape (`201 { code, link }` on success). Per the corrected §3 below, the post-commit best-effort `auth.passkey.cleared` write this endpoint used to make is removed entirely rather than moved org-scoped — this endpoint no longer writes that action at all. The already-atomic `MEMBER_PASSKEYS_RESET` (`org.member.passkeys_reset`) row is unchanged | Behavioral: the response shape and the `org.member.passkeys_reset` row are unchanged. `GET /api/orgs/{org}/audit?actionPrefix=auth.passkey.cleared` now returns nothing for this call site, same as before this PR (it always returned nothing here — the row was written `org_id = NULL`, invisible to any org-scoped read) — no consumer that was relying on today's actual production behavior is affected |
| `of_core::audit::Db::audit_global_on` | No change — reused as-is (already takes `&mut Unpinned`, per `#165`) | No |
| `of_core::audit::Tx::audit` | No change — reused as-is for `MEMBER_PASSKEYS_RESET`, which already used it before this change | No |
| MCP | none | — |
| Console REST | none — see the `reset_member_passkeys` row above; this endpoint's audit-visible behavior for `auth.passkey.cleared` is unchanged (still invisible to every org, now because the row is never written instead of because it was written `org_id = NULL`) | No |
| OAuth/discovery | none | — |
| Config | none | — |
| Schema | none — `passkeys` and `audit_events` are existing tables, no migration | No |

None of these are the kind of change Non-Negotiable Rule 6 requires a version bump or a
`docs/clients/matrix.md` update for — no tool, route, schema, or config surface changes shape or is
removed/renamed. They are called out at length anyway, per the same rule's spirit and per `#131`'s
own precedent, because the *behavior* changes (a rare-case 500 instead of a silent success; the
post-commit, always-`org_id = NULL` `auth.passkey.cleared` write on `reset_member_passkeys`
disappearing entirely rather than being kept and re-scoped) are exactly the kind of thing an
architect reviewer should see named rather than discover in the diff.

## §1 `passkeys::remove` becomes atomic

`crates/of-auth/src/passkeys.rs`, current shape (lines 540–574 as of this writing):

```rust
pub async fn remove(db: &Db, user: UserId, key: Uuid, ip: Option<&str>) -> Result<()> {
    let remaining = count(db, user).await?;
    if remaining <= 1 {
        return Err(AuthError::LastPasskey);
    }

    let affected = sqlx::query("DELETE FROM passkeys WHERE user_id = $1 AND id = $2")
        .bind(user)
        .bind(key)
        .execute(db.pool())
        .await?
        .rows_affected();

    if affected == 0 {
        return Err(AuthError::UnknownCredential);
    }

    if let Err(e) = db.audit_global(/* ... */).await {
        tracing::error!(/* ... */);
    }

    Ok(())
}
```

New shape, as actually shipped — **corrected during review from what this section originally
proposed** (see the corrected Scope/Out entry above): no `_tx` split, and the last-passkey check
takes row locks instead of an unlocked aggregate:

```rust
pub async fn remove(db: &Db, user: UserId, key: Uuid, ip: Option<&str>) -> Result<()> {
    let mut tx = db.begin_unpinned().await?;

    // `FOR UPDATE` rather than `count(*)`: an aggregate takes no row locks, so
    // two concurrent `remove` calls naming different keys on a two-key
    // account could each read `remaining == 2`, each pass the guard, and both
    // commit — leaving zero passkeys, the exact permanent lockout this check
    // exists to prevent on a product with no email recovery. Locking the rows
    // forces the second transaction to block here until the first commits or
    // rolls back, then re-evaluate against what it left behind — the same
    // pattern `Tx::count_owners_for_update` already uses for the structurally
    // identical last-owner invariant (`crates/of-core/src/orgs.rs`). An
    // aggregate cannot ride `FOR UPDATE`, hence selecting ids and counting in
    // Rust.
    let remaining: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM passkeys WHERE user_id = $1 FOR UPDATE")
            .bind(user)
            .fetch_all(tx.conn())
            .await?;
    if remaining.len() <= 1 {
        return Err(AuthError::LastPasskey);
    }

    let affected = sqlx::query("DELETE FROM passkeys WHERE user_id = $1 AND id = $2")
        .bind(user)
        .bind(key)
        .execute(tx.conn())
        .await?
        .rows_affected();

    if affected == 0 {
        return Err(AuthError::UnknownCredential);
    }

    // The delete and its audit row commit together: a destroyed credential
    // with no audit row is at least as attacker-interesting as a created one
    // with no audit row (#108/#131) — arguably more, since this is the step
    // that evicts a legitimate owner after a session takeover (#89).
    Db::audit_global_on(
        &mut tx,
        Entry::new(action::PASSKEY_REMOVED)
            .actor(user)
            .target("passkey", key.to_string())
            .from_request(ip, None),
    )
    .await?;

    tx.commit().await?;
    Ok(())
}
```

No `remove`/`remove_tx` split (unlike `finish_registration`/`finish_registration_tx`): this section
originally proposed one, mirroring `finish_registration`'s shape on the theory that a future caller
might need to fold another statement into the same commit — but `finish_registration_tx` earns that
split because `claim_finish` genuinely is a second caller today, and nothing analogous exists or is
proposed for `remove`. A speculative single-caller split was simplified away during review; add one
if and when a real second caller shows up.

The public `count(db, user)` helper (used by `has_credential` and `of_web::routes::auth::me`) is
untouched — `remove` runs its own `SELECT ... FOR UPDATE` on the transaction's connection instead of
calling it, since `count` takes `&Db` (pool-bound) and cannot run on a caller-held transaction, and
since an aggregate cannot take row locks in the first place. Duplicating one `SELECT` string is
preferable to changing `count`'s signature for one internal caller.

`LastPasskey`/`UnknownCredential` are returned *before* the transaction is asked to do anything
destructive in the first case, and after a no-op `DELETE` in the second — both cases already leave
nothing to roll back today, and opening the transaction unconditionally (rather than only after the
count check passes) costs one extra round trip on the refused-removal path in exchange for not
special-casing early returns before vs. after `begin_unpinned()`. This mirrors `finish_registration`,
which also opens its transaction before any of its own early-return checks that don't depend on it.

## §2 `passkeys::clear` becomes atomic

`crates/of-auth/src/passkeys.rs`, current shape (lines 627–649):

```rust
pub async fn clear(db: &Db, user: UserId, actor: UserId, ip: Option<&str>) -> Result<u64> {
    let mut tx = db.begin_unpinned().await?;
    let removed = clear_tx(tx.conn(), user).await?;
    tx.commit().await?;

    if let Err(e) = db.audit_global(/* ... */).await {
        tracing::error!(/* ... */);
    }

    Ok(removed)
}
```

New shape — the audit write moves inside the existing transaction, before `commit`:

```rust
pub async fn clear(db: &Db, user: UserId, actor: UserId, ip: Option<&str>) -> Result<u64> {
    let mut tx = db.begin_unpinned().await?;
    let removed = clear_tx(tx.conn(), user).await?;

    // The delete and its audit row commit together, the same reasoning as
    // `remove` above and `finish_registration` (#131): the admin-assisted-
    // reset row is the one that names who initiated a takeover (#88), and it
    // must not be losable independently of the delete it records.
    Db::audit_global_on(
        &mut tx,
        Entry::new(action::PASSKEY_CLEARED)
            .actor(actor)
            .target("user", user.to_string())
            .from_request(ip, None),
    )
    .await?;

    tx.commit().await?;
    Ok(removed)
}
```

`clear_tx`'s own doc comment (crates/of-auth/src/passkeys.rs:651–667) currently explains that it
writes no audit row itself because the caller may be running it on a *pinned* connection
(`reset_member_passkeys`'s case) where a `NULL`-org insert would violate RLS. That constraint is
unchanged by this section — `clear_tx` still writes no audit row, and `clear` (this section) still
only ever calls it on an `Unpinned` connection it opened itself. `clear_tx`'s doc comment is updated
to drop the now-stale claim that the caller writes its record "separately, after commit, as the
best-effort record `audit_global` already documents itself to be" — after this change, `clear`'s
own caller-side write happens *before* commit, not after; `reset_member_passkeys`'s own write is
addressed separately in §3.

## §3 `reset_member_passkeys`'s `auth.passkey.cleared` write is dropped

`crates/of-web/src/routes/orgs.rs`, current shape (lines 366–398):

```rust
let mut tx = state.db.begin(ctx.org.id).await?;
of_auth::passkeys::clear_tx(tx.conn(), target).await?;
of_auth::sessions::revoke_all_tx(tx.conn(), target).await?;
of_core::invites::create_account_claim_tx(tx.conn(), target, &token.hash, Some(ctx.user.id)).await?;
tx.audit(
    Entry::new(action::MEMBER_PASSKEYS_RESET)
        .actor(ctx.user.id)
        .target("user", target.to_string())
        .from_request(ip.as_deref(), None),
)
.await?;
tx.commit().await?;

// Best-effort global record — see `audit_global`'s own doc comment — and
// attributed to the admin who did this, not the member it happened to.
if let Err(e) = state.db.audit_global(
    Entry::new(action::PASSKEY_CLEARED)
        .actor(ctx.user.id)
        .target("user", target.to_string())
        .from_request(ip.as_deref(), None),
).await {
    tracing::error!(/* ... */);
}
```

The post-commit block cannot simply move earlier unchanged: `tx` here is a pinned `Tx` (opened via
`state.db.begin(ctx.org.id)`, `app.org_id` set to `ctx.org.id`), and `audit_events_append`'s RLS
policy (`WITH CHECK (current_org() IS NULL OR org_id = current_org())`, `0008_audit.sql`) rejects an
`org_id = NULL` insert on a connection where `current_org()` is non-null, on any deployment shape
where RLS actually applies (this includes today's default local/test shape, where `of_app` is
assumable and `SET LOCAL ROLE of_app` runs inside `Db::begin`, dropping the connection out of the
superuser/owner RLS exemption). `Db::audit_global_on` enforces the same rule at the type level — it
takes `&mut Unpinned`, which a pinned `Tx` has no accessor to produce (`Tx::conn()` hands out a bare
`&mut PgConnection`). There is no way to keep this row `org_id = NULL` and also make it commit
atomically with the pinned transaction that precedes it.

**Corrected during review.** This section originally proposed resolving the RLS conflict above by
writing the row with the real `org_id`, via `Tx::audit`, inside the same transaction as the other
three statements — keeping the redundant `auth.passkey.cleared` write, just made atomic and
org-scoped instead of best-effort and `NULL`-org. That plan rested on a comment (quoted below, as
originally written) claiming this mirrors what a self-service clear writes globally:

```rust
// Same event auth.passkey.cleared self-service clears write globally
// (of_auth::passkeys::clear), so a customer's SIEM export keyed on this
// action string sees an admin-assisted clear too...
```

That premise is false: `of_auth::passkeys::clear` — the function that would write this action
"self-service" and globally — **has no production caller**. `reset_member_passkeys` itself calls
[`clear_tx`], not [`clear`], specifically so it can write its own org-scoped record instead. There is
no self-service passkey clear anywhere in this product today, so there was no live global write for
this row to mirror, and no SIEM export keyed on `auth.passkey.cleared` could have been seeing
admin-assisted resets under this action name for any reason grounded in what the code actually does.
The redundancy was real, but the justification for keeping it was not.

**Fix actually shipped: the second write is deleted, not re-scoped.** The whole post-commit block —

```rust
// Best-effort global record — see `audit_global`'s own doc comment — and
// attributed to the admin who did this, not the member it happened to.
if let Err(e) = state.db.audit_global(
    Entry::new(action::PASSKEY_CLEARED)
        .actor(ctx.user.id)
        .target("user", target.to_string())
        .from_request(ip.as_deref(), None),
).await {
    tracing::error!(/* ... */);
}
```

— is removed with no replacement. `reset_member_passkeys` now writes exactly one audit row for a
reset, `MEMBER_PASSKEYS_RESET`, unchanged from before this PR (already atomic, already org-scoped).
This closes `#134`'s actual gap at this call site (the destructive change and its audit record are
one atomic unit) with a strictly smaller diff than either this section's original plan or the
issue's own fix sketch: no new `tx.audit` call, no new RLS-conflict explanation to maintain, and one
fewer row for an org admin reading `GET /api/orgs/{org}/audit` to reconcile against the reset they
just performed. See [`clear`]'s doc comment (`crates/of-auth/src/passkeys.rs`) for the corresponding
note that it has no production caller today.

## §4 Test updates

**Updated (corrected during review):** `an_admin_can_reset_a_members_authenticator_but_gains_nothing_by_it`
(`crates/of-web/tests/console.rs:1710`). This section originally proposed re-pointing its existing
`org_id IS NULL` assertion at the acting org's real id, since the original plan kept the row and
scoped it. Per the corrected §3 above, the row is dropped instead, so the test now asserts its
absence rather than its new scoping:

```rust
let cleared: i64 = sqlx::query_scalar(
    "SELECT count(*) FROM audit_events WHERE action = $1 AND target_id = $2",
)
.bind(of_core::audit::action::PASSKEY_CLEARED)
.bind(bob.user.to_string())
.fetch_one(h.db.pool())
.await
.unwrap();
assert_eq!(cleared, 0, /* ... */);
```

The row's content assertions for `MEMBER_PASSKEYS_RESET` (actor = Rob, target = Bob) are unchanged.

**New:** two forced-failure tests plus one concurrency test, mirroring
`a_forced_audit_failure_rolls_back_the_credential` (`crates/of-auth/tests/passkeys.rs`, from `#131`)
for the forced-failure technique:

- `a_forced_audit_failure_rolls_back_a_passkey_removal` (`crates/of-auth/tests/passkeys.rs`):
  trigger on `action = 'auth.passkey.removed'`; call `passkeys::remove` on an account with two
  keys; assert `Err`; assert both passkeys still present (the `DELETE` rolled back).
- `a_forced_audit_failure_rolls_back_a_passkey_clear` (same file): trigger on
  `action = 'auth.passkey.cleared'`; call `passkeys::clear`; assert `Err`; assert the account's
  passkey is still present.
- `a_forced_audit_failure_rolls_back_an_admin_assisted_reset` (`crates/of-web/tests/console.rs`) —
  **corrected during review**: this section originally proposed a trigger on
  `action = 'auth.passkey.cleared'` (the row §3 originally proposed keeping); since that row is
  dropped, the trigger is on `action = 'org.member.passkeys_reset'` instead, the one audit row this
  call site still writes. `POST /api/orgs/{org}/members/{user}/reset-passkeys`; assert the request
  fails (existing 500-class mapping); assert the target's passkey, session, and the absence of any
  claim row and any `org.member.passkeys_reset` row all reflect that nothing committed — proving the
  whole transaction (delete + session revoke + claim insert + audit write) is one atomic unit, not
  independent statements that happen to run in sequence.
- `concurrent_removes_leave_exactly_one_key` (`crates/of-auth/tests/passkeys.rs`, added for the
  corrected TOCTOU fix above): `tokio::join!` on two concurrent `passkeys::remove` calls naming
  different keys on a two-key account; assert exactly one succeeds and exactly one passkey survives.

(This section originally also proposed `an_admin_assisted_reset_is_invisible_to_a_different_org`, a
cross-org test for the org-scoped `PASSKEY_CLEARED` row. Dropped along with the row it tested for —
see the corrected §3.)

## Testing

- `cargo test -p of-auth --test passkeys` — existing suite plus the new forced-failure and
  concurrency tests.
- `cargo test -p of-web --test console` — existing suite (with the one updated assertion) plus the
  new forced-failure test.
- `cargo test --workspace` — full regression check; nothing outside these two files' call sites
  changes.
- `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`.

## Error Handling & Edge Cases

- **A failed audit write now fails the whole operation**, for all three call sites. This is the
  deliberate behavior change named throughout this spec, precedented by `#131`. Each call site's one
  caller already propagates the `Result` via `?` (`remove_passkey` → `passkeys::remove`;
  `reset_member_passkeys` → its own `?`-chained statements), so this surfaces as the existing
  500-class error mapping — no new HTTP status, no new response shape, a correctly-returned failure
  instead of a silently-swallowed one.
- **`db.begin_unpinned()`'s own failure** (pool exhaustion, etc.) surfaces via the existing
  `of_core::Error` → `AuthError::Core` `#[from]` conversion already present in
  `crates/of-auth/src/error.rs` — no new error variant needed, same as `#131`.
- **`LastPasskey`/`UnknownCredential` are unaffected.** Both are returned before the audit write is
  attempted (the count check short-circuits before any destructive statement; the zero-`rows_affected`
  check short-circuits before the audit write), so a refused removal still writes no audit row at
  all — unchanged from today.
- **`tx.commit()`'s own failure** is a normal `sqlx::Error`, converted via the existing
  `#[from] sqlx::Error` arm on `AuthError` (for `remove`/`clear`) or the equivalent conversion already
  in `of-web`'s `ApiError` (for `reset_member_passkeys`, which already handles `tx.commit()`
  failures today for its existing three-statement transaction).

## Assumptions

- **`passkeys::clear` has no production caller today.** `reset_member_passkeys` calls `clear_tx`
  directly, not `clear` — confirmed by searching the workspace for `passkeys::clear(` outside
  `crates/of-auth/tests/passkeys.rs`. `clear` is still fixed here because it is public
  crate-workspace API, is exercised directly by three existing tests, and is exactly the function
  the issue names — leaving it best-effort because nothing currently calls it in production would be
  fixing the letter of the issue while missing its point (the next caller inherits the bug otherwise).
- **`Db::begin`'s `SET LOCAL ROLE of_app` runs in this repository's own dev/CI Postgres** (confirmed
  by `crates/of-core/src/isolation.rs`'s startup check passing in CI today), which is why §3 asserts
  the `NULL`-org insert genuinely fails under RLS on a pinned connection in the shape this repo tests
  against, not only in a hypothetical managed-Postgres deployment.

**Superseded, kept for the record:** this section originally carried an assumption that the
`PASSKEY_CLEARED` row `reset_member_passkeys` writes should be kept, not dropped, on the strength of
a comment claiming it mirrored what a self-service clear writes globally. That comment was wrong —
`passkeys::clear` has no production caller, so there was no live self-service write for the row to
mirror — and PR review converged on dropping the row instead (see the corrected §3 and Assumptions
above). The assumption's own closing sentence anticipated this outcome: "if this assumption is
wrong, dropping the redundant write entirely ... is a strictly smaller diff than this spec proposes."

## Risks & Open Questions

Both risks originally flagged here were resolved during PR review rather than left open — see the
corrected Scope/Out, §3, and Assumptions sections above for the full account:

- ~~The last-passkey count-then-delete race (`remove`) is not fixed here~~ — fixed in this PR, once
  review reproduced it empirically and found the follow-up issue this section claimed had been filed
  did not exist.
- ~~Two audit rows per admin-assisted reset ... should be confirmed with the architect reviewer~~ —
  confirmed, and resolved by dropping the redundant `PASSKEY_CLEARED` write rather than keeping it.

**One follow-up does remain open, and it is a direct residue of this design's own §3 decision,**
not a pre-existing, unrelated gap: `savvagent/otto-factory#176` (global, `org_id IS NULL` audit
rows are permanently invisible to any reader). Dropping the redundant `auth.passkey.cleared` write
in §3 was justified on the grounds that a `NULL`-org row is already unreadable by any org-scoped
query (`Tx::audit_trail` filters by `org_id`) — which is exactly the gap `#176` names as unresolved
for the rest of this codebase's global audit rows. This design does not fix `#176`; it only
declines to add one more row to the pile it describes.
