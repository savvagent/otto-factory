# Passkey account labels and WebAuthn credential signals — a vault entry that names an account

## Goal

Make two accounts distinguishable in a password manager's picker, and make a deleted passkey stop
being offered. Every account gets a generated human-memorable label at creation, the registration
challenge names the credential with it, and the console adopts the three WebAuthn signal methods so
the browser's credential store and the `passkeys` table stop drifting apart. The signals are what
make the fix retroactive: a credential already sitting in somebody's vault is relabelled by signing
in once.

## Status — 2026-09-09

🚧 Tasks 1–7 implemented and reviewed; **Task 5b implemented and then reverted** (see its section).
Open as `savvagent/otto-factory#74`, CI green. **Remaining:** human review and merge, then Task 8's
record-as-shipped pass.

Review found more than the tasks did, and the pattern is worth keeping: the independent
security review — given only the diff, without this plan, the spec, the issues or the PR body —
is what showed Task 5b's throttle was a shared-IP denial of service on the only sign-in path, after
two earlier reviewers had accepted it. Four stale comments and one stale route description were
found by reviewers rather than by the tasks that invalidated them, and two of those were written
during this change while fixing something else. Deferred, filed rather than folded in:
`#64`, `#72`, `#73`, `#75`, `#76`. Closes `savvagent/otto-factory#59` and `savvagent/otto-factory#60`, which
ship together on purpose — #59 fixes labels for accounts created after the deploy, and #60's
`signalCurrentUserDetails` is the only thing that repairs the credentials that already exist.

**Spec:** `docs/specs/2026-09-09-passkey-labels-and-signals-design.md` — read it first. This plan
implements it exactly.

## Global Constraints

- **Signup still takes no request body.** The label is minted for a row that has just been
  inserted; it is never derived from, seeded by, or checked against anything a caller supplies. Any
  step that would add a branch revealing whether an account exists is the wrong step
  (`CLAUDE.md`, invariant 4).
- **Both webauthn-rs overrides stay untouched.** `require_resident_key(false)` on registration and
  `mediation: conditional` on authentication are on the challenge, never on the verification state.
  Account resolution stays by credential ID, never the user handle.
- **No SQL leaves `of-core`** — except inside `crates/of-auth/src/passkeys.rs`, which already owns
  the statements against the non-tenant `passkeys` / `webauthn_ceremonies` tables. Extend those in
  place; do not add a new SQL site in `of-web` or `of-mcp`, and do not refactor the existing
  `of-auth` ones (out of scope).
- **No new tenant table and no new tenant-scoped function.** `users` itself has no `org_id` and is
  absent from `0007_rls.sql`'s `tenant_tables`, so the new column carries no policy and needs no
  `<table>_tenant_isolation` name. But Task 1 does edit two **org-scoped** queries —
  `list_org_members` (`org_id = $1` on `Db`, guard 1) and `list_team_members` (`tm.org_id = $1` on
  `Tx`, and `team_members` **is** a tenant table with an RLS policy). The rule that holds is
  narrower than "users is not a tenant table": **`u.label` is one more column off the already-joined
  non-tenant `users`, and no `WHERE` predicate moves.** Touch a predicate and the isolation coverage
  in `crates/of-core/tests/membership.rs` and the `rls_scopes_*` cases in `tests/isolation.rs`
  (which already exercise `list_team_members` cross-org) is what must stay green. `0007_rls.sql` is
  not edited and still runs last.
- **Migrations are forward-only.** `0022_user_label.sql` is a new file; no applied migration is
  edited.
- **No new MCP tool**, so no `of-billing::classify` entry and no `tools::out` envelope changes;
  `exhaustive_over` / `every_tool_has_a_price` are untouched.
- **No mailer, no password, no recovery code.** Nothing here notifies anybody of anything.
- **No `unwrap()` outside tests.** No silent fallback on a resolution failure — with one deliberate,
  commented exception: the browser-side signal helpers swallow every error, because a password
  manager refusing a hint must never fail the sign-in it is attached to.
