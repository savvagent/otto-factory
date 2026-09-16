# Enterprise OIDC federation design

> **Status:** DRAFT — schema exists (`crates/of-core/migrations/0005_auth.sql`); nothing else
> does. This spec covers the whole feature in one task.
> **Implements:** the "Enterprises — OIDC federation" paragraph of
> `docs/specs/2026-09-01-otto-factory-design.md`'s Authentication section.
> **Depends on:** milestone 1 (passkey auth, sessions, `OrgCtx`) — merged. The promoted
> `of_core::crypto::Cipher`/`Sealed` primitive from `docs/specs/2026-09-03-df-trackers-design.md` §4.

## Goal & Success Criteria

An org admin binds an IdP (issuer, client id, client secret), claims one or more email
domains, and proves control of each via a DNS TXT record. Once a domain is verified, a
person signing in with an email at that domain is redirected to the bound IdP instead of
the passkey ceremony; on return, their IdP-asserted identity is pinned to a user row (by
subject, and — on first use — resolved/created by verified email) and a normal console
session is opened. An admin can additionally set `enforce_sso`, which refuses passkey
login for anyone who is a member of that org.

Success:

- `idp_connections` (one per org) and `claimed_domains` (globally unique per domain) are
  manageable by an org admin from the console: bind/replace/remove an IdP, claim a domain,
  see its verification instructions, verify it, remove it.
- A claimed-and-verified domain's login redirects to that org's IdP; the round trip ends
  with a normal `__Host-of_session` cookie, the same session primitive passkey login uses.
- `enforce_sso` is settable by an org admin and is enforced at passkey login: a member of
  an `enforce_sso` org cannot complete a passkey sign-in.
- First-time federated sign-in links to an existing user row by verified email when one
  exists, otherwise creates one, and in both cases ensures the person is an `org_members`
  row in the federating org.
- `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all
  --check`, and the `web/` gates are green. Every new tenant-adjacent write carries an
  explicit `org_id` predicate; every table this spec adds or touches that is deliberately
  outside RLS gets a cross-org negative test proving guard 1 alone still holds.

## Premise corrections

- The design doc's own sentence — "Admins can set `enforce_sso`, disabling individual
  passkey login for that org's members" — already answers the one design question that
  would otherwise be invented here: enforcement is **org-membership-scoped**, not
  domain-match-scoped. A contractor invited into an `enforce_sso` org with a personal email
  address is still refused passkey login; they must authenticate through the org's IdP (or,
  practically, through whichever claimed domain covers them, or be removed from the org).
  This spec does not add a second, narrower enforcement rule.
- `orgs.enforce_sso boolean NOT NULL DEFAULT false` already exists on the `Org` struct
  (`crates/of-core/src/orgs.rs:67`) and is already selected by every org read
  (`ORG_COLS`, `orgs.rs:134`). Nothing sets it yet. This spec adds the setter and the one
  enforcement check; it does not touch the column's shape.
- `idp_connections`, `claimed_domains`, and `user_identities` (`0005_auth.sql`) were laid
  down at milestone 1 and never edited by the later `0009_no_email` / `0010_passkeys`
  migrations. They are exactly as milestone 1 left them. `CLAUDE.md`'s migration-safety
  section already lists `idp_connections` and `claimed_domains` among the tables that
  "carry no `*_tenant_isolation` policy at all, deliberately" — the same bootstrap
  reasoning `0007_rls.sql`'s comment gives for `access_tokens`: authentication has to
  resolve who's signing in, and which org's IdP that implies, **before** an org is known,
  so pinning these tables to `current_org()` would make sign-in impossible. This spec does
  not invent that decision; it is already made and documented. `user_identities` carries no
  `org_id` column at all (it is keyed through `idp_connection_id`) and is in the same
  class — global identity-resolution data, not tenant data.
- `idp_connections.client_secret_ct` / `client_secret_nonce` (two `bytea` columns) predate
  the trackers work's single-`TEXT` `encode_sealed`/`decode_sealed` convention
  (`crates/of-core/src/trackers.rs`). They don't need it: `Cipher::seal` already returns
  `Sealed { ciphertext: Vec<u8>, nonce: Vec<u8> }` — two fields, and these are two columns.
  This spec writes `ciphertext`/`nonce` straight into `client_secret_ct`/`client_secret_nonce`
  with no concatenation step, which is what `of-core::crypto`'s doc comment says a caller
  with two columns available should do.
