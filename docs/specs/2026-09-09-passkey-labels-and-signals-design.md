# Passkey account labels and WebAuthn credential signals design

> **Status:** DRAFT — give every account a generated human-memorable label, use it to name the
> credential, and adopt the three WebAuthn signal methods so the browser's vault and the `passkeys`
> table stop drifting apart.

> **Implements:** `savvagent/otto-factory#59` (generated label) and `savvagent/otto-factory#60`
> (signal methods). **They ship together on purpose:** #59 fixes the label for accounts created
> after the deploy, and #60's `signalCurrentUserDetails` is the only thing that repairs the
> credentials that already exist. Either alone leaves half the population with two indistinguishable
> picker entries.

## Goal & Success Criteria

Two accounts must be distinguishable in a password manager's picker, both for keys registered from
now on and for keys already sitting in somebody's vault; and a passkey the server has deleted must
stop being offered.

- A brand-new account's registration challenge carries a `displayName` that is **not** the bare
  string `otto-factory`, and two consecutive `signup/start` calls produce different ones.
- The label is stored on `users` and is **stable** — a second `start_registration` for the same
  account names it identically, and the console shows the same words the picker shows.
- `GET /api/me/passkeys` returns each credential's base64url `credentialId`, round-tripping to the
  bytes in `passkeys.credential_id`.
- The `rpId` is readable from an unauthenticated endpoint and equals the one the challenge is built
  with.
- The console calls `signalCurrentUserDetails` after every completed ceremony and after a profile
  change, `signalAllAcceptedCredentials` after a passkey is removed, and `signalUnknownCredential`
  when a sign-in fails against a credential this server does not know — each feature-detected, each
  fire-and-forget, none able to fail the flow it is attached to.
- `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`,
  and `cd web && npm run check && npm run lint && npm test && npm run build` all pass.

## Premise corrections

The issues' premises survive contact with the repository, with three corrections worth recording
because they change what gets built:

1. **`upsert_user` creates accounts too.** #59 names only `create_unclaimed_user`
   (`crates/of-core/src/orgs.rs:239`), but `upsert_user` (`:319`) is the enterprise-OIDC
   first-login path and inserts rows as well. A label column that only one of the two writers fills
   is a column that is `NULL` for every federated account. Both insert sites generate one.
2. **`AuthError::UnknownCredential` and the `unknown_credential` code already exist**
   (`crates/of-auth/src/error.rs:40`, `crates/of-web/src/error.rs:243`), used today by
   `passkeys::remove` / `rename`. #60's fourth call site needs the console to tell "the server does
   not know this credential" from "the signature did not verify", and
   `finish_authentication` currently collapses both into `InvalidCredentials`
   (`crates/of-auth/src/passkeys.rs:263`). Reusing the existing variant is a smaller change than a
   new one — see §6, which is where the security argument for making the distinction lives.
3. **The OpenAPI document's `Me` and `SessionOpened` schemas are stale.** Both still describe
   `mustEnrollTotp` and `recoveryCodesRemaining` (`crates/of-web/src/openapi.rs:715-731`); the
   structs they document carry `shouldAddPasskey` and `passkeyCount`
   (`crates/of-web/src/routes/auth.rs:331`, `:114`) and have since TOTP was removed. §7 adds two
   fields to `Me`, which means editing that exact object. **Corrected here rather than left
   alone** — publishing a schema where two of four documented fields are fictional, while adding
   two more beside them, is shipping a document known to be wrong. Four lines, named in the plan
   and in the PR body so it is not mistaken for scope creep. `SessionOpened` is corrected in the
   same pass for the same reason; nothing else in `response_schemas()` is touched.
4. **The user handle is 16 raw UUID bytes, not the UUID's text form.**
   `start_passkey_registration` is handed `id.as_uuid()` (`passkeys.rs:126`) and webauthn-rs puts
   `user_unique_id.as_bytes()` on the wire. Every signal method takes that same handle, so the
   console needs a UUID-text → base64url-of-16-bytes conversion. Getting this wrong is invisible:
   the signal is accepted and matches nothing. §7 adds a test on each side of the wire.

## Scope

**In:**

- A new `users.label` column (migration `0022_user_label.sql`), `NOT NULL`, backfilled for existing
  rows, written by both insert sites in `of-core`.