- **No AI self-attribution** anywhere — commits, comments, docs, PR body.
- Rust gates before every commit: `cargo fmt --all`, then
  `cargo clippy --all-targets -- -D warnings` and `cargo test --workspace`. Tests need
  `podman compose up -d` (Postgres 16 on host port 15433) and a `.env` (`cp .env.example .env`).
- Console gates: `cd web && npm install` first (a fresh worktree has no `node_modules`), then
  `npm run check`, `npm run lint`, `npm test`, `npm run build`.
- Out-of-band artifacts touched: **`crates/of-core/migrations/`** (Task 1 — verify a fresh cluster
  applies cleanly) and the **console bundle** (`web/` — Tasks 6–7). Not touched: the container image
  (`Dockerfile`, `fly.toml`), the Cloudflare Worker (`web/worker/`, `web/wrangler.jsonc`),
  `.github/workflows/`, `.env.example` (no new `OF_*` key).

## File Structure

| File | Responsibility |
| --- | --- |
| `crates/of-core/migrations/0022_user_label.sql` | **Create.** Add `users.label`, backfill, `SET NOT NULL`. |
| `crates/of-core/src/labels.rs` | **Create.** `generate() -> String` — `adjective-noun-NN`. |
| `crates/of-core/src/lib.rs` | **Modify.** Declare and re-export the `labels` module. |
| `crates/of-core/src/orgs.rs` | **Modify.** `User.label`, `OrgMember.label`, `USER_COLS`, both insert sites, the member query. |
| `crates/of-core/src/teams.rs` | **Modify.** `TeamMember.label` and its query. |
| `crates/of-core/tests/` | **Modify/Create.** Label generation, stability, and both insert paths. |
| `crates/of-auth/src/passkeys.rs` | **Modify.** `RP_NAME`; name the challenge from the account; `credential_id` on `RegisteredKey`; `UnknownCredential` from `finish_authentication`. |
| `crates/of-auth/tests/passkeys.rs` | **Modify.** Challenge naming, label stability, user-handle encoding, the unknown-credential answer. |
| `crates/of-web/src/routes/auth.rs` | **Modify.** `webauthn_config` handler. |
| `crates/of-web/src/catalog.rs` | **Modify.** Mount `GET /api/auth/webauthn`, `Auth::Public`. |
| `crates/of-web/src/openapi.rs` | **Modify.** `User.label`, `Passkey.credentialId`, `WebauthnConfig`. |
| `crates/of-web/tests/console.rs` | **Modify.** rp_id endpoint, credential-id round trip, label on `/api/me`. |
| `web/src/lib/webauthn.ts` | **Modify.** `userHandle`, the three signal helpers, `rpId` caching, `authenticate()`'s return type. |
| `web/src/lib/webauthn.test.ts` | **Create.** No-op-when-absent, call-through, handle encoding. |
| `web/src/lib/api.ts` | **Modify.** `webauthnConfig()`. |
| `web/src/lib/types.ts` | **Modify.** `User.label`, `OrgMember.label`, `Passkey.credentialId`, `WebauthnConfig`. |
| `web/src/lib/format.ts` | **Modify.** `person()` takes the label, losing its unnamed fallback. |
| `web/src/routes/{signup,login,claim,settings}/+page.svelte` | **Modify.** The four signal call sites. |
| `web/src/routes/+layout.svelte`, `web/src/routes/invite/[org]/+page.svelte`, `web/src/routes/o/[org]/members/+page.svelte` | **Modify.** Render the label instead of a "no email" placeholder. |
| `web/messages/{en,es,de,fr,it,hi}.json` | **Modify.** Remove keys that become unreferenced. |

## Task Order & Rationale

The column comes first because everything else reads it: the challenge (Task 2), the console's
rendering (Task 7), and the signal payload (Task 6) all need a label to exist and to be stable.
Task 8 is the close-out. Tasks 3–5 are three independent server-side additions that the console half needs before it can be
written — the rp_id to send signals with, the credential ids to send, and the error code that says
when to send `signalUnknownCredential`. They are separate tasks rather than one because they touch
different surfaces and fail differently. Task 6 builds and proves the browser helpers against their
own tests, where the encoding risk lives; Task 7 is then a mechanical wiring of proven helpers into
pages, with nothing left to discover.

## Task 1 — `users.label`: the column, the generator, and both writers ⬜