- `of-auth` already depends on `of-core` (confirmed: `crates/of-auth/Cargo.toml`), so this
  spec calls `of_core::crypto::Cipher` directly — no new inter-crate dependency edge.
- `jsonwebtoken` (JWKS/id_token signature verification) and `reqwest` (rustls-tls, json —
  discovery document, token endpoint) are already workspace dependencies used today by
  `of-trackers` for GitHub App JWT signing and the GitHub/JIRA HTTP clients. Neither needs
  adding. **`hickory-resolver` is a new dependency** — nothing in this workspace does a DNS
  TXT lookup today, and that is exactly what proving domain control requires.

## Scope

**In:**

- `idp_connections` CRUD (one per org — binding a second IdP replaces the first, matching
  `tracker_connections`' `UNIQUE (org_id, provider)` precedent, here `UNIQUE (org_id)`).
- `claimed_domains` CRUD: claim (mint a verification token), list, verify (DNS TXT lookup),
  unclaim. A domain is globally unique — claimed by at most one org at a time.
- `enforce_sso` setter, admin-only, and its one enforcement point at passkey login.
- The OIDC authorization-code flow: `GET /.well-known/openid-configuration` discovery
  (cached in `idp_connections.discovery`), authorization-URL construction, code exchange,
  `id_token` verification (signature via the IdP's JWKS, `iss`/`aud`/`exp`/`nonce`), and
  `email`/`email_verified`/`sub` claim extraction.
- A server-rendered callback endpoint that mints the same session primitive passkey login
  uses (`of_auth::sessions::create` + the `__Host-of_session` cookie).
- First-use identity pinning to `user_identities`, with account linking by verified email
  and automatic `org_members` provisioning into the federating org.
- Console UI: an org-admin "SSO" settings page (bind/replace/remove IdP, claim/verify/remove
  domains, toggle `enforce_sso`), and a "sign in with SSO" entry point on the login page.

**Out:**

- SAML. Not asked for; the design doc says "enterprise SSO is OIDC-only in v1."
- SCIM directory sync, just-in-time role assignment beyond the default `member` role, or
  any IdP-driven deprovisioning (an IdP-side account disable does not remove `org_members`
  in this spec — that is a real gap, named in Risks & Open Questions, and is its own
  feature).
- More than one IdP per org. `idp_connections` is `UNIQUE (org_id)`; multiple IdPs for one
  org (e.g. after a merger) is a v2 concern, matching `tracker_connections`' identical v1
  simplification.
- Any change to the passkey ceremony itself, PKCE/OAuth 2.1 (Layer 1, MCP client auth), or
  the MCP tool surface. This is entirely Layer 2 (who the human is) and entirely
  console-facing; it touches no MCP tool and needs no `of-billing::classify` entry.
- A generic "any IdP, any protocol" abstraction customers could reuse for something else.
  Substrate-not-workflow does not apply here the way it does to the job queue — *this*
  feature is itself the substrate decision the design doc already made ("enterprises
  federate through OIDC"); there is no narrower mechanism to extract into a customer skill,
  since a customer's own skill has no way to mint this server's session cookie.

## Assumptions

- **Binding an IdP, claiming/verifying a domain, and toggling `enforce_sso` are all
  `admin`-role actions** (`OrgCtx::require_admin()`), not owner-only. `CLAUDE.md`'s role
  table gives `admin` explicit authority over "connections", and the tracker-connection
  binding work already treated an org-level provider connection as an admin action
  (`docs/plans/2026-09-03-df-trackers.md` Task 6: "admin-only, `OrgCtx::require_admin`").
  `enforce_sso` is a property of finishing that same setup, not a separate, more sensitive
  decision — it does not touch billing or org deletion, the two things `CLAUDE.md`
  reserves for `owner`.
- **First-time federated sign-in auto-provisions `org_members`, role `member`.** The
  alternative — federated sign-in only *authenticates*, and an admin must separately invite
  the same person — would make domain claiming pointless for its stated purpose
  (self-service enterprise onboarding: "anyone who can authenticate through our IdP is one
  of us" is the entire value proposition of claiming a domain). This mirrors how invitation
  acceptance already inserts an `org_members` row; the IdP assertion (email domain match,
  once the domain is verified) stands in for the invitation.
- **Account linking by email requires `email_verified: true` in the ID token or userinfo
  response**, never linked on an unverified claim. Requesting the `email` scope is not
  sufficient on its own — some IdPs return an email without asserting it. Without this
  check, a misconfigured or malicious IdP could assert an arbitrary email and take over an
  existing passkey account through the federation path. This is the one place this feature
  could open an account-takeover hole if skipped, and it is the single highest-priority
  check in the whole design.
- **The OIDC callback also re-checks that the verified email's domain still matches a
  claimed-and-verified domain routed to *this* `idp_connection`** — not just "some org's
  IdP accepted this person." A user typing `alice@acme.com` at the SSO entry point starts a
  ceremony scoped to Acme's `idp_connection`; if Acme's IdP somehow returns
  `bob@other-corp.com` (a misconfigured IdP, or a user who authenticated a different
  account than the one they intended), the callback must refuse rather than silently
  provisioning `bob` into Acme's org. This is what the ceremony record's stored `org_id` is
  for — the check is against the ceremony's own org, not a fresh domain lookup, so a
  domain reassigned mid-flow can't retarget an in-flight ceremony either.
- **The callback endpoint is a server-rendered `of-web` GET route** (`/sso/callback`),
  returning a redirect with the session cookie already set — not a SvelteKit console page
  that round-trips the authorization code through a second `POST`. This differs from the
  tracker-connection console flow's client-side callback page
  (`web/src/routes/trackers/callback`, which exists because that flow is an *admin binding
  an org-level connection*, not a browser session — nothing there needs to set a cookie).
  Minting `__Host-of_session` is exactly the kind of browser-facing, cookie-setting
  operation `of-web` already owns server-side for `/oauth/authorize`'s consent flow
  (`CLAUDE.md`: "`/oauth/authorize` is a browser surface that needs the console's session
  cookie, which is why it lives here"). Routing the code through client JS first would gain
  nothing and would briefly expose the authorization code to the page's JS context for no
  reason.
- **The redirect URI is one fixed, global path** (`{OF_PUBLIC_URL}/sso/callback`), not
  per-org. Which org's IdP a given callback belongs to is carried by the `state` parameter
  (resolved server-side via the ceremony record it references), the same shape the tracker
  console flow already uses for the same reason: an IdP's registered redirect URI is one
  static string, and it cannot contain a per-org path segment
  (`web/src/lib/trackerState.ts`'s documented rationale). No new `OF_*` config is needed —
  `OF_PUBLIC_URL` already exists and already anchors every other absolute URL this server
  issues.
- **A new `sso_ceremonies` table, not a reuse of `webauthn_ceremonies`.** The two serve
  parallel but distinct purposes (a WebAuthn challenge vs. an OIDC `state`/`nonce` pair) and
  forcing one shape to fit both would either add unused columns to the WebAuthn table or
  make the OIDC row stash unrelated data in `webauthn_ceremonies.state`'s opaque `jsonb`.
  A separate table with exactly the columns this flow needs is simpler to reason about, and
  costs one migration.
- **The `state` value is a single-use, bearer-shaped, short-TTL token and is hashed at
  rest**, reusing `of_auth::crypto::generate()`/`hash()`/`verify()` exactly as `sessions`,
  `access_tokens`, and `account_claims` already do for every other single-use or bearer
  token in this codebase. The `nonce` is not a bearer credential (it is sent to the IdP as
  a plaintext query parameter and its only job is anti-replay on the returned `id_token`),
  so it is stored in plaintext — there is no confidentiality property to protect, only
  integrity, and the ceremony row itself already provides that.
- **`idp_connections.discovery` (existing `jsonb NOT NULL DEFAULT '{}'`) caches the fetched
  `/.well-known/openid-configuration` document indefinitely, refreshed on next use whenever
  it's missing the `authorization_endpoint`/`token_endpoint`/`jwks_uri` keys the flow
  needs** — not on a TTL. IdP discovery documents change essentially never in practice
  (rotating `authorization_endpoint` would break every existing integration for that IdP),
  and a background refresh job is unrequested machinery for a document that's for all
  practical purposes static. Re-binding the connection (replacing issuer/client id/secret)
  always re-fetches.
- **DNS verification is a synchronous, admin-initiated `POST`, not a background poller.**
  The admin adds the TXT record, then clicks "Verify" — a poller checking every claimed
  domain on a schedule is unrequested machinery, and the manual `curl`-shaped action matches
  how every comparable product (Slack, Notion, Vercel) verifies domain ownership.

## §1 Schema

**No change to any existing table or column.** `0005_auth.sql`'s `idp_connections`,
`claimed_domains`, and `user_identities` are used exactly as laid down. One new,
additive-only migration:

```sql
-- crates/of-core/migrations/0031_sso_ceremonies.sql

-- State held between "redirect to the IdP" and "the IdP redirects back". Parallel to
-- webauthn_ceremonies (0010_passkeys.sql) but a distinct shape: an OIDC state/nonce pair,
-- not a serialized WebAuthn challenge. No org_id-based RLS — like webauthn_ceremonies, this
-- is short-lived correlation data read by state alone, before any session exists to pin an
-- org to. org_id is denormalized onto the row (not re-derived from the domain a second time
-- at callback) so a domain reassigned mid-flow can't retarget an in-flight ceremony.
CREATE TABLE sso_ceremonies (
  id                uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  org_id            uuid NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
  idp_connection_id uuid NOT NULL REFERENCES idp_connections (id) ON DELETE CASCADE,
  -- Single-use, bearer-shaped, hashed at rest — same convention as sessions/access_tokens.
  state_hash        bytea NOT NULL,
  -- Anti-replay on the returned id_token. Not a bearer credential (sent to the IdP as a
  -- plaintext query parameter); no confidentiality property to protect here.
  nonce             text NOT NULL,
  expires_at        timestamptz NOT NULL,
  consumed_at       timestamptz,
  created_at        timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX sso_ceremonies_state_key ON sso_ceremonies (state_hash);
CREATE INDEX sso_ceremonies_expiry_idx ON sso_ceremonies (expires_at);
```

`sso_ceremonies` is deliberately **not** added to `0007_rls.sql`'s `tenant_tables` array —
same reasoning as `webauthn_ceremonies`: it must be readable by `state_hash` alone, before
any org is known, and it holds no secret worth an RLS policy protecting (the state token
itself is what's confidential, and it's hashed).

## §2 `of_core` — `idp`, `domains`, `identities` modules

New modules, same shape as `of_core::trackers` (`Tx`-scoped CRUD for writes and
org-known reads; a small number of deliberately unscoped `&Db` functions for the
bootstrap-before-org-is-known reads, each with a doc comment naming it as the one place
that table is read without an org pinned — per the `resolve_connection_org` precedent).

**`of_core::idp`** (mirrors `of_core::trackers`'s `TrackerConnection` shape):

- `IdpConnection { id, org_id, issuer, client_id, discovery: serde_json::Value,
created_at }` — `client_secret_ct`/`client_secret_nonce` are **not** on this struct; they
  never leave `of-core` as plaintext-adjacent bytes. A separate accessor returns the sealed
  pair only to the one caller (the OIDC token-exchange step) that needs to open it.
- `upsert_connection(tx, issuer, client_id, secret: Sealed, discovery: Value) ->
Result<IdpConnection>` — `ON CONFLICT (org_id) DO UPDATE`, admin rebinding replaces.
- `get_connection(tx) -> Result<Option<IdpConnection>>`, `delete_connection(tx) ->
Result<()>` (cascades `sso_ceremonies` and, via `user_identities.idp_connection_id ON
  DELETE CASCADE`, existing identity pins — a removed connection's users fall back to
  needing a fresh federated sign-in or a passkey, which is correct: the org no longer
  vouches for that IdP).
- `get_connection_secret(tx) -> Result<Option<Sealed>>` — the one function that returns
  `client_secret_ct`/`client_secret_nonce`, used only by the token-exchange step.
- `resolve_for_domain(db: &Db, domain: &str) -> Result<Option<(OrgId, IdpConnection)>>` —
  **the one unscoped accessor**, doc-commented exactly like `resolve_connection_org`: joins
  `claimed_domains` (`verified_at IS NOT NULL`) to `idp_connections` on `org_id`. Never add
  a second unscoped accessor for `idp_connections`.

**`of_core::domains`**:

- `ClaimedDomain { org_id, domain, verification_token, verified_at, created_at }`.
- `claim(tx, domain, verification_token) -> Result<ClaimedDomain>` — `INSERT ... ON
CONFLICT (domain) DO UPDATE SET verification_token = EXCLUDED.verification_token,
  verified_at = NULL WHERE claimed_domains.org_id = $1` (the caller's own org, from `tx`).
  **Check `rows_affected() == 0` after the statement**: if another org already holds the
  domain, the `WHERE` clause blocks the update, the `ON CONFLICT` target still matched, and
  zero rows come back — that is `Error::DomainAlreadyClaimed` (message deliberately does
  not name which org holds it, matching the JIRA-site-registration precedent in
  `docs/specs/2026-09-03-df-trackers-design.md`'s Risks section for what's an acceptable,
  bounded disclosure vs. not — *this* case is a full account/organization identity, so it
  gets a fully generic message, not even a "yes it's claimed" confirmation beyond the error
  itself, which the caller already knows since they're the one being refused).
- `list(tx) -> Result<Vec<ClaimedDomain>>`, `delete(tx, domain) -> Result<()>` (the `WHERE
org_id = $1 AND domain = $2` predicate is what stops an admin unclaiming a domain their
  own org doesn't hold — this is a normal `Tx`-scoped, guard-1-only write, same as every
  other write here).
- `mark_verified(tx, domain) -> Result<ClaimedDomain>` — `UPDATE ... SET verified_at =
now() WHERE org_id = $1 AND domain = $2`. Called only after the DNS lookup (done outside
  any `Tx`, see §4) succeeds.

**`of_core::identities`**:

- `UserIdentity { id, user_id, idp_connection_id, subject, created_at }`.
- `resolve_user(db: &Db, idp_connection_id, subject) -> Result<Option<UserId>>` — unscoped,
  same bootstrap class as `resolve_for_domain`; `user_identities` carries no `org_id` at
  all, so there is no tenant boundary to pin here in the first place.
- `link(db: &Db, user_id, idp_connection_id, subject) -> Result<UserIdentity>` — first-use
  pinning, `ON CONFLICT (idp_connection_id, subject) DO NOTHING` then re-read (a duplicate
  callback for the same ceremony, or a race between two tabs, converges rather than errors
  — same tolerance `create_from_ticket` already uses for the analogous webhook-redelivery
  case in the trackers design).

**`of_core::orgs`** gains one function: `set_enforce_sso(tx, enforce: bool) ->
Result<Org>` — a normal `Tx`-scoped, RLS-covered write (`orgs` is already a tenant table).

## §3 Cross-org / bootstrap coverage tests

- `idp_connections`, `claimed_domains`: **not** `rls_scopes_*`-style tests (there is no
  policy to probe — these tables are deliberately outside RLS, per `CLAUDE.md`'s own list).
  Instead, a guard-1 test per `CLAUDE.md`'s "ordinary cross-org tests pass on guard 1
  alone" framing: org A binds a connection / claims a domain inside its own `Tx`; a `Tx`
  pinned to org B cannot see or mutate it through the normal `get_connection`/`list`/
  `delete` accessors (each of which threads `tx.org()` into its `WHERE`). Comment in the
  test explaining why this isn't an RLS test, matching the convention
  `resolve_connection_org`'s test already set.
- `resolve_for_domain` / `resolve_user`: a `#[sqlx::test]` upserting data for two different
  orgs/connections and asserting each resolves only its own — the same shape
  `resolve_connection_org`'s cross-org test already uses.
- `domains::claim`'s cross-org collision: org A claims `acme.com`; org B's `claim` for the
  same domain returns `Error::DomainAlreadyClaimed` and leaves org A's row untouched.
- `set_enforce_sso`: ordinary `rls_scopes_*` coverage already exists for `orgs` generally;
  no new policy needed (this is a new column write on an already-RLS-governed table), but a
  functional test that org B's `Tx` cannot flip org A's flag (guard 1: the `UPDATE`'s
  `WHERE org_id = $1` already prevents it; assert it).

## §4 `of_auth` — the OIDC client and DNS verification

New modules `crates/of-auth/src/oidc.rs` and `crates/of-auth/src/dns.rs`, matching
`of-trackers`'s existing split between pure client logic (no SQL — `of-core` owns every
statement) and the crate that calls out over the network.

**`oidc.rs`**:

- `fetch_discovery(issuer: &str) -> Result<Value>` — `GET {issuer}/.well-known/openid-configuration`
  via the workspace `reqwest` client, JSON body stored verbatim into `idp_connections.discovery`.
- `authorization_url(discovery, client_id, redirect_uri, state, nonce) -> Result<Url>` —
  reads `authorization_endpoint` from the cached discovery document, builds
  `response_type=code&scope=openid+email+profile&client_id=...&redirect_uri=...&state=...&nonce=...`.
- `exchange_code(discovery, client_id, client_secret: &str, code, redirect_uri) ->
Result<TokenResponse>` — `POST` to `token_endpoint`, `grant_type=authorization_code`.
  `client_secret` is the **opened** plaintext, held only for the duration of this call —
  never logged, never returned.
- `verify_id_token(discovery, client_id, id_token: &str, expected_nonce: &str) ->
Result<Claims>` — fetches (and short-lived-in-memory-caches, keyed by `jwks_uri`) the JWKS
  document, verifies the `id_token`'s signature with `jsonwebtoken` against the matching
  key (`kid`), and checks `iss == discovery.issuer`, `aud` contains `client_id`, `exp` not
  passed, `nonce == expected_nonce`. `Claims { sub, email: Option<String>, email_verified:
  Option<bool> }` — exactly the fields this design consumes, nothing else deserialized.
- **The one hard security rule in this file, called out because it's the highest-priority
  check in the whole design (see Assumptions):** the caller of `verify_id_token` (the
  `of-web` callback handler) refuses to proceed unless `claims.email.is_some() &&
  claims.email_verified == Some(true)`. This is a caller-side check, not baked into
  `verify_id_token` itself, because `verify_id_token`'s job is "is this token genuinely
  from this issuer, for this request" — a separate, equally-required question from "does
  its content satisfy this feature's trust requirement."

**`dns.rs`**:

- `verify_txt_record(domain: &str, expected_token: &str) -> Result<bool>` — resolves
  `_otto-factory-verify.{domain}` `TXT` records via `hickory-resolver` (new dependency —
  nothing in this workspace does DNS resolution today), returns `true` iff any returned
  record's content equals `format!("otto-factory-verify={expected_token}")`. A resolution
  failure (NXDOMAIN, timeout, no records) is `Ok(false)`, not an error — "not yet verified"
  and "temporarily unreachable" both mean "not verified now, admin can retry," and
  conflating them into an error the admin can't act on differently would be noise. A
  genuine transport failure (the resolver itself unreachable) is the one case that's a real
  `Error`, since that's an operator-facing problem, not the domain's.

**No new `OF_*` config.** The DNS resolver uses the system's configured resolver (no
override needed — the same trust boundary as any other outbound network call this server
already makes). No credential of otto-factory's own is needed to do a TXT lookup.

## §5 `of_web` — routes

New unauthenticated routes in `catalog.rs`, matching the shape of the existing passkey
ceremony endpoints and `/oauth/authorize`:

- `POST /api/auth/sso/start` `{ email: String }` → normalizes, extracts the domain,
  `of_core::idp::resolve_for_domain`. No match → `404`-shaped `sso_not_configured` error
  ("no SSO configured for this address — sign in with a passkey instead"). Match → mint an
  `sso_ceremonies` row (`state` via `of_auth::crypto::generate()`, hashed for storage;
  `nonce` via the same generator, stored plain), build the authorization URL via
  `of_auth::oidc::authorization_url`, return `{ redirect_url: String }` for the console to
  navigate to. **Accepted, bounded disclosure** (documented in Risks & Open Questions,
  parallel to the trackers spec's JIRA-site-registration timing note): this reveals whether
  a domain has SSO configured, not whether any specific account exists — the same class of
  leak "Continue with SSO" flows in comparable products (Slack, Notion) accept by design.
- `GET /sso/callback?code=...&state=...` → looks up `sso_ceremonies` by hashing the
  incoming `state` and comparing (never a raw equality query against the hash column — same
  constant-time-verify convention `of_auth::crypto::verify` already provides), checks
  `expires_at` and `consumed_at IS NULL`, marks it consumed in the same step (single-use).
  No match / expired / already consumed → a plain error page, **not** a `404`
  (this endpoint is never queried for an org's existence the way `OrgCtx` is; it's a
  malformed-or-replayed-request page). On match: open a normal `Tx` pinned to the
  ceremony's stored `org_id`, `get_connection` + `get_connection_secret`, open the sealed
  secret, `exchange_code`, `verify_id_token` with the ceremony's stored `nonce`, enforce the
  `email_verified` rule (§4), and enforce that the verified email's domain still resolves
  (via a fresh `resolve_for_domain` check against the *ceremony's own* `org_id`, per the
  Assumptions bullet on re-validating at callback time) to this same org — a mismatch is a
  refusal, not a fallback to a different org. Then: `identities::resolve_user` by
  `(idp_connection_id, subject)`; if none, resolve or create a `users` row by
  `lower(email)` (reusing the existing case-insensitive unique index) and `identities::link`
  it. Ensure `org_members` contains `(org_id, user_id, role: member)` (insert-if-absent,
  matching invitation acceptance's existing tolerance for "already a member"). Commit.
  `of_auth::sessions::create` + `session::set_cookie`, `302` to the console's org (or
  general) landing page.
- `PUT /api/orgs/{org}/sso/connection` (`OrgCtx::require_admin`) `{ issuer, client_id,
client_secret }` → `fetch_discovery`, seal the secret, `upsert_connection`. Rejects (with
  the OIDC-side error surfaced verbatim, per this repo's "errors are written for an LLM/an
  operator that has never read the docs" convention — here the reader is a human admin, but
  the same honesty standard applies) if discovery fetch fails: a connection is never saved
  half-configured.
- `DELETE /api/orgs/{org}/sso/connection` (`require_admin`).
- `POST /api/orgs/{org}/sso/domains` `{ domain }` (`require_admin`) → generates a
  verification token, `domains::claim`, returns the domain row **and** the exact TXT record
  name/value the console renders as setup instructions.
- `GET /api/orgs/{org}/sso/domains` (`require_admin`, or any member — read-only, no secret
  in this response; matches the read-vs-write admin split elsewhere in `of-web`. **Decision:
  `require_admin`** for consistency with the rest of this settings surface — nothing about
  it needs member-level visibility and keeping the whole SSO settings page one authorization
  level avoids a split-permission page).
- `POST /api/orgs/{org}/sso/domains/{domain}/verify` (`require_admin`) → reads the domain
  row's `verification_token` in a short read-only `Tx`, releases it, calls
  `dns::verify_txt_record` **outside any `Tx`** (a DNS lookup has no place holding a
  Postgres transaction open, same rule the tracker sync engine's outbound writes already
  follow), then — only on `true` — opens a second short `Tx` to `mark_verified`. `false` →
  `200` with `{ verified: false }`, not an error; the admin just hasn't propagated DNS yet.
- `DELETE /api/orgs/{org}/sso/domains/{domain}` (`require_admin`).
- `PUT /api/orgs/{org}/sso/enforce` `{ enforce_sso: bool }` (`require_admin`) →
  `set_enforce_sso`. Refuses (`400`, naming the reason) when turning it on and the org has
  no *verified* claimed domain and no bound `idp_connection` — turning on enforcement with
  no way for any member to complete an IdP sign-in would lock every passkey-only member out
  with no path back in, which is a self-inflicted lockout this endpoint can and should
  refuse to create.

**Passkey login enforcement.** `of_auth::login::with_passkey` (or its `of-web` caller —
whichever already has the resolved `user_id` in hand before minting a session) gains one
check: is `user_id` in `org_members` for any org with `enforce_sso = true`? If so, refuse
with a clear message pointing at `/api/auth/sso/start` instead of opening a session. This
is a read against `org_members`/`orgs` joined on the caller's own resolved identity, not a
new unscoped accessor — the user is already known at this point, the same as every other
post-ceremony passkey login step.

## §6 Console UI

- **Login page**: below the existing passkey button, a collapsed "Sign in with SSO"
  section — an email field, `POST /api/auth/sso/start`, `window.location.assign(redirect_url)`
  on success; the `sso_not_configured` error surfaces inline pointing back at the passkey
  button.
- **`web/src/routes/o/[org]/settings/sso/+page.svelte`** (`OrgCtx`-equivalent client-side
  admin gate, matching the existing tracker-connection settings page's structure): a
  connection form (issuer/client id/secret — the secret field is write-only, never
  round-tripped back from the API), a domains list (add, see TXT instructions, verify,
  remove) each showing `verified` / `pending` status, and the `enforce_sso` toggle with the
  lockout-refusal error surfaced inline if the API refuses it.
- **`/sso/callback`** needs **no console page** — it's a server response, per §5/Assumptions.

## Error Handling & Edge Cases

- Claiming a domain another org already holds: `Error::DomainAlreadyClaimed`, generic
  message, no confirmation of which org (§2).
- `enforce_sso` turned on with no working IdP path for the org: refused at the API layer
  (§5), not silently accepted and discovered later as a lockout.
- OIDC callback with a mismatched/expired/reused `state`: generic error page, no org/domain
  information disclosed (this is the unauthenticated bootstrap path; treat it with the same
  care as a bad bearer token).
- `email_verified` false or absent: refused, pointing the person at their org's own sign-in
  instructions (whatever the IdP's own login support says) rather than at otto-factory,
  since otto-factory isn't what's misconfigured.
- Discovery document missing `authorization_endpoint`/`token_endpoint`/`jwks_uri`: refused
  at bind time (`PUT .../sso/connection`), never silently deferred to first login attempt.
- A user's existing passkey account gets linked to a federated identity: no passkey is
  removed or disabled by this — the account can still sign in either way, unless its org
  turns on `enforce_sso`, in which case only the federated path works for that org's
  purposes (the passkey itself is untouched and still works for any other org the person
  belongs to that isn't `enforce_sso`).
- Deleting an `idp_connection` cascades `sso_ceremonies` (in-flight ceremonies for it become
  meaningless) and — via `user_identities.idp_connection_id ON DELETE CASCADE` — existing
  identity pins; those users keep their `users`/`org_members` rows, they simply have no
  federated identity to sign in with until the org re-binds an IdP or removes `enforce_sso`.

## Risks & Open Questions

- **Domain-SSO-configured disclosure via `POST /api/auth/sso/start`** is an accepted,
  bounded leak (a domain either has SSO configured or it doesn't; no account-level
  information), matching the precedent already accepted for JIRA site registration in the
  trackers design. Revisit only if a future threat model treats this as sensitive.
- **No IdP-side deprovisioning.** Removing someone from the IdP's directory does not remove
  their `org_members` row here — this spec has no webhook or SCIM channel to learn about
  that. An admin must remove the member from the console directly, same as today for a
  passkey-only member. Named as an explicit non-goal (Scope §Out), not silently assumed
  away.
- **One IdP per org (v1 simplification)**, same class of limitation `tracker_connections`
  already accepted for "one connection per provider per org." Revisit if a real customer
  needs two.
- **`idp_connections.discovery` never expires on its own.** If an IdP genuinely rotates its
  discovery document's endpoints without the otto-factory admin re-binding, sign-in would
  break silently until someone notices and re-binds. Accepted per the Assumptions bullet on
  refresh-on-miss vs. TTL; revisit only if this is observed in practice.
- **`sso_ceremonies` rows accumulate.** No sweep/GC job is specified here — matching
  `webauthn_ceremonies`, which also has none today (only an index on `expires_at` for a
  future one). Out of scope for this spec; a follow-up housekeeping task, not a correctness
  gap (expired/consumed rows are simply never matched again).