- A label generator in `of-core` producing `adjective-noun-NN`.
- `passkeys::start_registration` naming the credential from the account rather than from a constant.
- `RegisteredKey.credentialId` on `GET /api/me/passkeys`, plus its OpenAPI schema and console type.
  This adds one column to the `SELECT` already in `passkeys::list`. `of-auth` owning SQL against the
  non-tenant `passkeys` table is a pre-existing exception to "every SQL statement lives in
  `of-core`"; widening an existing statement by a column does not deepen it, and no new query site
  is created anywhere.
- A new **public** `GET /api/auth/webauthn` returning the relying-party id.
- `finish_authentication` distinguishing an unknown credential id from a failed signature.
- Signal helpers in `web/src/lib/webauthn.ts` and their four call sites in the console.
- The console rendering the label where it currently renders a "no email" placeholder.

**Out:**

- **No change to what signup submits.** `POST /api/auth/signup/start` still takes no body. #59 is
  explicit that "take the email at signup" undoes the reason the ordering exists, and constraint 4
  of `CLAUDE.md` says the same. The label is minted for a row that has just been inserted and is
  never derived from, seeded by, or checked against anything a caller supplies.
- **No uniqueness constraint on `label`.** It is a display hint, not an identifier — nothing looks
  an account up by it. A `UNIQUE` index would turn a cosmetic collision into an insert failure on
  the signup path, and would make the column a thing an enumeration probe could ask about.
- **No relabelling of stored credentials server-side.** A credential's name is baked in by the
  challenge that created it; the only repair is the browser-side `signalCurrentUserDetails`. There
  is no migration that can fix a vault.
- **No new MCP tool and no `of-billing::classify` entry.** Nothing here touches the MCP surface, so
  `exhaustive_over` / `every_tool_has_a_price` are unaffected. Consistent with constraint 2: this is
  substrate hygiene, not workflow.
- **No mailer.** Nothing here notifies anybody of anything.
- **No client-specific behaviour.** Signal methods are feature-detected on
  `PublicKeyCredential`; there is no branch on a browser, an OS, or a password manager
  (constraint 3, applied to browsers rather than to coding agents, and to the same effect).
- No change to `docs/clients/matrix.md`: nothing a coding-agent MCP client sends or receives moves.

## Public-interface changes — all additive

Per Non-Negotiable Rule 6, named explicitly so the architect reviewer can check the claim:

| Surface | Change | Breaking? |
|---|---|---|
| Console REST | `Passkey.credentialId` added to `GET /api/me/passkeys` | No — new field |
| Console REST | `User.label` added everywhere `User` is returned (`/api/me`, `PATCH /api/me`) | No — new field |
| Console REST | `OrgMember.label` added to `GET /api/orgs/{org}/members` | No — new field |
| Console REST | `Me.credentialName` / `Me.credentialDisplayName` added to `GET /api/me` — see §7 | No — new fields |
| Console REST | `GET /api/auth/webauthn` added to `catalog.rs`, `Auth::Public`, `.returns("WebauthnConfig")` | No — new route |
| Auth errors | `finish_authentication` may now answer `unknown_credential` where it answered `invalid_credentials` | Behavioural, not structural; both codes already exist and are already documented. See §6. |
| Schema | `0022_user_label.sql` — new forward-only migration; no existing migration is edited | No |
| Config | none | — |
| MCP | none | — |

`crates/of-web/src/openapi.rs` gates this: `every_referenced_schema_is_defined` fails on a
`.returns(…)` naming a component that does not exist. The document therefore needs a **new**
`WebauthnConfig` component, and edits to four existing ones — `User` (`label`), `OrgMember`
(`label`), `Me` (`credentialName`, `credentialDisplayName`), and `Passkey` (`credentialId`) — each
added to its `properties` **and** to its `required` array, since none of the four is nullable.

## §1 The label column

`crates/of-core/migrations/0022_user_label.sql`:

- `ALTER TABLE users ADD COLUMN label TEXT;`
- Backfill every existing row with a randomly generated `adjective-noun-NN`, using small
  literal arrays inline in the migration. **The duplication with the Rust generator is deliberate
  and harmless**: the backfill runs once per cluster and never again, so the two lists cannot drift
  in any way that matters. A comment in the migration says so, or a later reader will try to
  "fix" it by importing one from the other.
- `ALTER TABLE users ALTER COLUMN label SET NOT NULL;`

`NOT NULL` rather than nullable-with-a-fallback: a nullable label means every reader carries an
`unwrap_or` whose fallback is the constant this change exists to remove, and one forgotten
`unwrap_or` puts `otto-factory` back in a picker. The constraint is what makes §2 unable to
regress.

`0007_rls.sql` is untouched: `users` is not a tenant table (it has no `org_id` and is not in
`tenant_tables`), so there is no policy to register and no cross-org negative test to add. Stated
explicitly rather than skipped.