**Files:** `crates/of-core/migrations/0022_user_label.sql` (new), `crates/of-core/src/labels.rs`
(new), `crates/of-core/src/lib.rs`, `crates/of-core/src/orgs.rs`, a test file under
`crates/of-core/tests/`
**Interfaces:** produces `of_core::labels::generate()`, `User.label`, `OrgMember.label`; consumes
nothing.

- [ ] Write the failing tests first, as bare `#[sqlx::test]` cases in a new
      `crates/of-core/tests/labels.rs` plus unit tests inside `labels.rs`. Bare, not
      `#[sqlx::test(migrations = "...")]` — `./migrations` is sqlx's default and every existing file
      in `crates/of-core/tests/` relies on it; the explicit form is what `crates/of-web/tests/*`
      needs only because its path is `../of-core/migrations`.
      - `generate()` matches `^[a-z]+-[a-z]+-[0-9]{2}$`, and 1,000 calls produce more than 900
        distinct values — the assertion that catches the failure that matters, a generator collapsed
        toward a constant. Do **not** assert two draws always differ: that is a ~1-in-576,000 flake.
      - `create_unclaimed_user` returns a non-empty `label`, and two calls differ.
      - `upsert_user` returns a non-empty `label` — the enterprise-OIDC insert path.
      - `get_user` reads back exactly the label the insert returned.
      - `set_profile` leaves the label alone when it sets an email and a name.
      - `list_org_members` and `list_team_members` each carry the member's label.
- [ ] Run `cargo test -p of-core --test labels` — expect compile failure (no `labels` module, no
      `label` field).
- [ ] Create `crates/of-core/migrations/0022_user_label.sql`:
      `ALTER TABLE users ADD COLUMN label TEXT;` → backfill every existing row with a random
      `adjective-noun-NN` drawn from small literal arrays inline in the SQL → `ALTER TABLE users
      ALTER COLUMN label SET NOT NULL;`. Comment the arrays: the duplication with the Rust generator
      is deliberate, the backfill runs once per cluster and never again, so the two cannot drift in
      any way that matters. Do **not** add a `UNIQUE` index (spec Scope/Out: a cosmetic collision
      must not become an insert failure on the signup path). Do not touch `0007_rls.sql`.
- [ ] Create `crates/of-core/src/labels.rs` with `pub fn generate() -> String`: 80 adjectives × 80
      nouns × `10..=99`, `rand::thread_rng()`, joined with `-`. Comment **why it is random and not
      derived from the id** — a function of the primary key would let anybody who learned an id
      recover the label. Comment that the label stays English: it is a handle, not prose, and a
      translated one would make the picker and the console disagree. Give the module a doc comment
      saying which `labels` this is — `web/src/lib/labels.ts` already exists and means translated
      words for `JobStatus`/`Role`; the two never meet but are one grep apart. Declare the module in
      `crates/of-core/src/lib.rs`.
- [ ] In `crates/of-core/src/orgs.rs`: add `pub label: String` to `User` with a doc comment saying
      what it is for (a vault entry that names an account, not an identifier — nothing looks an
      account up by it); add `label` to `USER_COLS`; bind `labels::generate()` in
      `create_unclaimed_user` **and** `upsert_user`; add `label: String` to `OrgMember` and `u.label`
      to the member query's column list. Do the same for `TeamMember` in
      `crates/of-core/src/teams.rs` — `person()` is shared between the org-members and teams pages,
      and a required third argument has to compile at both. Leave `set_profile` alone.
      **Do not touch `TeamMember.email`.** It is `String` where `users.email` is nullable, which is
      a real pre-existing decode bug, but widening a non-null console field is a larger decision
      than this change — it is filed as `savvagent/otto-factory#64` (spec Risks).
- [ ] Run `cargo test -p of-core --test labels` — expect green.
- [ ] Run `cargo test --workspace` — expect green (other crates may need `label` in `User`
      constructions; fix any that do not compile).
- [ ] Verify a fresh cluster applies cleanly: `podman compose down -v && podman compose up -d` then
      `cargo test -p of-core`.
