# Passkey registration audit: distinguish signup/claim/add, and carry an IP

> **Status:** IMPLEMENTED — shipped in `savvagent/otto-factory#107`, closing `savvagent/otto-factory#88`,
> filed as a lower-severity follow-up during the review of `savvagent/otto-factory#86` (see
> `docs/specs/2026-09-10-passkey-audit-events-design.md`, which shipped the `auth.passkey.registered`
> / `auth.passkey.cleared` rename this spec builds on). Deployed and verified via the merge commit's
> own CI run (`cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all
> --check`, `npm run check`/`lint`/`test` all green) and the automatic `flyctl deploy` to
> `otto-factory-mcp` that followed it. Three follow-up issues were filed rather than folded in, per
> the independent security reviewer's own recommendation not to hold the merge for them: `#108`
> (`finish_registration`'s credential INSERT and its audit write are not atomic), `#109`
> (`claim_finish`/`add_passkey_finish` write the audit row before the ceremony-ownership check that
> can still reject the request), `#110` (`client_ip` stores the proxy header value unvalidated).
>
> **Amendment (PR #107 review):** the original plan for `via` below is `via: &str` with call-site
> string literals, matching every other `.detail(json!({...}))` call site in the codebase (see the
> "No `via` enum" bullet in Scope/Out). Two independent reviewers on the mandatory review trio
> (architect-reviewer and the type-design-analyzer pass) flagged that this specific call site differs
> from that precedent in a way that matters: `via` crosses a crate boundary (`of-web` → `of-auth`) as
> a function parameter, which is exactly the shape `crates/of-core/src/audit.rs`'s own module doc
> warns about for action names ("a typo in a literal produces an event nobody will ever find") — the
> six precedent call sites are all single-site literals inside one `json!` call, not a value threaded
> across crates. The implementation was changed to a small `of_auth::passkeys::RegistrationVia` enum
> (`Signup` / `Add` / `Claim`) with `.as_str()` feeding the same `json!({"via": ...})` call — the wire
> shape of `detail.via` is unchanged, only the Rust-level parameter type. Every code sample and
> signature below still shows the original `via: &str` design; read `RegistrationVia` wherever `via:
> &str` appears, and read the "No `via` enum" bullet as superseded by this amendment.

## Goal & Success Criteria

`finish_registration` writes one `auth.passkey.registered` row regardless of which of its three
callers reached it — a brand-new signup, an already-signed-in session adding a second key, or someone
redeeming an admin-issued claim code to take over an account whose old credentials were just wiped.
The claim case is the one that matters most: it is the takeover-completion event that follows an
admin-assisted reset (`org.member.passkeys_reset`, `docs/specs/2026-09-10-passkey-audit-events-design.md`
§1a), and today's row cannot distinguish it from an ordinary signup. The row also carries no IP,
unlike every neighboring auth event (`LOGIN_FAILED`, `LOGOUT`, `PASSKEY_CLEARED`).

- `finish_registration` takes two new parameters: `via: &str` (the originating flow) and
  `ip: Option<&str>` (the caller's address, already computed at every call site or trivially
  computable).
- Its audit write carries `.detail(json!({ "via": via }))` and `.from_request(ip, None)`, matching the
  shape `PASSKEY_CLEARED` already uses.
- The three callers — `signup_finish`, `add_passkey_finish`, `claim_finish` — pass `"signup"`,
  `"add"`, and `"claim"` respectively, and each passes the request's IP.
- `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --all --check`
  all pass.

## Premise corrections

None. The issue's file and line references match the current tree (`crates/of-auth/src/passkeys.rs`
lines 216–267 for `finish_registration`; `crates/of-web/src/routes/auth.rs` for the three callers).
`docs/specs/2026-09-10-passkey-audit-events-design.md` has already landed and renamed the action to
`auth.passkey.registered` — this spec's diff sits on top of that shape, not the old `TOTP_ENROLLED`
one the issue's description predates.

## Scope

**In:**

- `finish_registration`'s signature gains `via: &str` and `ip: Option<&str>`.
- Its audit write gains `.detail(json!({ "via": via }))` and `.from_request(ip, None)`.
- The three call sites in `crates/of-web/src/routes/auth.rs` (`signup_finish`, `add_passkey_finish`,
  `claim_finish`) pass their flow name and IP. `add_passkey_finish` gains a `parts: Parts` extractor
  parameter — the only signature change on that side, needed to compute an IP it does not currently
  have.
- The three direct-call test sites in `crates/of-auth/tests/passkeys.rs` are updated for the new
  signature.
- A test asserting the `detail` column's `via` value for at least one non-signup path (claim), since
  that is the path the issue calls out as the one that matters most.

**Out:**

- **No user-agent.** Every existing `.from_request(ip, ...)` call in this codebase passes `None` for
  user agent (`crates/of-auth/src/login.rs`, `crates/of-web/src/routes/orgs.rs`) — nothing threads a
  user agent through today. Adding it here would be new scope beyond what the issue asks for and
  inconsistent with every neighboring call site; a follow-up that threads user-agent through the
  whole auth surface at once is a separate, larger decision.
- **No `via` enum.** Every existing `.detail(json!({...}))` call site in this codebase
  (`crates/of-auth/src/login.rs`, `crates/of-web/src/oauth.rs`, `crates/of-web/src/routes/orgs.rs`,
  `crates/of-mcp/src/tools/jobs.rs`, `crates/of-web/src/routes/tokens.rs`,
  `crates/of-web/src/routes/trackers.rs`) passes plain string literals directly into the `json!`
  macro; none of them define a typed enum first. `via` follows the same convention: a plain `&str`,
  with exactly three call sites each passing a string literal, is not a place a typo can hide the way
  a value threaded through several layers might be — the reviewer checklist below covers it instead of
  a type.
- **No change to `finish_registration`'s error behavior or the write's best-effort-ness.** The audit
  write stays inside the existing `if let Err(e) = ... { tracing::error!(...) }` pattern; a failed
  audit write still does not fail the ceremony.
- **No change to `#87` (the admin-assisted reset's non-atomicity) or `#89` (`passkeys::remove`/`rename`
  writing no audit row, `login::logout` discarding its write) — both are separate, already-filed
  issues this change does not touch.**
- **No new audit action constant.** `auth.passkey.registered` already exists and already means "a
  passkey was registered"; `via` is a detail on the existing action, not a new one — three new actions
  (`auth.passkey.registered.signup` / `.claim` / `.add`) would fragment a single fact into three,
  making "how many registrations happened" require an `IN` clause instead of one filter, for no
  benefit `detail` doesn't already provide.

## Public-interface changes

| Surface | Change | Breaking? |
|---|---|---|
| `of_auth::passkeys::finish_registration` (crate-public fn, used only within the workspace) | Signature gains `via: &str, ip: Option<&str>` | No — not a wire interface; every caller is in this workspace and is updated in the same PR |
| Audit trail / SIEM export | `auth.passkey.registered` rows gain a `detail.via` field and (for `add`/`claim`, which did not set it before) a populated `ip` column | Additive — existing rows are untouched, existing queries that do not reference `detail.via` are unaffected |
| MCP | none | — |
| Console REST | none — `POST /api/auth/signup/finish`, `POST /api/me/passkeys/finish`, `POST /api/auth/claim/finish` keep their existing request/response shapes | — |
| OAuth/discovery | none | — |
| Config | none | — |
| Schema | none — `detail` and `ip` are existing columns on `audit_events`, already written unconditionally | No |

This does not touch `docs/clients/matrix.md`: nothing an MCP coding-agent client sends or receives
moves, and nothing on the console's request/response bodies changes shape.

## §1 `finish_registration`'s new parameters

`crates/of-auth/src/passkeys.rs`:

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
    // ... unchanged up to the audit write ...

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

    Ok(user_id)
}
```

`via` is placed after `nickname` and `ip` last, matching `passkeys::clear(db, user, ip)`'s existing
trailing-`ip` convention in the same file.

## §2 The three call sites

`crates/of-web/src/routes/auth.rs`:

- **`signup_finish`** already computes `let ip = client_ip(&parts, &state.config);` before calling
  `finish_registration`. Add `"signup"` and `ip.as_deref()` to the call.
- **`claim_finish`** already computes `ip` the same way, before `consume_account_claim`. Add
  `"claim"` and `ip.as_deref()` to the call — this is the row the issue calls out as the one that
  matters most, since it is what proves who actually walked through the door after an admin reset.
- **`add_passkey_finish`** computes no IP today because it never needed one. Add a `parts: Parts`
  parameter (the extractor already imported in this file; `Parts` and the existing `caller:
  CurrentUser` extractor both implement `FromRequestParts` and do not consume each other — see the
  precedent at `crates/of-web/src/routes/orgs.rs:503-506`, which already combines the two on one
  handler), compute `let ip = client_ip(&parts, &state.config);`, and add `"add"` and
  `ip.as_deref()` to the call.

None of the three call sites' existing error handling, response shape, or the post-`finish_registration`
ceremony-ownership check (`claim_finish`'s "does the registered account match the claim", `add_passkey_finish`'s
"does it match the caller's session") changes.

## §3 Test updates

`crates/of-auth/tests/passkeys.rs` has three direct calls to `passkeys::finish_registration` that need
the two new arguments (lines 92, 173, 628 as of this writing): `register_new`'s (used pervasively —
pass `"signup"` and `None`), a second-key registration in the two-devices test (pass `"add"` and
`None`, since a second key on an existing account is what that path represents), and the credential-
naming test (pass `"signup"` and `None`).

A new test in the same file, following the shape of `registration_writes_the_passkey_registered_action`
(same file, ~line 383): drive `finish_registration` directly with `via = "claim"` and assert the
resulting row's `detail->>'via'` is `"claim"`. One assertion at the unit level is sufficient — the
three call sites in `of-web` are exercised by existing endpoint tests in `crates/of-web/tests/console.rs`
(signup, add-a-second-key, and the claim-code redemption flow all already have coverage; this spec
does not add endpoint-level assertions on `detail`, since the unit-level test on `finish_registration`
itself is what actually proves the parameter is threaded through, and duplicating that assertion at
the HTTP layer for all three paths is coverage this change does not need to be confident in).

## Testing

- `cargo test -p of-auth --test passkeys` — the new `via` assertion, plus the three updated call
  sites still compiling and passing.
- `cargo test -p of-web --test console` — confirm the existing signup / add-passkey / claim-finish
  endpoint tests still pass unchanged (they assert response shape and status, not audit detail, and
  none of that is expected to change).
- `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`.

## Error Handling & Edge Cases

- **The audit write still fails open.** Adding `via`/`ip` does not change what happens if the write
  itself fails — the existing `tracing::error!` branch is unchanged in shape, just carries two more
  fields on the `Entry`.
- **`via` is caller-supplied, not derived.** A future fourth caller of `finish_registration` that
  forgets to pass a real value would compile (any `&str` satisfies the parameter) and write a
  misleading `detail.via`. This is the tradeoff Scope/Out accepts by not introducing an enum: the
  three existing call sites are covered by the plan's file-by-file steps, and `rg
  "finish_registration("` finding exactly three non-test call sites is the check a reviewer or a
  future contributor runs by hand. Recorded here so a reviewer adding a fourth caller later knows to
  check for this rather than assuming the compiler would catch a missing value.
- **`add_passkey_finish` gaining a `Parts` extractor.** `Parts: FromRequestParts` clones from the
  request rather than consuming it (as does `CurrentUser`), so ordering relative to `caller:
  CurrentUser` in the signature does not matter functionally; this spec places it after `caller` to
  match the existing precedent at `orgs.rs:503-506`.

## Risks & Open Questions

- **None outstanding.** This is a narrow, additive change to one function's parameters and three
  call sites already identified by the issue; the two items it deliberately does not fold in
  (`#87`, `#89`) are already tracked separately.