## §2 The generator

New module `crates/of-core/src/labels.rs`, `pub fn generate() -> String`.

- Shape: `<adjective>-<noun>-<NN>`, e.g. `quiet-harbor-41`. Two digits, `10..=99`, so the label
  never reads as a truncated number and every label is the same length class.
- Word lists: 80 adjectives × 80 nouns × 90 numbers ≈ 576,000 combinations. Concrete, neutral,
  short English words; nothing that reads as a slur, a brand, or a person's name in any of the six
  console locales.
- `rand::thread_rng()` (`rand 0.8`, already a dependency of `of-core`). **Not derived from the
  UUID.** #59 requires the label to be unguessable-from-anything-a-stranger-holds; a function of the
  primary key would let anybody who learned an id recover the label and vice versa.
- Collision is acceptable and unhandled — see Scope/Out. The failure a collision causes is that two
  strangers see the same words, which is only visible if both their keys are in one vault. The
  success criterion "two consecutive `signup/start` calls differ" is therefore probabilistic: it
  fails about once in 576,000 runs, which is a tolerance worth stating rather than rediscovering as
  a flake. The generator's own unit test samples 1,000 draws and requires more than 900 distinct
  values, which catches the failure that actually matters — a generator that has collapsed toward a
  constant.
- The module's doc comment says which `labels` this is. `web/src/lib/labels.ts` already exists and
  means something else entirely (translated words for `JobStatus` and `Role`); the two never meet,
  but the names are one grep apart.

**The label stays English.** It is a handle, not prose — the same rule that keeps
`--transport http` and every wire value verbatim across the six locales (`web/README.md`,
`CLAUDE.md`). Translating it would mean the picker and the console disagreed the moment somebody
switched languages.

## §3 Writing and reading the label

- `User` gains `pub label: String`; `USER_COLS` (`crates/of-core/src/orgs.rs:121`) gains `label`.
- `create_unclaimed_user` and `upsert_user` both bind `labels::generate()`.
- `OrgMember` gains `label: String` and its query selects it — the org members page is where an
  admin picks whose passkeys to reset, and a list of rows reading "—" for every member without an
  address is exactly the ambiguity #59 is about, in the one place where picking wrong is
  destructive.
- `set_profile` does not touch it. Setting an address must not change the words somebody has already
  learned; #59 is explicit that the label survives an address being added.

## §4 Naming the credential — one function, two callers

`crates/of-auth/src/passkeys.rs`:

- Extract the relying-party name to `const RP_NAME: &str = "otto-factory";` and use it in both
  `relying_party()` and the naming below, so the two cannot disagree.
- **The composition lives in exactly one function**, `pub fn credential_names(user: &User) -> CredentialNames`,
  returning `{ name, display_name }`:
  - `name` = `email` → `name` → `label`. The address when there is one, because that is what a
    manager sorts and searches by; the label when there is not, because a blank is not an option.
  - `display_name` = `format!("{RP_NAME} · {label}")` — **always the label**, address or not. #59:
    "an account that later sets an address gets the better label without losing the one its owner
    already learned."
- `start_registration` resolves the account (existing, or the row `create_unclaimed_user` just
  returned) and calls it. The constant `"otto-factory".to_string()` fallback at `passkeys.rs:134`
  disappears. Nothing else in the ceremony changes: both webauthn-rs overrides, the
  exclude-credentials list, and the ceremony storage are untouched.

**Why a function rather than two `format!`s at the call site.** `signalCurrentUserDetails` sends the
same pair from the browser, and a browser that composed it itself would hold a second copy of the
`otto-factory · ` prefix and of the email → name → label precedence, in TypeScript, with nothing
keeping the two in sync. The failure that produces is the quiet one: the signal is accepted and
writes a label subtly unlike what a fresh registration would have written, so "repair a stale label
by signing in once" half-works and looks like it worked. The console is therefore **given** the
pair rather than deriving it — see §7 — and `credential_names` is the one place either half can
change.

## §5 Exposing the relying-party id

New handler `auth::webauthn_config`, `GET /api/auth/webauthn`, `Auth::Public`, returning
`{ "rpId": "<host>" }`.

- Sourced from `Config::rp_id()` (`crates/of-web/src/state.rs:113`) — the same value
  `relying_party` was built from, not `location.hostname`. #60 is right about why: `rp_id` may be a
  registrable parent domain of the origin, and a console that guesses it silently no-ops on exactly
  the deployments where it is not the host.