- [ ] `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, commit as
      `of-core: give every account a generated label`.

## Task 2 — Name the registration challenge from the account ⬜

**Files:** `crates/of-auth/src/passkeys.rs`, `crates/of-auth/tests/passkeys.rs`
**Interfaces:** consumes `User.label` from Task 1; produces the challenge naming every later task
depends on.

- [ ] Add failing tests to `crates/of-auth/tests/passkeys.rs`:
      - A brand-new account's challenge has `displayName != "otto-factory"` and a `name` that is
        not the bare product name either.
      - Two consecutive `start_registration(db, &rp, None)` calls produce different `displayName`s.
      - A second `start_registration` for the **same** account produces the **same** `displayName`
        as the first — the stability rule, and the exact confusion #59 is about.
      - `credential_names` is a pure function of a `User`: unit-test the three `name` precedences
        (email, then name, then label) and that `display_name` carries the label in all three.
      - After `set_profile` sets an address, `name` becomes the address and `displayName` still
        carries the same generated words.
      - `user.id` in the challenge is base64url of the account UUID's **16 raw bytes** — the
        server half of the encoding pair the console must match (spec Risks).
- [ ] Run `cargo test -p of-auth --test passkeys` — expect failure.
- [ ] In `crates/of-auth/src/passkeys.rs`: add `const RP_NAME: &str = "otto-factory";` and use it in
      `relying_party()`. Add `pub struct CredentialNames { pub name: String, pub display_name: String }`
      and `pub fn credential_names(user: &User) -> CredentialNames` — `name` = `email` → `name` →
      `label`, `display_name` = `format!("{RP_NAME} · {label}")`. Rewrite `start_registration` so
      both arms produce the account's `User` and call it; delete the `"otto-factory".to_string()`
      fallback. **Comment why this is a function and not two `format!`s at the call site:** the
      console sends the same pair to `signalCurrentUserDetails`, and a second copy of the prefix and
      the precedence rule in TypeScript would drift silently — the signal would still be accepted
      and would write a label subtly unlike what a fresh registration writes. Task 3 exports this
      pair on `/api/me` for exactly that reason. Also comment why `displayName` keeps the generated
      words once an address exists — an owner who learned them does not lose them. Change nothing
      else in the ceremony: both overrides, the exclude-credentials list, and `store_ceremony` stay
      as they are.
- [ ] Run `cargo test -p of-auth --test passkeys` — expect green.
- [ ] `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`,
      commit as `of-auth: name a passkey after the account, not the product`.

## Task 3 — Give the console what it must signal with ⬜

**Files:** `crates/of-web/src/routes/auth.rs`, `crates/of-web/src/catalog.rs`,
`crates/of-web/src/openapi.rs`, `crates/of-web/tests/console.rs`
**Interfaces:** consumes `credential_names` from Task 2; produces the `rpId` and the composed name
pair the console's signal helpers send.

- [ ] Add failing tests to `crates/of-web/tests/console.rs`:
      - `GET /api/auth/webauthn` answers `200` **with no session cookie**, and its `rpId` equals the
        host in the harness's `PUBLIC_URL` — the same value `of_web::relying_party` built the
        challenge from. This is the test that would have caught a console shortcut to
        `location.hostname`.
      - `console_signal_matches_the_challenge`: start a registration and read `/api/me`; the
        challenge's `user.name` / `user.displayName` equal `/api/me`'s `credentialName` /
        `credentialDisplayName` exactly. This is what keeps the signal writing byte-for-byte what a
        fresh registration would write, rather than trusting two implementations of one rule.
      - `/api/me`'s `user.label` is present and non-empty.
- [ ] Run `cargo test -p of-web --test console` — expect failure.
- [ ] Add `auth::webauthn_config`: `State(state)` only, no extractor, returning
      `Json(WebauthnConfig { rp_id })` serialized `camelCase`. Source it from `state.config.rp_id()`.
      On `None`, return an internal error naming `OF_PUBLIC_URL` — **not** a `null` field and **not**
      an `unwrap()`: the server cannot have started without it (`of_web::relying_party` refuses at
      boot), and a `null` would make the console silently skip every signal. Doc-comment that the
      value is public by necessity — it is in every challenge an unauthenticated caller can ask for.
- [ ] Mount it in `catalog.rs` as `Endpoint::get("/api/auth/webauthn", auth::webauthn_config)
      .auth(Auth::Public).returns("WebauthnConfig")` with a summary and a `describe` written for a
      reader who has never seen these docs: what it is, and that a console must read the rp_id here
      rather than guessing it from the page's hostname, because the rp_id may be a registrable
      parent domain of the origin.
- [ ] Add `credential_name` and `credential_display_name` to the `Me` struct in
      `crates/of-web/src/routes/auth.rs`, both filled by `passkeys::credential_names(&caller.user)`.
      Doc-comment them: the console must never compose these itself — see Task 2.
- [ ] `openapi.rs`: add the new `WebauthnConfig` component, and add `label` to `User`, `OrgMember`
      **and** `TeamMember` (Task 1 added all three fields), plus `credentialName` /
      `credentialDisplayName` to `Me` — each in `properties` **and** in `required`, since none is
      nullable. All the schema work for the `label` column lands here, in one step, rather than
      trailing into Task 4: nothing in the suite compares document fields against struct fields
      (`every_referenced_schema_is_defined` only checks that named components *exist*), so a schema
      step skipped in a task about something else ships silently. `every_referenced_schema_is_defined` is the gate that
      fails if a `.returns(…)` names a component that does not exist.
- [ ] **While in `response_schemas()`, correct the two stale schemas it holds.** `Me` and
      `SessionOpened` still document `mustEnrollTotp` / `recoveryCodesRemaining`
      (`openapi.rs:715-731`); the structs carry `shouldAddPasskey` and `passkeyCount`
      (`routes/auth.rs:331`, `:114`) and have since TOTP was removed. Replace the fictional fields
      with the real ones in both, and fix both `required` arrays. Nothing else in the function is
      touched, and the correction is called out in the PR body so it is not read as scope creep —
      it is here only because Task 3 has to edit `Me` anyway, and adding two true fields beside two
      false ones would be shipping a document known to be wrong.
- [ ] Run `cargo test -p of-web --test console` — expect green, including
      `every_documented_get_is_actually_mounted`, which now exercises the new route for free.
- [ ] `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, commit as
      `of-web: publish the relying-party id and the composed credential name`.

