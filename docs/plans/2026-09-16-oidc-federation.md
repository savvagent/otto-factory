# Enterprise OIDC federation — implementation plan

**Spec:** [`docs/specs/2026-09-16-oidc-federation-design.md`](../specs/2026-09-16-oidc-federation-design.md)
— read it first, in full, before starting any task. It went through three critique rounds
that found and closed a real account-takeover hole (email-based auto-linking, exploitable
via `PATCH /api/me`'s unverified email self-assignment), a login-CSRF hole on the OIDC
callback, a backwards boolean guard, and a TOCTOU race — the security-critical shape below
is not optional hardening, it is the spec.

Goal: turn `crates/of-auth`'s enterprise-OIDC surface from "schema only" into the working
feature — admin-bound IdP connections, DNS-verified claimed domains, the OIDC
authorization-code flow with a browser-bound, single-use ceremony, `enforce_sso`, and the
console UI for all of it — while keeping every existing invariant intact (tenant
isolation's two guards, SQL confined to `of-core`, no AI attribution, forward-only
migrations, no email ever sent).

## Status — 2026-09-16

Not started. All four tasks below are ⬜.

## Global Constraints

- Every SQL statement lives in `of-core`. No query in `of-auth` or `of-web`.
- `idp_connections`, `claimed_domains`, `user_identities`, and the new `sso_ceremonies` are
  deliberately outside RLS (bootstrap-before-org-known, per `CLAUDE.md`'s own list plus this
  spec's §1/§2). Every write still carries an explicit `org_id`/`id` predicate (guard 1);
  every one of these tables gets a **guard-1** cross-org test, not an `rls_scopes_*` one.
- `orgs` carries **no RLS policy at all** — it is the tenant, not tenant-scoped data
  (`0029_org_job_counters.sql`'s own comment). `set_enforce_sso` and the two lockout-guarded
  deletes protect themselves with `WHERE id = tx.org()` plus an explicit `SELECT ... FOR
UPDATE` lock — not a policy.
- **The email-linking invariant is absolute: a federated identity is never linked to a
  pre-existing `users` row by email match, under any circumstance.** Only two paths ever
  create a `user_identities` row: a brand-new user (no existing row for that email) or an
  already-authenticated session's explicit link action. Any task that touches this logic
  re-reads spec §5's callback walkthrough before writing code.
- The anonymous-ceremony new-user insert is `ON CONFLICT (lower(email)) DO NOTHING
RETURNING`, never `DO UPDATE` — `DO UPDATE` is the exact race the email-linking invariant
  above exists to close. `rows_affected() == 0` is a refusal, not an adopt-the-row signal.
- The OIDC callback requires **both** the URL-carried `state` **and** the
  `__Host-of_sso_binding` cookie to match the same ceremony row before it does anything —
  `state` alone is replayable (login CSRF). Both checks happen inside one `SELECT ... FOR
UPDATE`-then-consume transaction on the ceremony row.
- The three lockout guards (`DELETE .../connection`, `DELETE .../domains/{domain}` when
  it's the last verified one, `PUT .../sso/enforce` turning enforcement on) all take
  `SELECT 1 FROM orgs WHERE id = $1 FOR UPDATE` before evaluating "does this org still have
  a working SSO path," matching `orgs.rs`'s existing `count_owners_for_update` precedent.
  `PUT .../sso/enforce`'s guard is an AND of two **positive** conditions (has a connection
  **and** has a verified domain) — not an earlier draft's backwards AND-of-negations.
- Run `cargo fmt --all` before every Rust commit. No `unwrap()` outside tests. No AI
  self-attribution anywhere (commits, PR body, comments, docs).
- Tests need `podman compose up -d` and a `.env` with `DATABASE_URL` (`cp .env.example
.env`). `#[sqlx::test]` gives each test a fresh throwaway database; there are no database
  mocks.
- No MCP tool is added by this feature — no `of-billing::classify` entry needed, confirmed
  by spec Scope §Out.
- No new `OF_*` config variable is needed (confirmed by spec §4/§5) — the DNS resolver uses
  the system resolver, and `OF_PUBLIC_URL` already anchors the fixed `/sso/callback` path.

## File Structure

| File | Responsibility |
|---|---|
| **Create.** `crates/of-core/migrations/0031_sso_ceremonies.sql` | `sso_ceremonies` table (spec §1) |
| **Create.** `crates/of-core/src/idp.rs` | `IdpConnection` CRUD + `resolve_for_domain` (spec §2) |
| **Create.** `crates/of-core/src/domains.rs` | `ClaimedDomain` CRUD (spec §2) |
| **Create.** `crates/of-core/src/identities.rs` | `UserIdentity` CRUD + `resolve_user`/`resolve_by_email` (spec §2) |
| **Create.** `crates/of-core/src/ceremonies.rs` | `SsoCeremony` create + locked consume-by-state (spec §1/§5) |
| **Modify.** `crates/of-core/src/orgs.rs` | `set_enforce_sso`, the shared org-row lock helper, the two lockout-guarded delete paths' guard logic |
| **Modify.** `crates/of-core/src/error.rs` | `Error::DomainAlreadyClaimed`, `Error::SsoLockout { reason }`, `Error::TicketAlreadyLinked`-shaped new variants as needed |
| **Modify.** `crates/of-core/src/lib.rs` | `pub mod idp; pub mod domains; pub mod identities; pub mod ceremonies;` |
| **Modify.** `crates/of-core/tests/isolation.rs` | guard-1 cross-org tests for `idp_connections`, `claimed_domains`, `sso_ceremonies` writes, `set_enforce_sso` |
| **Create.** `crates/of-core/tests/oidc.rs` | integration tests for the new modules' CRUD + the ceremony mid-flight-reassignment/binding/email-match tests from spec §3 |
| **Create.** `crates/of-auth/src/oidc.rs` | discovery fetch, authorization URL, code exchange, `id_token` verification (spec §4) |
| **Create.** `crates/of-auth/src/dns.rs` | `verify_txt_record` via `hickory-resolver` (spec §4) |
| **Modify.** `crates/of-auth/Cargo.toml` | add `hickory-resolver` |
| **Modify.** `crates/of-auth/src/login.rs` | `enforce_sso` check on passkey login |
| **Create.** `crates/of-auth/tests/oidc.rs` | recorded-fixture tests for discovery/token-exchange/JWKS verification, no live network |
| **Modify.** `crates/of-web/src/catalog.rs` | new routes: `sso/start`, `me/sso/link/start`, `/sso/callback`, org SSO connection/domains/enforce endpoints |
| **Create.** `crates/of-web/src/routes/sso.rs` | handlers for all of the above |
| **Modify.** `crates/of-web/src/session.rs` (or wherever the session cookie helpers live) | `__Host-of_sso_binding` cookie set/read/clear helpers, mirroring `set_cookie` |
| **Create.** `crates/of-web/tests/sso.rs` | route-level tests: full anonymous flow, full authenticated-link flow, every refusal path from spec §5/Error Handling |
| **Create.** `web/src/routes/o/[org]/settings/sso/+page.svelte` | admin SSO settings page (spec §6) |
| **Modify.** `web/src/routes/login/+page.svelte` (or equivalent) | "Sign in with SSO" entry point |
| **Modify.** account-settings page (wherever the passkey list lives) | "Link SSO identity" action |
| **Modify.** `crates/of-core/migrations/0007_rls.sql`'s companion doc, `CLAUDE.md` | recorded at ship time (Task 5 / record-as-shipped), not during Tasks 1–4 |

## Task Order & Rationale

Task 1 (schema + `of-core`) has no consumer outside its own tests and is reviewable in
isolation — no running behavior changes. Task 2 (`of-auth`'s OIDC/DNS clients) needs
nothing from Task 1 directly (pure client logic, no SQL) but is sequenced second because
Task 3 needs both. Task 3 (`of-web` routes) needs Tasks 1 and 2 — it's where the ceremony
lifecycle, the ledger of lockout guards, and the actual ID-token trust decisions all meet.
Task 4 (console UI) is last because it has no test-suite gate beyond `npm run
check`/`lint`/`test`/`build` and consumes whatever Task 3 exposed.

---

## Task 1 — `of-core`: schema, CRUD, ceremonies, lockout guards ⬜

**Spec:** §1, §2, §3. Read them again before writing the migration — the `binding_hash`
and `user_id` columns and the guard-1 test list are not optional additions, they are what
closed rounds 1 and 2 of the spec's own critique.

**Files:** `crates/of-core/migrations/0031_sso_ceremonies.sql`, `crates/of-core/src/idp.rs`,
`crates/of-core/src/domains.rs`, `crates/of-core/src/identities.rs`,
`crates/of-core/src/ceremonies.rs`, `crates/of-core/src/orgs.rs`,
`crates/of-core/src/error.rs`, `crates/of-core/src/lib.rs`,
`crates/of-core/tests/isolation.rs`, `crates/of-core/tests/oidc.rs`.

**Interfaces produced:** `of_core::idp::{IdpConnection, upsert_connection, get_connection,
get_connection_secret, delete_connection, resolve_for_domain}`,
`of_core::domains::{ClaimedDomain, claim, list, delete, mark_verified}`,
`of_core::identities::{UserIdentity, resolve_user, resolve_by_email, link}`,
`of_core::ceremonies::{SsoCeremony, create, consume_by_state_hash}`,
`of_core::orgs::{set_enforce_sso, lock_for_sso_guard}`. Consumes `of_core::crypto::{Cipher,
Sealed}` (already exists).

- [ ] Write `crates/of-core/migrations/0031_sso_ceremonies.sql` exactly per spec §1:
      `sso_ceremonies` with `org_id`, `idp_connection_id`, nullable `user_id`, `state_hash`,
      `binding_hash`, plaintext `nonce`, `expires_at`, `consumed_at`, `created_at`; unique
      index on `state_hash`; index on `expires_at`. **Not** added to `0007_rls.sql`'s
      `tenant_tables` array — no RLS on this table, matching `webauthn_ceremonies`.
- [ ] Write a failing test first in `crates/of-core/tests/oidc.rs` (new file, mirrors the
      `#[sqlx::test]` shape in `crates/of-core/tests/trackers.rs`) covering: `idp::upsert_connection`
      replace-on-rebind (`ON CONFLICT (org_id) DO UPDATE`), `get_connection_secret` returning
      the sealed pair unopened, `domains::claim`'s `ON CONFLICT (domain) DO UPDATE ...
      WHERE org_id = $1` with a same-org re-claim resetting `verified_at` to `NULL`,
      `domains::mark_verified`, `identities::link`'s `ON CONFLICT (idp_connection_id,
      subject) DO NOTHING` converge-not-error behavior. Confirm it fails to compile (modules
      don't exist yet).
- [ ] Implement `crates/of-core/src/idp.rs`, `domains.rs`, `identities.rs` per spec §2's
      exact function list. `resolve_for_domain` and `resolve_user`/`resolve_by_email` are
      the only unscoped (`&Db`, not `&mut Tx`) accessors — doc-comment each one naming it as
      the single sanctioned unscoped read for its table, matching `resolve_connection_org`'s
      existing doc-comment convention in `trackers.rs`. `domains::claim`'s verification
      token is generated via the workspace `rand` crate directly (`of-core` cannot depend on
      `of-auth`'s `crypto::generate()` — that's the wrong layering direction).
      `domains::claim` checks `rows_affected() == 0` after its `ON CONFLICT ... WHERE`
      statement and returns the new `Error::DomainAlreadyClaimed` (generic message, no org
      named — spec §2) in that case.
- [ ] Add `pub mod idp; pub mod domains; pub mod identities;` to `crates/of-core/src/lib.rs`.
      Run the Task 1 test file again; confirm it compiles and the cases above pass.
- [ ] Implement `crates/of-core/src/ceremonies.rs`: `SsoCeremony` struct
      (`FromRow`/`Serialize`/`JsonSchema`, matching `TrackerBinding`'s derive list);
      `create(db: &Db, org_id, idp_connection_id, user_id: Option<UserId>, state_hash: &[u8],
      binding_hash: &[u8], nonce: &str, expires_at: DateTime<Utc>) -> Result<SsoCeremony>` —
      unscoped `&Db` insert (no RLS to satisfy, no org to pin a `Tx` to yet at ceremony
      creation in the anonymous case); `consume_by_state_hash(db: &Db, state_hash: &[u8]) ->
      Result<Option<SsoCeremony>>` — opens its own short transaction internally (this table
      has no RLS, so a bare `db.pool()` transaction is correct, not a tenant `Tx`),
      `SELECT ... FOR UPDATE WHERE state_hash = $1 AND consumed_at IS NULL AND expires_at >
      now()`, and if a row is found, `UPDATE ... SET consumed_at = now()` **before**
      returning it — the row is marked consumed whether or not the caller's subsequent
      binding-cookie check passes, per spec §5 ("marks the ceremony consumed in the same
      step regardless of outcome"). Doc-comment this ordering explicitly: the binding-cookie
      check happens in `of-web` *after* this function returns, and this function's job ends
      at "resolve and burn the ceremony," not "decide if the whole callback succeeds."
- [ ] Write a failing test for `consume_by_state_hash`: a second call with the same
      `state_hash` (simulating a replayed or racing callback) returns `None`, not the same
      row twice — confirms the `FOR UPDATE` + `consumed_at` write are atomic against a
      concurrent caller. Use two overlapping `tokio::spawn`ed calls against the same pool,
      not just two sequential calls, to actually exercise the race.
- [ ] Implement `crates/of-core/src/orgs.rs` additions: `set_enforce_sso(tx: &mut Tx,
      enforce: bool) -> Result<Org>` and a shared `lock_for_sso_guard(tx: &mut Tx) ->
      Result<()>` doing `SELECT 1 FROM orgs WHERE id = $1 FOR UPDATE` (bind `tx.org()`).
      `set_enforce_sso`, when `enforce == true`, calls `lock_for_sso_guard` first, then
      checks (within the same locked transaction) that `idp::get_connection(tx)` returns
      `Some` **and** `domains::list(tx)` contains at least one row with `verified_at.is_some()`
      — refuse with a new `Error::SsoLockout` variant naming which piece is missing if
      either check fails. When `enforce == false`, no guard — just `UPDATE orgs SET
      enforce_sso = $2 WHERE id = tx.org()`.
- [ ] Add the same `lock_for_sso_guard` + refusal shape to `idp::delete_connection` (refuse
      while `org.enforce_sso == true`, no further condition — removing the only connection
      always breaks the SSO path) and to `domains::delete` (refuse while `org.enforce_sso ==
      true` **and** the domain being deleted is verified **and** it is the org's only
      currently-verified domain — count verified domains excluding this one, inside the same
      locked transaction). Both need to read `orgs.enforce_sso`, so both take `&mut Tx` and
      call `orgs::lock_for_sso_guard` (or an equivalent org-row read) before their delete
      statement — name this cross-module call explicitly in code comments, since `of-core`
      modules calling each other is normal within the crate but worth being clear about here
      given how security-load-bearing the ordering is.
- [ ] Add `Error::DomainAlreadyClaimed` and `Error::SsoLockout { reason: String }` (or
      similarly named — match this file's existing `#[error(...)]` + `code()`/`retriable()`
      pattern) to `crates/of-core/src/error.rs`. `retriable()` is `false` for both — retrying
      the identical call cannot succeed; the caller's request itself needs to change.
- [ ] Cross-org negative tests in `crates/of-core/tests/isolation.rs`, guard-1 style (not
      `rls_scopes_*` — comment each test explaining why, matching `resolve_connection_org`'s
      existing test comment): org A binds a connection / claims a domain / sets
      `enforce_sso`; a `Tx` pinned to org B cannot see, mutate, or flip any of it. Also:
      `resolve_for_domain`/`resolve_user` cross-org resolution test (each resolves only its
      own org/connection, per spec §3). Also: `domains::claim`'s cross-org collision
      (`Error::DomainAlreadyClaimed`, org A's row untouched).
- [ ] The three remaining spec §3 tests in `crates/of-core/tests/oidc.rs`: a domain
      reassigned mid-flight cannot retarget an in-flight ceremony (ceremony's stored
      `org_id` wins over a fresh `resolve_for_domain`, tested at the `of-core` layer as "the
      ceremony row still names org A after org B claims-and-verifies the same domain" — the
      *callback's* use of this fact is Task 3's test, this task only proves the data layer
      supports it); the TOCTOU guard: two concurrent `domains::delete` calls against the
      org's two verified domains — assert the second one (whichever loses the lock race)
      sees the updated count and refuses, never both succeeding.
- [ ] `cargo test -p of-core --test isolation`, `cargo test -p of-core --test oidc`, `cargo
      test --workspace`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all
      --check`. All green before commit.
- [ ] `cargo fmt --all`, then commit: `of-core: idp connections, claimed domains, sso ceremonies, and enforce_sso lockout guards`.

## Task 2 — `of-auth`: OIDC client + DNS verification ⬜

**Spec:** §4. No dependency on Task 1's schema (pure client logic, no SQL) — sequenced
second only because Task 3 needs both.

**Files:** `crates/of-auth/src/oidc.rs`, `crates/of-auth/src/dns.rs`,
`crates/of-auth/Cargo.toml`, `crates/of-auth/tests/oidc.rs`.

**Interfaces produced:** `of_auth::oidc::{fetch_discovery, authorization_url,
exchange_code, verify_id_token, Claims, TokenResponse}`, `of_auth::dns::verify_txt_record`.
Consumes `of_core::crypto::Sealed` (for `exchange_code`'s opened-secret parameter type) and
the workspace `reqwest`/`jsonwebtoken` clients (already present) plus the new
`hickory-resolver`.

- [ ] Add `hickory-resolver` to `crates/of-auth/Cargo.toml` and the workspace
      `[workspace.dependencies]` table in the root `Cargo.toml` (pin a version; check for
      the current stable release rather than guessing a number).
- [ ] Write failing recorded-fixture tests first in `crates/of-auth/tests/oidc.rs` (no live
      network, matching `of-trackers`'s existing testing convention for its GitHub/JIRA
      clients — use a local mock HTTP server, e.g. the same crate/approach `of-trackers`'s
      fixture tests already use, check `crates/of-trackers/tests/` for the pattern before
      picking a new one): discovery document fetch and field extraction; authorization URL
      construction (exact query string shape per spec §4); code exchange happy path;
      `id_token` verification — valid signature/issuer/audience/nonce accepted, wrong
      issuer/audience/nonce/expired-token each rejected with a distinct, named failure.
- [ ] Implement `oidc.rs` per spec §4's exact function list. `verify_id_token` fetches and
      short-lived-in-memory-caches the JWKS document (keyed by `jwks_uri` — a simple
      `tokio::sync::RwLock<HashMap<...>>` or similar, no need for a full cache crate).
      **Does not** itself enforce `email_verified` — that check is the caller's
      responsibility per spec §4's own explanation of why it's a caller-side check, not
      baked into this function. Leave a doc comment on `Claims` pointing at spec §4/§5 so a
      future reader doesn't "helpfully" move the check into this function and lose the
      caller-side placement's reasoning.
- [ ] Implement `dns.rs`'s `verify_txt_record`: resolves `_otto-factory-verify.{domain}` TXT
      records, returns `Ok(true)` iff any record equals
      `format!("otto-factory-verify={expected_token}")`, `Ok(false)` on NXDOMAIN/timeout/no
      records, `Err` only on a genuine resolver-transport failure (per spec §4's exact
      Ok/Err split — do not conflate "not verified yet" with "operator problem").
- [ ] `cargo test -p of-auth --test oidc`, `cargo test --workspace`, `cargo clippy
      --all-targets -- -D warnings`, `cargo fmt --all --check`.
- [ ] `cargo fmt --all`, then commit: `of-auth: OIDC discovery/token-exchange/id_token verification and DNS TXT record checks`.

## Task 3 — `of-web`: routes, ceremony lifecycle, `enforce_sso` at login ⬜

**Spec:** §5 in full — this is the task where the callback's exact check ordering
(`state` → `binding_hash` → `email_verified` → domain-still-resolves →
ceremony-kind-specific linking) and the three lockout guards' wiring all come together. Do
not summarize or reorder the callback's steps from memory; re-read §5 while implementing
this task's handler.

**Files:** `crates/of-web/src/catalog.rs`, `crates/of-web/src/routes/sso.rs` (new),
`crates/of-web/src/session.rs` (or equivalent — wherever `set_cookie`/`COOKIE_NAME` live),
`crates/of-auth/src/login.rs` (the `enforce_sso` check), `crates/of-web/tests/sso.rs` (new).

**Interfaces produced:** the routes listed in spec §5. Consumes Task 1's `of_core::idp`/
`domains`/`identities`/`ceremonies`/`orgs` and Task 2's `of_auth::oidc`/`dns`.

- [ ] Add a `binding` cookie helper alongside the existing `set_cookie`/`COOKIE_NAME`
      (`__Host-of_sso_binding`, `HttpOnly; Secure; Path=/; SameSite=Lax`, `Max-Age` matching
      the ceremony's 10-minute TTL) and a `clear_binding_cookie()` helper (`Max-Age=0`) for
      the callback response, per the spec's Risks & Open Questions note on not leaving a
      stale cookie behind.
- [ ] Write failing route-level tests first in `crates/of-web/tests/sso.rs` (mirrors
      `crates/of-web/tests/oauth_http.rs`'s or the passkey ceremony tests' shape — a real
      router, a real throwaway Postgres, no mocks): the full anonymous sign-in flow against
      a fixture IdP (reuse Task 2's fixture-server approach) ending in a session cookie;
      `sso_not_configured` for an unclaimed domain; the full authenticated-link flow;
      email-match refusal on the authenticated-link path; email-collision refusal on the
      anonymous path (pre-seed a `users` row, assert no session opens and no
      `user_identities` row is created); `email_verified: false` refusal; a replayed
      `state` (second callback with the same `state_hash`) refusal; a mismatched/missing
      binding-cookie refusal (drop or corrupt the cookie before the second request); domain
      reassigned mid-flight still resolves to the original org; each of the three lockout
      guards' refusal (delete the only connection while `enforce_sso = true`, delete the
      last verified domain while `enforce_sso = true`, enable `enforce_sso` with no
      connection / no verified domain — three separate cases); a passkey login refused for
      a member of an `enforce_sso` org. Confirm the test file fails to compile (handlers
      don't exist yet).
- [ ] Implement `POST /api/auth/sso/start` and `POST /api/me/sso/link/start` per spec §5:
      resolve domain via `idp::resolve_for_domain`, generate `state`/`binding`/`nonce` (via
      `of_auth::crypto::generate()` for the two hashed values, a plain random string for
      `nonce`), `ceremonies::create`, set the binding cookie, build the authorization URL
      via `of_auth::oidc::authorization_url`, return `{ redirect_url }`. The link-start
      variant additionally requires an authenticated session and a non-null caller email.
- [ ] Implement `GET /sso/callback` per spec §5's full walkthrough, in order: hash incoming
      `state` → `ceremonies::consume_by_state_hash` → generic refusal on `None` → hash the
      binding cookie and compare to the consumed ceremony's `binding_hash` → generic
      refusal on mismatch (the ceremony is already consumed at this point, so no separate
      "burn it" step is needed here) → open a `Tx` pinned to `ceremony.org_id` →
      `get_connection`/`get_connection_secret` → open the secret → `exchange_code` →
      `verify_id_token` → enforce `email_verified` → enforce `resolve_for_domain` against
      `ceremony.org_id` still resolving → `identities::resolve_user` → branch on
      `ceremony.user_id`: authenticated path enforces the email-match-to-caller check before
      `identities::link`; anonymous path does the `DO NOTHING RETURNING` insert (this is
      `of_core::identities`'s job — confirm Task 1 exposed it as such, not left as raw SQL
      in this handler, which would violate "every SQL statement lives in `of-core`") and
      refuses on `rows_affected() == 0` or `resolve_by_email` returning `Some`. On success:
      commit, `of_auth::sessions::create`, `session::set_cookie`, clear the binding cookie,
      `302` redirect. On any refusal: no commit, no session, clear the binding cookie
      anyway (nothing left to protect once the ceremony's outcome is decided).
- [ ] Implement the admin-only connection/domain/enforce endpoints (`PUT`/`DELETE
.../connection`, `POST`/`GET`/`POST .../verify`/`DELETE .../domains[/{domain}]`, `PUT
.../enforce`) per spec §5, each behind `OrgCtx::require_admin()`. `PUT .../connection`
      calls `of_auth::oidc::fetch_discovery` before `idp::upsert_connection` and rejects
      (with the OIDC-side error surfaced) on a discovery fetch failure. `POST .../verify`
      reads the token in a short `Tx`, releases it, calls `dns::verify_txt_record` outside
      any `Tx`, then opens a second short `Tx` for `mark_verified` only on `true`.
- [ ] Add the `enforce_sso` check to the passkey login path (`of_auth::login::with_passkey`
      or its `of-web` caller, per spec §5's "Passkey login enforcement" note — locate the
      exact resolved-`user_id`-before-session-mint point first, do not add a second lookup
      elsewhere): before minting a session, check whether `user_id` is in `org_members` for
      any org with `enforce_sso = true`; refuse with a message pointing at
      `/api/auth/sso/start` if so.
- [ ] Add every new route to `crates/of-web/src/catalog.rs` with a summary/description
      (the router and OpenAPI document are built from this one list — a route not in the
      catalog is not reachable, on purpose).
- [ ] Run the Task 3 test file; confirm every case passes. `cargo test -p of-web`, `cargo
      test --workspace`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all
      --check`.
- [ ] `cargo fmt --all`, then commit: `of-web: enterprise SSO routes — connection/domain admin, sign-in and link ceremonies, enforce_sso login gate`.

## Task 4 — `web/`: console UI ⬜

**Spec:** §6.

**Files:** `web/src/routes/o/[org]/settings/sso/+page.svelte` (new), the login page (add
the SSO entry point), the account-settings page wherever the passkey list already lives
(add the "Link SSO identity" action), plus whatever shared API client module this console
already uses for typed fetch calls (follow the existing pattern rather than inventing one
— check how the tracker-connection settings page at
`web/src/routes/o/[org]/settings/trackers` — or wherever that landed — calls its API).

- [ ] Admin SSO settings page: connection form (issuer/client id/secret — secret field
      write-only, cleared after submit, never populated from a `GET` response), domains
      list (add, see the exact TXT record name/value the API returned, verify, remove) each
      showing verified/pending status, `enforce_sso` toggle. Every lockout-refusal (`400`
      from any of the three guarded endpoints) surfaces its message inline rather than a
      generic toast — the whole point of naming the reason server-side is for a human to
      read it here.
- [ ] Login page: collapsed "Sign in with SSO" section below the passkey button — email
      field, `POST /api/auth/sso/start`, `window.location.assign(redirect_url)` on success;
      `sso_not_configured` surfaces inline, pointing back at the passkey button.
- [ ] Account settings: "Link SSO identity" action — `POST /api/me/sso/link/start`, same
      navigate-on-success pattern.
- [ ] `npm run check && npm run lint && npm test && npm run build`. All green.
- [ ] Commit: `web: enterprise SSO settings page, sign-in entry point, account-linking action`.

---

## Out-of-band reminders (Phase 5 of the otto-factory-development skill, not a task step)

- **Database migrations changed** (`0031_sso_ceremonies.sql`): confirm a fresh cluster
  applies cleanly (`podman compose down -v && podman compose up -d`, `cargo test -p
of-core`), confirm `0007_rls.sql` still runs last, confirm `sso_ceremonies` does **not**
  appear in its `tenant_tables` array.
- **No config surface change** — vacuously satisfied, state it explicitly rather than
  skipping the check.
- **`web/` changed** — full `npm run check && npm run lint && npm test && npm run build`
  gate, plus a manual pass in a real browser (the run skill's Phase 5 step 15) exercising
  both the admin SSO settings page and an actual sign-in-with-SSO round trip against a real
  or fixture OIDC provider, since none of the automated gates drive a real browser through
  a third-party redirect.
- **Record-as-shipped** (separate PR, per the skill): flip this spec's `Status` to
  IMPLEMENTED, mark this plan's tasks ✅, and update `CLAUDE.md`'s RLS-exempt-auth-tables
  list to include `sso_ceremonies` (spec's own Risks & Open Questions note).