- **Public on purpose, and it discloses nothing.** The rp_id is in every challenge this server
  hands an unauthenticated caller, and it is derivable from `OF_PUBLIC_URL`, which is the address
  the caller just typed. Making it authenticated would only mean the two call sites that have no
  ceremony (`PATCH /api/me`, `DELETE /api/me/passkeys/{id}`) had to fetch it twice.
- `Config::rp_id()` returns `Option`; the server cannot have started without it — `relying_party`
  is built at boot and refuses a bad one — so `None` here is an internal error, not a `null` field.
  Answering `null` would make the console silently skip every signal.
- The console reads it **once** into module state in `web/src/lib/webauthn.ts` and caches it for the
  page's lifetime, the way `connect/+page.svelte` reads the MCP endpoint out of
  `/.well-known/oauth-protected-resource` rather than baking it in.

## §6 Telling an unknown credential from a bad signature

`finish_authentication` (`crates/of-auth/src/passkeys.rs:263`) answers `AuthError::UnknownCredential`
— code `unknown_credential` — when the presented credential id is in no `passkeys` row, and keeps
answering `InvalidCredentials` for everything downstream of that (no keys, verification failure).

**Why this is not the enumeration leak the rest of this module works to avoid.** The whole point of
the "one opaque answer" rule (`webauthn_failed`, `passkeys.rs:539`) is that a specific error tells a
forger which part of their forgery to fix. This distinction tells them nothing of the kind:

- A credential id is ≥16 bytes minted by the authenticator. It cannot be guessed, and it is not
  disclosed cross-origin, so the only parties who hold one are the authenticator that made it and
  the relying parties it was registered with. The caller asking the question already has the answer's
  subject in hand.
- It says nothing about *accounts*. `unknown_credential` is returned before any account is resolved,
  so it distinguishes no address, no user, and no org from any other.
- Everything after the lookup stays collapsed. A real credential with a bad signature, a real
  credential on an account whose keys are gone, and a replayed ceremony are all still one answer.
- The alternative is worse and is the bug #60 was filed for: with no way to tell the two apart the
  console must either signal on every failure — silently evicting a *good* passkey from somebody's
  vault after one cancelled prompt — or never signal, which is today, and leaves the
  assisted-recovery path handing an already-locked-out person a dead key first.

`note_failure` is not called on this branch and does not become called: there is still no account to
attribute an audit row to. This is the one auth-spine behaviour change in the work and is flagged to
the reviewers as such.

**Two comments in the repository currently assert the opposite, and both are part of this change.**
`credential_failures_are_one_answer` in `crates/of-web/src/error.rs` says in its doc comment that
"an unknown credential, a bad signature, and a disabled account are one answer", and
`crates/of-auth/src/passkeys.rs:266-268` says "An unknown credential. Nothing to attribute an audit
row to, and nothing to distinguish for the caller." Neither is load-bearing on a test — the array
that test iterates is `UnknownUser` / `NoPasskey` / `InvalidCredentials` / `Disabled`, and
`UnknownCredential` was never in it — so nothing fails if they are left behind. That is exactly why
they have to be named here: under the house rule that comments explain *why*, a comment that
contradicts the code is the defect, and the argument above is the text that replaces them.

## §7 The console half

**The console never composes a credential name.** `GET /api/me` gains `credentialName` and
`credentialDisplayName`, both computed by `passkeys::credential_names` (§4) — the same function the
challenge is built from. The console forwards two strings it was handed. That is what makes the
promise in #60's first table row true: a signal writes byte-for-byte what a fresh registration would
have written, and a `console_signal_matches_the_challenge` test asserts the two agree rather than
trusting that two implementations of one rule stayed in step.

`web/src/lib/webauthn.ts` gains, alongside the existing ceremony helpers:

- `userHandle(uuid: string): string` — UUID text → base64url of the 16 raw bytes, matching what
  webauthn-rs put in `user.id`. Exported so it can be tested directly.
- `signalAccount(me)` → `signalCurrentUserDetails({ rpId, userId, name, displayName })`, reading
  `credentialName` / `credentialDisplayName` straight off `/api/me`.
- `signalAcceptedCredentials(me, keys)` → `signalAllAcceptedCredentials({ rpId, userId, allAcceptedCredentialIds })`
- `signalUnknownCredential(credentialId)` → `signalUnknownCredential({ rpId, credentialId })`