## Task 4 — `credentialId` on the passkey list ⬜

**Files:** `crates/of-auth/src/passkeys.rs`, `crates/of-web/src/openapi.rs`,
`crates/of-web/tests/console.rs`
**Interfaces:** produces the ids `signalAllAcceptedCredentials` sends.

- [ ] Add a failing test to `crates/of-web/tests/console.rs`: after an account registers a passkey,
      `GET /api/me/passkeys` returns a `credentialId` for every key, and base64url-decoding it
      yields exactly the bytes in that row's `passkeys.credential_id`.
- [ ] Run `cargo test -p of-web --test console` — expect failure.
- [ ] Add `pub credential_id: String` to `RegisteredKey` (serialized `credentialId`), select
      `credential_id` in `passkeys::list`, and encode it base64url **unpadded** — the same alphabet
      the ceremony helpers use, so the console can compare without re-encoding. Doc-comment it: this
      is a public key handle the authenticator already holds and a browser is expected to see, it is
      **not** a secret, and it is here because `signalAllAcceptedCredentials` cannot work without it
      — so nobody removes it later assuming otherwise.
- [ ] Add `credentialId` to the `Passkey` schema in `openapi.rs` — `properties` **and** `required`.
      (`OrgMember` and `TeamMember` gained `label` in Task 3's schema step, alongside `User`.)
- [ ] Run `cargo test -p of-web --test console` and `cargo test -p of-auth` — expect green.
- [ ] `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, commit as
      `of-auth: return each passkey's credential id`.

## Task 5 — Tell an unknown credential from a bad signature ⬜

**Files:** `crates/of-auth/src/passkeys.rs`, `crates/of-auth/tests/passkeys.rs`
**Interfaces:** produces the `unknown_credential` code the login page branches on. **This is the one
auth-spine behaviour change in the work** — flag it in the PR body for the architect and security
reviewers.

- [ ] Add failing tests to `crates/of-auth/tests/passkeys.rs`:
      - An assertion presenting a credential id this server has never stored fails with
        `AuthError::UnknownCredential`.
      - An assertion presenting a **known** credential with a bad signature still fails with
        `AuthError::InvalidCredentials` — the collapse downstream of the lookup is preserved.
      - After `passkeys::clear`, signing in with the cleared account's old credential answers
        `UnknownCredential` (the row is gone), and no audit row is written for it.
- [ ] Run `cargo test -p of-auth --test passkeys` — expect failure.
- [ ] In `finish_authentication`, return `AuthError::UnknownCredential` on the `owner == None`
      branch instead of `InvalidCredentials`. Leave every downstream failure collapsed and leave
      `note_failure` uncalled on this branch — there is still no account to attribute a row to.
      Comment **why this specific distinction is not the enumeration leak the rest of the module
      avoids**: a credential id is unguessable, is not disclosed cross-origin, and the caller asking
      already holds it; it resolves no account, address, or org; and the alternative is a console
      that must either evict good passkeys after one cancelled prompt or leave a dead key haunting
      the picker of somebody who is already locked out.
- [ ] **Correct the two comments that now contradict the code.** Neither is load-bearing on a test
      — `credential_failures_are_one_answer` iterates `UnknownUser` / `NoPasskey` /
      `InvalidCredentials` / `Disabled` and never included `UnknownCredential` — so nothing fails if
      they are left behind, which is exactly why they are a step:
      - `crates/of-web/src/error.rs` — the doc comment on `credential_failures_are_one_answer`
        currently says "an unknown credential, a bad signature, and a disabled account are one
        answer". Rewrite it to say what is still collapsed and what is now distinguished, and why.
      - `crates/of-auth/src/passkeys.rs:266-268` — "nothing to distinguish for the caller" is now
        false. Replace it with the argument above.
- [ ] Confirm no other of-web change is needed: `AuthError::UnknownCredential` already maps to
      `unknown_credential` (`crates/of-web/src/error.rs:243`), both variants already map to HTTP
      400, and the console already has `m.error_unknown_credential()` ("That passkey is not
      registered here. Try a different one.") in all six catalogs.
- [ ] Run `cargo test -p of-auth --test passkeys` and `cargo test --workspace` — expect green.
- [ ] `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, commit as
      `of-auth: say when a presented credential is one this server never stored`.

## Task 5b — Throttle `login/finish` — **implemented, then reverted** ⬜→✅(reverted)

**Files:** none, in the end. Implemented as `1b6ec94` and reverted in full.

Added mid-implementation because Task 5's implementer flagged that `login/finish` had never been
throttled, reaches its new `unknown_credential` branch before verifying a signature, and writes no
audit row — an unbounded, untraceable probe. The independent security review of the finished PR then
showed the cure was worse than the disease, and the reasoning that justified it was wrong:

- `ratelimit` is an exponential **lockout**, not a rate cap. Keyed on a shared source address, ~12
  unauthenticated requests lock every account behind an office NAT, a CGNAT pool or a VPN exit out
  of the console for thirty minutes, renewably — on the only human sign-in path this product has.
- The `record`-not-`charge` argument (a colleague's success clears what another's flapping
  authenticator accumulated) does not survive contact with the order of operations: `check` runs
  before the credential work, so past the threshold nobody can reach the success that resets it.
- A success resets the bucket, so it never bounded the oracle: one throwaway account buys four free
  probes per authentication, indefinitely.
- The distinction was already observable by **timing** before Task 5 touched any error code — the
  known-credential path fetches keys, verifies a signature and writes an audit row; the unknown path
  does none of them. The throttle was never the control closing this.

Doing it properly needs a rate-cap policy `ratelimit` does not have and a key an attacker cannot
force a stranger to share. That is a design change to a security control and belongs in its own spec
with its own review, not in a mid-PR patch. Filed as `savvagent/otto-factory#75` instead.

## Task 6 — The browser signal helpers ⬜

**Files:** `web/src/lib/webauthn.ts`, `web/src/lib/webauthn.test.ts` (new),
`web/src/lib/api.ts`, `web/src/lib/types.ts`
**Interfaces:** consumes Tasks 3–5; produces `userHandle`, `signalAccount`,
`signalAcceptedCredentials`, `signalUnknownCredential` for Task 7.

- [ ] `cd web && npm install` if `node_modules` is absent.
- [ ] Write `web/src/lib/webauthn.test.ts` first (plain vitest, no jsdom needed — stub
      `globalThis.PublicKeyCredential`):
      - `userHandle('00112233-4455-6677-8899-aabbccddeeff')` equals the base64url, unpadded,
        of those 16 bytes — the console half of the encoding pair Task 2 asserted server-side. A
        second case with a UUID whose bytes force `-` and `_` in the output.
      - Each helper resolves and does nothing when `PublicKeyCredential` is undefined.
      - Each helper resolves and does nothing when `PublicKeyCredential` exists but the specific
        signal method is undefined — feature detection is per method, not per interface.
      - Each helper calls through with the expected argument object when the method exists,
        including the `rpId` fetched from `/api/auth/webauthn`.
      - `signalAccount` forwards `credentialName` / `credentialDisplayName` from `/api/me`
        **verbatim** — it must not compose, prefix, or fall back to `email`/`label` itself. A test
        that passes a `Me` whose `credentialDisplayName` is a sentinel string and asserts that exact
        string reaches `signalCurrentUserDetails` is what stops the duplication creeping back.
      - `signalAcceptedCredentials` forwards each key's `credentialId` **un-re-encoded** into
        `allAcceptedCredentialIds` — the second half of the same encoding-agreement risk the handle
        test covers, and the invariant Task 4's doc comment states ("so the console can compare
        without re-encoding").
      - A helper whose underlying method **rejects** still resolves — the caller can `await` it and
        nothing throws.
      - A helper does nothing when the rp_id could not be fetched, rather than falling back to
        `location.hostname`.
- [ ] Run `npx vitest run src/lib/webauthn.test.ts` — expect failure.
- [ ] Add `WebauthnConfig` to `types.ts` and `webauthnConfig: () => get<WebauthnConfig>('/api/auth/webauthn')`
      to `api.ts`. Add `credentialName` / `credentialDisplayName` to `Me`. Add `label: string` to
      `User`, `OrgMember`, and `TeamMember`, and
      `credentialId: string` to `Passkey` in `types.ts`, with a doc comment on `credentialId`
      repeating that it is a public handle and not a secret.
- [ ] Implement in `web/src/lib/webauthn.ts`: `userHandle(uuid)`; a module-level cached
      `rpId()` promise reading `/api/auth/webauthn` **once** per page lifetime — the same shape as
      `connect/+page.svelte` reading the MCP endpoint out of the discovery document rather than
      baking it in; and `signalAccount(me)`, `signalAcceptedCredentials(me, keys)`,
      `signalUnknownCredential(credentialId)`. `signalAccount` reads `me.credentialName` and
      `me.credentialDisplayName` straight off `/api/me` and composes nothing. Each: resolve the rp_id, return early if it or the
      method is absent, call, and swallow every error in a `catch`. **Comment the swallow** —
      it is the one place in this console where discarding an error is correct, and without the
      comment it reads as the silent-fallback antipattern the house style forbids. Widen
      `authenticate()`'s return type to `{ rawId: string; [k: string]: unknown }` so a caller can
      name the credential it just offered.
- [ ] Run `npx vitest run src/lib/webauthn.test.ts` — expect green.
- [ ] `npm run check`, `npm run lint`, `npm test` — expect green. Commit as
      `web: add the WebAuthn signal helpers`.

## Task 7 — Wire the signals in, and show the label ⬜

**Files:** `web/src/routes/{signup,login,claim,settings}/+page.svelte`,
`web/src/routes/+layout.svelte`, `web/src/routes/invite/[org]/+page.svelte`,
`web/src/routes/o/[org]/members/+page.svelte`, `web/src/lib/format.ts`,
`web/messages/{en,es,de,fr,it,hi}.json`
**Interfaces:** consumes Task 6.

- [ ] Signal call sites, each **after** the existing `session.refresh()`, each `await`ed but unable
      to throw. **Read `credentialName` / `credentialDisplayName` off `session.me`, never off the
      response the ceremony just returned** — `signup/finish`, `login/finish` and `claim/finish`
      answer `SessionOpened` and `PATCH /api/me` answers a bare `User`, and none of them carries the
      pair. Finding the fields missing there and composing them in TypeScript instead is the exact
      drift Task 2 exists to prevent:
      - `signup/+page.svelte` — after `signupFinish` succeeds → `signalAccount`. Also after
        `saveProfile` on the second step, which is where the address first exists.
      - `login/+page.svelte` — after `loginFinish` succeeds → `signalAccount`; in the `catch`, when
        the error's code is `unknown_credential`, → `signalUnknownCredential(credential.rawId)`.
        **Hoist `credential` to a `let` declared above the `try`**: it is `const`-declared inside the
        block today (`:40`) and is not in scope in the `catch` at `:44`. Keep rendering the existing
        translated message; add no new string.
      - `claim/+page.svelte` — after `claimFinish` succeeds → `signalAccount`.
      - `settings/+page.svelte` — after `saveProfile`'s `setProfile` succeeds → `signalAccount`;
        after every `keys = await api.passkeys()` refresh (both `addPasskey` and `act`) →
        `signalAcceptedCredentials`. Signalling after an add or a rename is harmless and idempotent;
        signalling after a remove is the point.
- [ ] Render the label where the console names an account with no address:
      `format.ts`'s `person(name, email, label)` → `name ?? email ?? label`, losing its
      `common_unnamed_account()` fallback; `+layout.svelte:199` → `email ?? label`;
      `members/+page.svelte` → `member.email ?? member.label` at both sites; the two `person()`
      call sites in `o/[org]/teams/+page.svelte` pass `member.label` as the new third argument;
      `invite/[org]/+page.svelte` → `session.me?.user.email ?? session.me?.user.label ?? …`, keeping
      its existing fallback for the not-signed-in case.
- [ ] Delete from all six `web/messages/*.json` the keys this leaves unreferenced — expected to be
      `nav_no_email`, `members_no_email`, `common_unnamed_account`, and `members_this_account` if it
      also becomes unused. **Keep `invite_no_email`**: that page's placeholder also covers the
      not-signed-in case, which no label can fill. Grep each candidate across `web/src` before
      deleting (`web/src/lib/paraglide/` is generated — ignore it); a key still referenced anywhere
      stays. `scripts/check-messages.mjs` gates keys *missing* from a locale, not keys nothing
      references, so a dead key would pass silently — hence deleting them deliberately here.
- [ ] Update any fixture or render test that constructs a `User`, `OrgMember`, or `Passkey`
      (`web/src/routes/o/[org]/page.render.test.ts`, `web/src/lib/openapi.fixtures.ts`, the org page
      harness) so the new required fields are present.
- [ ] `npm run check` (which runs `scripts/check-messages.mjs` first), `npm run lint`, `npm test`,
      `npm run build` — all four green.
- [ ] Commit as `web: signal credential changes and show the account label`.

## Task 8 — Close out ⬜

**Files:** this plan, `docs/specs/2026-09-09-passkey-labels-and-signals-design.md`
**Interfaces:** consumes Tasks 1–7.

The three things below have to reach the PR body, and each of them is a thing a reviewer needs and
would not otherwise be told. They are a task rather than a memory:

- [ ] Open the PR with `Closes #59` and `Closes #60`, and name in the body:
      1. **The auth-spine behaviour change** (Task 5) — `finish_authentication` now answers
         `unknown_credential` where it answered `invalid_credentials`, with the §6 argument for why
         that is not an enumeration leak. The architect and security reviewers are the audience.
      2. **The OpenAPI correction** (Task 3) — `Me` and `SessionOpened` had fields that no longer
         exist; fixed because Task 3 opens that object anyway, not as scope creep.
      3. **`savvagent/otto-factory#64` deferred, deliberately** (Task 1) — `TeamMember.email` is
         non-null-typed over a nullable column; fixing it widens a console field and is its own
         change.
- [ ] After merge, flip the spec's `> **Status:**` to `IMPLEMENTED`, mark every task above ✅, and
      rewrite the `## Status` block. Commit as `docs: record passkey labels and signals as shipped`,
      through its own worktree and PR like any other change.
