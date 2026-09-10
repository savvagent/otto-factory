# Passkey audit events design

> **Status:** IMPLEMENTED, with one open security follow-up — give passkey registration and
> clearing their own `auth.passkey.*` audit action names instead of the TOTP names they used to
> borrow, stop discarding the two audit writes on those paths, and log (without changing behavior)
> the two places a stored credential that fails to deserialize is silently dropped. Shipped in
> `savvagent/otto-factory#86`, closing `savvagent/otto-factory#76`, deployed and verified via the
> merge commit's own CI run (`cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`,
> `cargo fmt --all --check` all green) and a subsequent successful `flyctl deploy` to
> `otto-factory-mcp`. Three findings from the mandatory review trio were addressed in the same PR
> (a false atomicity claim in a doc comment, a missing IP on the admin-reset's audit row, an
> incomplete historical-constant doc comment).
>
> **`savvagent/otto-factory#87` is the one to track before treating this area as closed**: the
> admin-assisted passkey reset is not atomic (a partial failure can leave an account with no
> passkeys and no outstanding claim code — the exact takeover window the claim-code coupling exists
> to close), and its global audit row may not name the admin who performed it. Two lower-severity
> gaps were also filed rather than folded in — `savvagent/otto-factory#88` (registration's audit
> row can't distinguish signup/claim/add-key and carries no IP), `savvagent/otto-factory#89`
> (`passkeys::remove`/`rename` write no audit row; `login::logout` still discards its write
> silently).

## Goal & Success Criteria

An auditor filtering the trail for passkey registrations finds the events; one filtering for TOTP
finds nothing, because TOTP does not exist in this product. A failed audit write on either path
leaves a line in the logs instead of vanishing. A stored credential that cannot be decoded — the
realistic cause is a `webauthn-rs` upgrade changing the serialized shape — leaves a line naming the
user it happened for, on both paths that can hit it, without changing what either path returns.

- `crates/of-core/src/audit.rs` defines `auth.passkey.registered` and `auth.passkey.cleared`.
  `finish_registration` writes the former, `clear` writes the latter. Neither writes a
  `auth.totp.*` action again.
- `TOTP_ENROLLED` / `TOTP_RESET` stay defined, re-documented as historical: rows created before
  this change used them for these same two passkey events, and nothing here rewrites those rows.
- A failed write on either path reaches `tracing::error!` with the actor and the action, matching
  the existing `note_failure` pattern in the same file.
- `finish_authentication`'s `filter_map` and `update_stored_credential`'s early return each log at
  `error` level, with the user id, when a stored credential fails to deserialize. Neither function's
  return value or error type changes for any input it was already given.
- `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --all
  --check` all pass.

## Premise corrections

The issue's line numbers have drifted since it was filed (PR #77 landed on top of it), but every
claim still holds against the current tree:

- `crates/of-core/src/audit.rs:28-29` — `TOTP_ENROLLED` / `TOTP_RESET`, unchanged content, same
  lines.
- `crates/of-auth/src/passkeys.rs:255-257` — `finish_registration`'s discarded
  `db.audit_global(Entry::new(action::TOTP_ENROLLED)...)`.
- `crates/of-auth/src/passkeys.rs:497-503` — `clear`'s discarded
  `db.audit_global(Entry::new(action::TOTP_RESET)...)`.
- `crates/of-auth/src/passkeys.rs:354` — `finish_authentication`'s
  `.filter_map(|raw| serde_json::from_value::<Passkey>(raw).ok())`.
- `crates/of-auth/src/passkeys.rs:593-596` — `update_stored_credential`'s
  `let Ok(mut passkey) = serde_json::from_value::<Passkey>(raw) else { return Ok(()) };`.

`docs/plans/2026-09-04-doc-drift-no-email.md` Task 2 already flagged exactly this rename as
deliberately deferred, "the file's own definition of a breaking change", and this spec is that
deferred decision.

**One correction found during planning, not in the issue: a third call site writes
`TOTP_RESET`.** `crates/of-web/src/routes/orgs.rs:372-376`, inside `reset_member_passkeys` (the
admin-assisted recovery endpoint `CLAUDE.md` describes), issues its own **org-scoped** write after
calling `of_auth::passkeys::clear`:

```rust
tx.audit(
    Entry::new(action::TOTP_RESET)
        .actor(ctx.user.id)
        .target("user", target.to_string()),
)
```

This is not the same event as `clear`'s own write: `clear`'s is global (`Db::audit_global`, no org,
actor = the affected account) and records "this account's passkeys were cleared"; this one is
org-scoped (`Tx::audit`, guard-1-compliant — it runs inside the same pinned transaction as the claim
code that follows) and records "an admin reset a member's passkeys", with the admin as actor and the
member as `target`. Both fire on the same request. Without fixing this one too, the issue's own
success criterion — "one filtering for TOTP finds nothing" — is false for every org's own audit
trail (`audit_trail`, the console's security page and any customer's SIEM export scoped to their
org), which is arguably the more visible of the two failure modes since it is customer-facing rather
than global-only. In scope now; see §1a.

## Scope

**In:**

- Three new action constants in `of-core::audit::action` (`PASSKEY_REGISTERED`, `PASSKEY_CLEARED`,
  `MEMBER_PASSKEYS_RESET`), and the three call sites (two in `of-auth`, one in `of-web`) that switch
  to them.
- Doc-comment updates on the two TOTP constants they replace, marking them historical-only.
- Logging the two discarded audit-write results at `error` level, matching `note_failure`.
- Logging the two silent credential-deserialization drops at `error` level, with no behavior
  change.
- Tests: the new action constants are the ones written by registration, self-clearing, and
  admin-assisted reset; the two deserialization-drop paths keep their current success/failure
  behavior when a stored credential is corrupt.

**Out:**

- **No rewrite of existing rows.** `audit_events` has an `ENABLE ROW LEVEL SECURITY` +
  `FORCE ROW LEVEL SECURITY` table with `SELECT`/`INSERT` policies only (`0008_audit.sql:52-58,78`)
  — there is no `UPDATE` policy, so an `UPDATE` against it is refused for every role, including the
  owner, on every deployment shape. Backfilling old `auth.totp.enrolled` / `auth.totp.reset` rows to
  the new names is not merely undesirable here, it is not possible without a schema change this
  issue does not ask for. See §1 for the discontinuity this settles on instead.
- **No dual-read.** `Tx::audit_trail`'s `action_prefix` match could be taught to treat
  `auth.passkey.%` and the old constants as one class, but that is permanent query complexity paid
  forever for a one-time cutover. A comment on the historical constants (§1) is cheaper and does not
  degrade over time.
- **No change to `finish_registration`'s or `clear`'s return type or error behavior.** Both audit
  writes stay best-effort by design — `Db::audit_global`'s own doc comment says a failed login-side
  audit write must not become an authentication outage, and the same reasoning holds for
  registration and clearing. This spec adds visibility, not a new failure mode.
- **No change to what `finish_authentication` or `update_stored_credential` return.** A credential
  that fails to deserialize is still dropped from consideration; `keys.is_empty()` still produces
  `InvalidCredentials`; `update_stored_credential`'s decode failure still returns `Ok(())`. Only a
  log line is added on the branch that already discards.
- **No new audit constants for `MAGIC_LINK_SENT`, `MAGIC_LINK_CONSUMED`, `RECOVERY_CODE_USED`, or
  `EMAIL_VERIFIED`.** `docs/plans/2026-09-04-doc-drift-no-email.md` Task 2 already flagged these as
  apparently dead; touching them is a separate decision this issue does not raise.
- **No new `of-auth`/`of-web` dependency on a tracing-capture test crate.** The existing
  `note_failure` / `webauthn_failed` log lines in this same file are not asserted by a test either;
  this change follows the same convention rather than introducing test infrastructure to check a
  log line's exact text.

## Public-interface changes

Per Non-Negotiable Rule 6, named explicitly because the audit action taxonomy is called out in its
own module doc as breaking to rename: *"Actions are dotted and stable because they are queried by
prefix and because they end up in customers' SIEM exports. Renaming one is a breaking change."*

| Surface | Change | Breaking? |
|---|---|---|
| Audit trail / SIEM export | New writes for passkey registration and self-clearing use `auth.passkey.registered` / `auth.passkey.cleared` instead of `auth.totp.enrolled` / `auth.totp.reset` | **Yes, for any saved query filtering on the old action strings going forward** — see §1 for the mitigation (both old constants stay defined and documented) and the discontinuity this accepts |
| Audit trail / SIEM export (org-scoped) | `reset_member_passkeys`'s org-scoped write uses a new `org.member.passkeys_reset` instead of `auth.totp.reset` | **Yes, same discontinuity, org-scoped this time** — see §1a |
| MCP | none | — |
| Console REST | none | — |
| OAuth/discovery | none | — |
| Config | none | — |
| Schema | none — no migration; only which literal string a `Tx`-free `INSERT` binds changes | No |

This does not touch `docs/clients/matrix.md`: nothing an MCP coding-agent client sends or receives
moves. The audience for this change is a human or a SIEM reading `audit_trail` / raw
`audit_events` rows, not a coding agent.

## §1 New action constants, and what happens to the old ones

`crates/of-core/src/audit.rs`, in `pub mod action`:

```rust
pub const TOTP_ENROLLED: &str = "auth.totp.enrolled";
pub const TOTP_RESET: &str = "auth.totp.reset";
```

gain doc comments marking them historical:

```rust
/// Historical only. TOTP was removed from this product; rows with this action
/// predate `auth.passkey.registered` (see `PASSKEY_REGISTERED`) and are not
/// rewritten. Nothing writes this constant anymore.
pub const TOTP_ENROLLED: &str = "auth.totp.enrolled";
/// Historical only, for the same reason as `TOTP_ENROLLED`. Superseded by
/// `PASSKEY_CLEARED`.
pub const TOTP_RESET: &str = "auth.totp.reset";
```

and two new constants are added next to them:

```rust
pub const PASSKEY_REGISTERED: &str = "auth.passkey.registered";
pub const PASSKEY_CLEARED: &str = "auth.passkey.cleared";
```

**Why keep the old constants at all**, rather than delete them now that nothing writes them: a
SIEM export or a saved console query built against `auth.totp.enrolled` still needs to resolve to
something when a developer greps this file for what that string meant. Deleting the constant loses
the one place that answer lives. The doc comment is the documented discontinuity Scope/Out settles
for instead of a backfill or a dual-read.

`crates/of-auth/src/passkeys.rs`:

- `finish_registration`'s write changes from `Entry::new(action::TOTP_ENROLLED)` to
  `Entry::new(action::PASSKEY_REGISTERED)`.
- `clear`'s write changes from `Entry::new(action::TOTP_RESET)` to
  `Entry::new(action::PASSKEY_CLEARED)`.

Neither call site's `.actor(...)` / `.from_request(...)` chain changes — only the action string.

## §1a The admin-assisted reset's own org-scoped event

`crates/of-web/src/routes/orgs.rs`, `reset_member_passkeys`, currently writes:

```rust
tx.audit(
    Entry::new(action::TOTP_RESET)
        .actor(ctx.user.id)
        .target("user", target.to_string()),
)
```

**A new constant, not a reuse of `PASSKEY_CLEARED`.** This event and `clear`'s own `PASSKEY_CLEARED`
write are not the same fact from two places — they are two different facts about the same request,
with different actors: `clear`'s write says "this account's passkeys were removed", attributed to
the account itself (global, no org); this write says "an admin reset a member's passkeys",
attributed to the admin, org-scoped, naming the member as `target`. That is the same shape as the
existing `MEMBER_ROLE_CHANGED` / `MEMBER_REMOVED` pair in the "Org administration" section of
`audit::action` — an admin acting on a member, inside that org's own trail. The new constant belongs
there, not next to `PASSKEY_REGISTERED`/`PASSKEY_CLEARED`:

```rust
pub const MEMBER_PASSKEYS_RESET: &str = "org.member.passkeys_reset";
```

`reset_member_passkeys`'s write changes to `Entry::new(action::MEMBER_PASSKEYS_RESET)`, with its
`.actor(...)` / `.target(...)` chain unchanged. This write already uses `tx.audit(...).await?` — it
propagates its error, not best-effort — so §2 does not apply to it: it fails the request (500) if
the audit write fails, which is existing behavior this spec does not change.

**Correction: this write is not inside the same transaction as the claim code, contrary to an
earlier draft of this section.** `create_account_claim` (`crates/of-core/src/invites.rs`) commits
its own transaction before `let mut tx = state.db.begin(ctx.org.id)` is even opened for the audit
write — by the time this `tx.audit(...)` call runs, `passkeys::clear`, `sessions::revoke_all`, and
the claim-code insert have already committed. A failure here rolls back only this write's own
(otherwise-empty) transaction, and the request answers 500 after those side effects already
happened. This is pre-existing structure — this spec's diff only changes the literal action string
bound in an `.await?` call that was already error-propagating, so no runtime behavior changes here
— but the justification for the asymmetry is narrower than the earlier draft claimed: it is simply
that this write already propagates its error and this spec has no reason to change that, not that
the write is atomic with anything else. Filing the actual non-atomicity as a separate concern is
out of scope for this rename.

## §2 Stop discarding the two audit writes

Both sites currently read `let _ = db.audit_global(...).await;`. Replace with the pattern already
established by `note_failure` in the same file (`passkeys.rs:614-622`):

```rust
if let Err(e) = db
    .audit_global(Entry::new(action::PASSKEY_REGISTERED).actor(user_id))
    .await
{
    tracing::error!(error = %e, user_id = %user_id, "failed to write audit event for passkey registration");
}
```

and equivalently for `clear`, with `"failed to write audit event for passkey clear"` and the
existing `.from_request(ip, None)` chain preserved. **Not propagating the error stays correct** —
`Db::audit_global`'s doc comment already argues this (`audit.rs:189-192`), and this spec does not
revisit that argument, only the fact that failing silently is worse than failing loudly at the log
level nothing else sees.

## §3 Log the two silent credential-deserialization drops

`finish_authentication` (`passkeys.rs:352-356`):

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

`update_stored_credential` (`passkeys.rs:585-596`):

```rust
let Some(raw) = raw else { return Ok(()) };
let passkey = match serde_json::from_value::<Passkey>(raw) {
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
let mut passkey = passkey;
```

(`let mut passkey = passkey;` — or an equivalent restructuring — because the original binds `mut`
directly in the `let Ok(...) else` pattern; either form is acceptable as long as `passkey` stays
mutable for the `update_credential` call below it.)

Both messages name which of the two call sites hit — registration-time lookup during sign-in versus
the sign-counter update after a successful one — because the second is the one `CLAUDE.md` calls
out as also silently disabling clone detection, and a shared message would make that specific case
harder to grep for.

**Why this does not touch the safety argument in §6 of `docs/specs/2026-09-09-passkey-labels-and-signals-design.md`.**
That section's whole point is that a credential row that exists but fails to deserialize still
*resolves an owner* — the lookup that decides `UnknownCredential` vs. `InvalidCredentials` runs
against the `passkeys` table by `credential_id`, before any row's `credential` JSON is parsed at
all. Logging the parse failure after that lookup has already happened changes nothing about which
of the two errors a caller sees.

## Testing

`crates/of-auth/tests/passkeys.rs`:

- Extend (or add alongside) `a_passkey_creates_an_account_and_signs_back_into_it`-style coverage
  with an assertion that registration writes exactly one `PASSKEY_REGISTERED` row for the new
  account — same query shape as the existing `login_failures` helper, parameterized on action.
- Extend `clearing_passkeys_leaves_no_way_in` with the equivalent assertion for
  `PASSKEY_CLEARED`.
- A new test asserting a corrupted stored `credential` column (`UPDATE passkeys SET credential =
  '{"garbage": true}' WHERE ...`) does not change `finish_authentication`'s outcome for an account
  with only that one key: it still reaches `InvalidCredentials` via the existing
  `keys.is_empty()` branch, not a panic or a different error. This is the regression the `filter_map`
  rewrite in §3 is most likely to introduce if the match arms are transposed.

No test asserts the log line's text — see Scope/Out. `cargo test -p of-auth --test passkeys` is the
gate for the `of-auth` half; `cargo clippy --all-targets -- -D warnings` and `cargo fmt --all
--check` cover the rest.

`crates/of-web/tests/console.rs` (or wherever `reset_member_passkeys` is already covered — grep for
its route before adding a new test file): extend the existing coverage of the admin-reset endpoint
with an assertion that the org's audit trail gains a `MEMBER_PASSKEYS_RESET` row attributed to the
admin, targeting the affected member, and not a `TOTP_RESET` row.

## Error Handling & Edge Cases

- **Both audit writes still fail open.** A database outage during registration or clearing must not
  turn either into a user-visible error; §2 only adds a log line on that branch.
- **Every stored credential for an account fails to deserialize.** Unchanged: `keys.is_empty()`
  still produces `InvalidCredentials`, now preceded by one `error` line per corrupt row instead of
  none.
- **Only the sign-counter update's row fails to deserialize.** Unchanged: `update_stored_credential`
  still returns `Ok(())` and clone-detection is still silently skipped for that credential — §3 adds
  the log line `CLAUDE.md`'s description of this code already calls for, it does not change the
  brittleness `CLAUDE.md` calls out as a known risk.
- **A future contributor deletes `TOTP_ENROLLED`/`TOTP_RESET` outright.** Nothing in the code
  depends on them existing (grep confirms no other reference in the workspace), so deleting them
  compiles cleanly. This spec asks that they stay, documented, as the answer to "what do old rows
  mean" — flagged here so a reviewer removing them later knows it is a deliberate reversal of this
  decision, not a cleanup.

## Risks & Open Questions

- **The two old constants having no writer left is, on its own, a slightly unusual shape** — a
  public constant nothing in the workspace produces. `rg` confirms this is already true of
  `MAGIC_LINK_SENT`, `MAGIC_LINK_CONSUMED`, `RECOVERY_CODE_USED`, and `EMAIL_VERIFIED`
  (`docs/plans/2026-09-04-doc-drift-no-email.md` Task 2), so it is a precedented shape in this file,
  not a new one.
- **This is the second passkey-adjacent change in a row to touch `passkeys.rs`** (after
  `2026-09-09-passkey-labels-and-signals-design.md`). No functional overlap: that work touched
  credential naming and console signals; this touches audit-write outcomes and error logging. Line
  numbers cited above are read from the current tree, post-merge of that work.