Each of the three: reads the cached `rpId`, feature-detects the method on `PublicKeyCredential`,
returns immediately if it is absent, and wraps the call in a `catch` that swallows. **The swallow
gets a comment saying why** — it is the one place in this console where discarding an error is
correct, and without the comment it reads as the silent-fallback antipattern the house style
forbids.

`authenticate()`'s return type widens from `unknown` to `{ rawId: string; [k: string]: unknown }`
so the login page can name the credential it just offered when the server says it does not know it.
The object already carries the field; only the type changes.

Call sites:

| File | Trigger | Signal |
|---|---|---|
| `signup/+page.svelte` | `signup/finish` succeeded | `signalCurrentUserDetails` |
| `login/+page.svelte` | `login/finish` succeeded | `signalCurrentUserDetails` |
| `login/+page.svelte` | `login/finish` failed with `unknown_credential` | `signalUnknownCredential` |
| `claim/…/+page.svelte` | `claim/finish` succeeded | `signalCurrentUserDetails` |
| `settings/+page.svelte` | `PATCH /api/me` changed the email or the name | `signalCurrentUserDetails` |
| `settings/+page.svelte` | `DELETE /api/me/passkeys/{id}` succeeded | `signalAllAcceptedCredentials` with the remaining ids |

Every one of those is downstream of a completed ceremony or a live session cookie, which is what
keeps this off the enumeration surface (#60's first rule).

The console also renders `user.label` where it renders a no-email placeholder today
(`+layout.svelte:199`, `invite/[org]/+page.svelte:60`, `members/+page.svelte:224` and `:277`) and
in `format.ts`'s `person()`, whose `name ?? email ?? unnamed` fallback becomes `name ?? email ??
label` and can no longer be reached. No new translatable strings are introduced — the label is a
value, not prose. Keys left unreferenced by that (`nav_no_email`, `members_no_email`,
`common_unnamed_account`, and `members_this_account` if it also becomes unused) are **deleted from
all six catalogs**; `invite_no_email` stays, because that page's placeholder also covers the
not-signed-in case, which no label can fill. `scripts/check-messages.mjs` gates keys missing from a
locale, not keys nothing references, so leaving dead ones would pass silently — the decision is made
here rather than left to whoever writes the diff.

## Error Handling & Edge Cases

- **A signal method throws or rejects.** Swallowed; the flow continues. Nobody is unable to sign in
  because a password manager refused a hint.
- **`PublicKeyCredential` is absent entirely** (SSR, an old browser). `isSupported()`'s guard shape
  is reused; the helpers no-op.
- **`GET /api/auth/webauthn` fails or has not resolved yet.** The helpers no-op rather than
  guessing from `location.hostname`. A signal that silently matches nothing is worse than no signal,
  because it looks like it worked.
- **`signalAllAcceptedCredentials` with an empty list.** Legal, and the correct thing after an
  admin reset: it tells the vault this account has no accepted credentials. `passkeys::remove`
  refuses to delete the last key, so the console's own delete path never sends an empty list —
  but the type permits it and nothing rejects it.
- **A `NULL` label.** Impossible after §1's `NOT NULL`. The migration backfills before the
  constraint is applied, so a cluster with existing accounts does not fail to migrate.
- **Two accounts drawing the same label.** Cosmetic and unhandled, by design (Scope/Out).
- **A federated (OIDC) account.** `upsert_user` generates a label like any other insert, so a
  federated account is nameable in a vault too.

## Risks & Open Questions

- **The user-handle encoding is the one silent-failure risk in the change.** If the console encodes
  the UUID's text instead of its bytes, every signal is accepted and matches nothing, and no test
  that only exercises one side would notice. Mitigated by testing both halves: a Rust test asserting
  the challenge's `user.id` is base64url of the UUID's 16 bytes, and a `web/` test asserting
  `userHandle()` produces exactly that for a known UUID — the same shape as the existing test that
  reads `web/project.inlang/settings.json` to prove the two halves of i18n agree.
- **Signal support is narrow.** Chrome 132+ with Google Password Manager, and little else today.
  That is the platform where the bug actually bites (Android passkeys sync into GPM by default), and
  everywhere else is a clean no-op. Not a reason to wait.
- **`unknown_credential` from `login/finish` is a new answer** from the auth spine. §6 argues it is
  safe; the independent security review sees the diff without this document and gets to disagree.
- **End-to-end behaviour is not test-covered.** `crates/of-auth/tests/passkeys.rs` uses a software
  authenticator with no vault to inspect, and signal methods are browser affordances. Verifying that
  a stale label is actually repaired needs the CDP virtual authenticator, which is how usernameless
  sign-in was verified and is out of scope for automated tests here.
