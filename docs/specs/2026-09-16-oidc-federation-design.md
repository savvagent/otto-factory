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
the passkey ceremony; on return, their IdP-asserted identity is pinned to a user row by
subject — on first use, a brand-new user row when no account exists for that email, never
a silent link to a pre-existing one — and a normal console session is opened. An admin can
additionally set `enforce_sso`, which refuses passkey login for anyone who is a member of
that org.

Success:

- `idp_connections` (one per org) and `claimed_domains` (globally unique per domain) are
  manageable by an org admin from the console: bind/replace/remove an IdP, claim a domain,
  see its verification instructions, verify it, remove it.
- A claimed-and-verified domain's login redirects to that org's IdP; the round trip ends
  with a normal `__Host-of_session` cookie, the same session primitive passkey login uses.
- `enforce_sso` is settable by an org admin and is enforced at passkey login: a member of
  an `enforce_sso` org cannot complete a passkey sign-in.
- First-time federated sign-in creates a new user row when no existing row holds that
  email, and in that case provisions `org_members` in the federating org. When a row
  already holds that email, sign-in is refused rather than silently linked (see
  Assumptions) — linking is only ever done from an authenticated session.
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
- First-use identity pinning to `user_identities` — new-account creation with automatic
  `org_members` provisioning for a previously-unseen email, or explicit linking from an
  already-authenticated session; never a silent link to a pre-existing, unauthenticated
  account by email match (see Assumptions).
- Console UI: an org-admin "SSO" settings page (bind/replace/remove IdP, claim/verify/remove
  domains, toggle `enforce_sso`), a "sign in with SSO" entry point on the login page, and an
  authenticated "link SSO identity" action in account settings.

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
- **`email_verified: true` in the ID token or userinfo response is required before the
  callback trusts an email claim at all**, never trusted on an unverified claim. Requesting
  the `email` scope is not sufficient on its own — some IdPs return an email without
  asserting it. This defends against a misconfigured or malicious *IdP*.
- **A malicious *local user* is a separate, more concrete threat this codebase already
  makes exploitable, and `email_verified` alone does not close it.** `PATCH /api/me`
  (`crates/of-core/src/orgs.rs`'s `Db::set_profile`) lets any signed-in passkey account set
  its own `users.email` to an arbitrary address with **zero ownership proof** — there is no
  mail-based verification anywhere in this product (`CLAUDE.md`: "no email is ever sent").
  Left unaddressed, this is a pre-hijacking attack with no email-based fix available: an
  attacker registers a passkey account, sets its email to `alice@acme.com` before Acme ever
  binds SSO, and waits. When Acme later federates and the real Alice authenticates through
  Acme's IdP with a genuinely verified `alice@acme.com`, resolving by email match would find
  the attacker's row, link the federated identity to it, and grant *the attacker's own
  passkey-controlled account* membership in Acme's org under Alice's name — full
  impersonation, and the attacker never touches the IdP at all.

  **The fix: a federated identity is never linked to a pre-existing user row by email match,
  under any circumstance — including an existing `org_members` row for the same org** (an
  "already a member of this org" carve-out was considered and rejected: the same `PATCH
  /api/me` hole lets an attacker who *legitimately* joined the org under their own email
  later rewrite it to `alice@acme.com` too, so org membership is not evidence of email
  ownership either). The only two paths that ever create a `user_identities` row are:

  1. **No existing `users` row holds the verified email** — safe to create a new user row
     and auto-provision `org_members` (the clean bootstrap case; nothing to hijack).
  2. **An already-authenticated session explicitly starts a "link my SSO identity" ceremony**
     (a new `POST /api/me/sso/link/start`, session-cookie-gated) — the ceremony carries the
     caller's own already-known `user_id`, so the callback links to *that* row by construction,
     never by an email lookup. Proof of ownership here comes from holding a working session
     (a passkey the person already controls), not from the email claim.

  Case 3 — a verified federated email collides with an existing `users` row that has
  *no* linked identity and the caller has no session — is refused outright: **no session is
  opened**, with a message directing the person to sign in with their existing credentials
  first and link SSO from account settings. This is a real, accepted UX cost (an org
  federating for the first time can't have existing passkey-holding members "just work" on
  their first SSO attempt; each must explicitly link once) traded for closing an account-
  takeover path this product has no other way to close, given it never sends mail. Recorded
  in Risks & Open Questions, not silently accepted.
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
- **The `state` value is a single-use, bearer-shaped token, hashed at rest, with a 10-minute
  TTL** — matching the "ten single-use codes"-era links' own TTL convention recorded in this
  repo's history for the last comparable single-use token. Generated via
  `of_auth::crypto::generate()`, and looked up at the callback the same way `sessions` and
  `access_tokens` already resolve *their* incoming bearer values: hash the incoming `state`
  with `of_auth::crypto::hash()` and look it up with a plain `WHERE state_hash = $1` (a
  deterministic-hash equality lookup is exactly what those two precedents already do — this
  spec's earlier draft called this a "constant-time-verify" case and it is not; corrected
  here to match the actual precedent instead of inventing a stricter one). The `nonce` is
  not a bearer credential (it is sent to the IdP as a plaintext query parameter and its only
  job is anti-replay on the returned `id_token`), so it is stored in plaintext — there is no
  confidentiality property to protect, only integrity, and the ceremony row itself already
  provides that.
- **`sso_ceremonies` gains a nullable `user_id`**, set only by the authenticated
  "link my SSO identity" path (§ above) and left `NULL` for the anonymous "sign in with
  SSO" path — see §1/§5 for the two ceremonies' distinct handling at callback time.
- **The callback needs a second secret binding it to the browser that started the ceremony
  — the `state` parameter alone is not enough, and an earlier draft of this spec omitted
  this entirely.** `state` travels in a URL (the redirect from the IdP), which means it
  necessarily also travels through browser history, referrer headers, and — critically —
  anything that captures and replays that exact URL. Without a second check, an attacker
  can legitimately start and complete a ceremony as themselves, capture the resulting
  `?code=...&state=...` callback URL *before* letting it load, and get a victim to load it
  instead (a chat link, an `<img>` tag — `CLAUDE.md` already treats exactly this class of
  URL, unfurled by a link preview, as hostile for the console's invitation links). Since the
  ceremony's `org_id`/`user_id` are fixed server-side at creation time and nothing checks
  them against the browser making the callback request, that victim's browser would end up
  holding a session cookie for **the attacker's own account** (classic login CSRF) — or, for
  the authenticated-link ceremony, would permanently link the attacker's real IdP identity
  onto whatever account the ceremony names. This is also this design's own instance of the
  rule `CLAUDE.md` already states for every other browser-facing endpoint in this codebase:
  "no credential is ever spent on a `GET`" — this callback mints a session on an
  unauthenticated `GET` with no confirmation, which needs the same browser-binding
  discipline OAuth clients are expected to apply to `state` (RFC 6749 §10.12), not a
  narrower defense. **Fix:** ceremony-start (`sso/start` and `sso/link/start`) additionally
  sets a short-lived, `HttpOnly`, `Secure`, `Path=/`, `SameSite=Lax` cookie —
  `__Host-of_sso_binding` — carrying a second random value distinct from `state`; the
  ceremony row stores only its hash (`binding_hash`, generated/hashed via the same
  `of_auth::crypto::generate()`/`hash()` pair as `state`). The callback reads this cookie
  from the incoming request and requires it to match the ceremony's stored `binding_hash`
  *in addition to* the URL-carried `state` resolving a ceremony at all — a captured-and-
  replayed callback URL loaded in a different browser has no way to also present the
  matching cookie. A missing or mismatched binding cookie is refused with the same generic
  error as an invalid `state`, and — like a valid callback — still consumes the ceremony
  (all of this happens inside one `SELECT ... FOR UPDATE`-then-consume `Tx` on the ceremony
  row, so a mismatched attempt cannot be retried against the same ceremony either).
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
  -- Set only by the authenticated "link my SSO identity" path (an existing session
  -- proving account ownership); NULL for the anonymous "sign in with SSO" path. The
  -- callback links to this user_id directly when set, and never resolves by email at
  -- all in that case — see the design spec's Assumptions on why an email match alone
  -- must never establish or extend account access.
  user_id           uuid REFERENCES users (id) ON DELETE CASCADE,
  -- Single-use, bearer-shaped, hashed at rest — same convention as sessions/access_tokens.
  state_hash        bytea NOT NULL,
  -- A second secret, distinct from state_hash, carried by an HttpOnly cookie set at
  -- ceremony-start and checked at callback. state travels in a URL and can be captured
  -- and replayed by an attacker into a victim's browser (login CSRF); this is what proves
  -- the browser completing the callback is the same one that started the ceremony. See
  -- the design spec's Assumptions for the full threat this closes.
  binding_hash      bytea NOT NULL,
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
  all, so there is no tenant boundary to pin here in the first place. Used on every
  callback, first thing: a returning federated user (this pair already linked) always takes
  this path regardless of which ceremony kind they came through.
- `resolve_by_email(db: &Db, email: &str) -> Result<Option<UserId>>` — unscoped, used
  **only** by the anonymous-ceremony callback path to decide "create a new user" (`None`)
  vs. "refuse, this email already belongs to someone" (`Some`). **Never used to choose whom
  to link an identity to** — see the Assumptions bullet on why an email match must never by
  itself establish or extend access. Its one caller treats `Some(_)` strictly as a refusal
  signal, not as a target to link.
- `link(db: &Db, user_id, idp_connection_id, subject) -> Result<UserIdentity>` — first-use
  pinning, `ON CONFLICT (idp_connection_id, subject) DO NOTHING` then re-read (a duplicate
  callback for the same ceremony, or a race between two tabs, converges rather than errors
  — same tolerance `create_from_ticket` already uses for the analogous webhook-redelivery
  case in the trackers design). Called with a `user_id` the caller already resolved through
  one of the two sanctioned paths in §5 — `link` itself does no email resolution.

**`of_core::orgs`** gains one function: `set_enforce_sso(tx, enforce: bool) -> Result<Org>`.
**Correction to an earlier draft of this section:** `orgs` is **not** an RLS-covered tenant
table — it is the tenant itself. It is absent from `0007_rls.sql`'s `tenant_tables` array,
and `crates/of-core/migrations/0029_org_job_counters.sql` says so explicitly: *"orgs is not
one of 0007_rls.sql's tenant_tables — it is the tenant, not tenant-scoped data, and carries
no org-scoping RLS policy."* `set_enforce_sso`'s only protection is guard 1: `UPDATE orgs
SET enforce_sso = $2 WHERE id = tx.org()` — the caller cannot name a different org's row
because `Tx` is pinned to the caller's own org id and `orgs.id` (not `org_id`) is the match
column. This puts `orgs` writes in the same guard-1-only bucket as `idp_connections`/
`claimed_domains`, not the RLS-covered bucket — §3's test for this needs to prove guard 1
holds, the same as it does for every other table in this spec.

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
- `set_enforce_sso`: **`orgs` carries no RLS policy at all** (§2's correction) — this is a
  guard-1-only test, same bucket as `idp_connections`/`claimed_domains`, not a "policy
  already covers it" case. Assert org B's `Tx` cannot flip org A's flag (the `UPDATE`'s
  `WHERE id = tx.org()` is the only thing preventing it).
- A domain reassigned mid-flight cannot retarget an in-flight ceremony: start a ceremony
  for org A (which denormalizes `org_id` onto the `sso_ceremonies` row), then have org B
  successfully claim-and-verify the same domain before the ceremony's callback runs;
  assert the callback still resolves against org A (the ceremony's stored `org_id`), not a
  fresh `resolve_for_domain` lookup that would now return org B.
- A callback whose `state` resolves a real ceremony but whose `__Host-of_sso_binding`
  cookie is missing or does not match `binding_hash` is refused, and the ceremony is
  consumed by the attempt (cannot be retried against the same ceremony with a corrected
  cookie).
- The authenticated-link ceremony's email-match guard: a caller with `user_id = U` (email
  `alice@acme.com`) whose IdP callback returns a domain-matching but different email
  (`bob@acme.com`) is refused, and no `user_identities` row is created for `U`.

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

- `POST /api/auth/sso/start` `{ email: String }` (unauthenticated) → normalizes, extracts
  the domain, `of_core::idp::resolve_for_domain`. No match → `sso_not_configured` error
  ("no SSO configured for this address — sign in with a passkey instead"). Match → mint an
  `sso_ceremonies` row with `user_id = NULL` (the anonymous-ceremony kind — `state` via
  `of_auth::crypto::generate()`, hashed for storage; a second, independent value generated
  the same way for `binding_hash`; `nonce` via the same generator, stored plain), set the
  `__Host-of_sso_binding` cookie (see Assumptions) to the plaintext binding value, build the
  authorization URL via `of_auth::oidc::authorization_url`, return `{ redirect_url: String
  }` for the console to navigate to. **Accepted, bounded disclosure** (documented in Risks &
  Open Questions, parallel to the trackers spec's JIRA-site-registration timing note): this
  reveals whether a domain has SSO configured, not whether any specific account exists — the
  same class of leak "Continue with SSO" flows in comparable products (Slack, Notion) accept
  by design.
- `POST /api/me/sso/link/start` (session-cookie-gated — the caller must already be signed
  in) → the caller's own org memberships are irrelevant here; this only needs the caller's
  already-resolved `user_id`. If the caller's account has no verified email at all yet
  (`users.email IS NULL`), refuse — there is nothing to route to an IdP by. Otherwise, same
  domain resolution and binding-cookie setup as `sso/start`, but the minted `sso_ceremonies`
  row sets `user_id = Some(caller's user_id)` (the authenticated-ceremony kind). Same
  response shape.
- `GET /sso/callback?code=...&state=...` (unauthenticated) → hashes the incoming `state`
  with `of_auth::crypto::hash()` and looks it up with `SELECT ... FOR UPDATE WHERE
state_hash = $1` (see Assumptions — this is a deterministic-hash equality lookup, the same
  shape `sessions`/`access_tokens` already use for their own incoming bearer values; the
  `FOR UPDATE` is what makes "check, then mark consumed" atomic against a second concurrent
  callback for the same ceremony), checks `expires_at` and `consumed_at IS NULL`, **hashes
  the `__Host-of_sso_binding` cookie from the request and requires it to match the
  ceremony's stored `binding_hash`** (see Assumptions — this is the browser-binding check
  that closes the login-CSRF path a bare `state` check leaves open), marks the ceremony
  consumed in the same step regardless of outcome (single-use, and a failed binding check
  cannot be retried against the same ceremony). No `state` match / expired / already
  consumed / binding mismatch → the same generic error page, **not** a `404` (this endpoint
  is never queried for an org's existence the way `OrgCtx` is; it's a
  malformed-or-replayed-request page — and deliberately not distinguishing *which* of these
  four checks failed, since that distinction is only useful to an attacker probing the
  endpoint). On match: open a normal `Tx` pinned to the ceremony's stored `org_id`,
  `get_connection` + `get_connection_secret`, open the sealed secret, `exchange_code`,
  `verify_id_token` with the ceremony's stored `nonce`, enforce the `email_verified` rule
  (§4), and enforce that the verified email's domain still resolves (via a fresh
  `resolve_for_domain` check against the *ceremony's own* `org_id`) to this same org — a
  mismatch is a refusal, not a fallback to a different org. Then, **the two ceremony kinds
  diverge**:
  - `identities::resolve_user(idp_connection_id, subject)` first, regardless of kind — a
    returning federated user (already linked, from either kind of ceremony originally) always
    resolves here and skips everything below.
  - **Authenticated-ceremony (`ceremony.user_id.is_some()`)**: **first requires the verified
    email to case-insensitively match the ceremony's own `user_id`'s current
    `users.email`** — not just the same domain. Without this, a shared browser or a stale
    IdP session (a kiosk, a leftover login from testing a different account) could silently
    link a *different* person's real corporate identity onto the caller's account with no
    confirmation step; matching domain alone was not enough to rule that out. A mismatch
    refuses with a message naming the discrepancy (this is an authenticated caller, so
    naming it is not an enumeration risk the way the anonymous path's errors are). Once that
    holds: if the `(idp_connection_id, subject)` pair is unlinked, `identities::link(ceremony.user_id,
    ...)` directly — no email-based *account resolution* happens here, only this one
    equality check against the account already known from the session. If that exact pair
    is already linked to a *different* user (someone else's federated identity), refuse:
    this ceremony's caller cannot steal another account's IdP link by replaying a callback
    against it.
  - **Anonymous-ceremony (`ceremony.user_id.is_none()`)**: `identities::resolve_by_email`. `None`
    → `INSERT ... ON CONFLICT (lower(email)) DO NOTHING RETURNING` for the verified email
    (**not** `DO UPDATE` — a third review round caught that `DO UPDATE ... RETURNING`, this
    spec's own earlier wording, is `Db::upsert_user`'s "create or converge" shape, whose
    whole point is to hand back whatever row already holds that email even when it wasn't
    the caller's own insert that put it there; under this codebase's default `READ
    COMMITTED` isolation, a `PATCH /api/me` landing in the window between
    `resolve_by_email` returning `None` and this `INSERT` running would let `DO UPDATE`
    silently converge onto — and then link this federated identity to — a row created by
    exactly the race the "never link by email match" rule exists to prevent. `DO NOTHING`
    is the correct shape here, matching `domains::claim` (§2) and `repos.rs`'s existing
    "insert, and treat zero rows affected as a race loss to refuse, never a hint to adopt
    the other row" pattern). If `rows_affected() == 0`, treat identically to
    `resolve_by_email` returning `Some(_)` — refuse, no session opened; a concurrent writer
    won the same race `resolve_by_email` would have caught a moment earlier, and adopting
    that row would be the same mistake `DO UPDATE` almost was.

    If the insert succeeds, `identities::link` the new row, ensure
    `org_members` contains `(org_id, new_user_id, role: member)` (insert-if-absent). `Some(_)`
    → **refuse. No session is opened.** Error message: "an account already exists for this
    email — sign in with your existing credentials, then link SSO from account settings."
    (See Assumptions for why this refusal, not a silent link, is the correct behavior.)
  - Whichever branch succeeds: commit, `of_auth::sessions::create` + `session::set_cookie`,
    `302` to the console's org (or general) landing page. The refusal branch commits nothing
    and opens no session.
- `PUT /api/orgs/{org}/sso/connection` (`OrgCtx::require_admin`) `{ issuer, client_id,
client_secret }` → `fetch_discovery`, seal the secret, `upsert_connection`. Rejects (with
  the OIDC-side error surfaced verbatim, per this repo's "errors are written for an LLM/an
  operator that has never read the docs" convention — here the reader is a human admin, but
  the same honesty standard applies) if discovery fetch fails: a connection is never saved
  half-configured.
- `DELETE /api/orgs/{org}/sso/connection` (`require_admin`) → locks the org row (see the
  `PUT .../sso/enforce` entry below for why), refuses (`400`, naming the reason) while
  `orgs.enforce_sso = true`: removing the org's only IdP while every member's passkey login
  is refused would lock every member — including the admin issuing this call — out
  entirely, with no path back in. `enforce_sso` must be turned off first.
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
- `DELETE /api/orgs/{org}/sso/domains/{domain}` (`require_admin`) → locks the org row (see
  `PUT .../sso/enforce` below), refuses (`400`, same reasoning as the connection delete
  above) when `orgs.enforce_sso = true` **and** this is the org's only currently-verified
  domain — removing the last routable domain while enforcement is on would strand every
  member who isn't already linked with no way to reach the anonymous sign-in path either
  (the authenticated link-start path is also unreachable for anyone not already signed in,
  which under `enforce_sso` is everyone who hasn't federated yet). Deleting a non-last
  verified domain, or an unverified one, proceeds. The org-row lock is what stops two
  concurrent deletes (starting from two verified domains) from each reading "not last" and
  both succeeding — a plain read-then-delete without it is a real TOCTOU race, not a
  theoretical one, since this is exactly the two-admin-at-once scenario the console makes
  easy to trigger by accident.
- `PUT /api/orgs/{org}/sso/enforce` `{ enforce_sso: bool }` (`require_admin`) →
  `set_enforce_sso`. **Corrected in this revision — an earlier draft's guard was an AND of
  two negated conditions and only refused when both a connection and a verified domain were
  simultaneously absent, which is backwards: `resolve_for_domain` is an inner join, so
  *either* piece missing alone already makes SSO sign-in unreachable.** Refuses (`400`,
  naming the reason) turning it on **unless the org has both** a bound `idp_connection`
  **and** at least one verified `claimed_domains` row — turning on enforcement with no way
  for any member to complete an IdP sign-in would lock every passkey-only member out with no
  path back in, which is a self-inflicted lockout this endpoint can and should refuse to
  create. This is the same guard `DELETE .../connection` and `DELETE .../domains/{domain}`
  enforce from the other direction — together they mean `enforce_sso = true` can never
  coexist with "no working IdP path," checked at every edge that could produce that state,
  not just the one that turns the flag on. **All three of these checks (both deletes, and
  this enable path) take `SELECT 1 FROM orgs WHERE id = $1 FOR UPDATE` at the top of their
  transaction before evaluating "does this org still have a working path"** — the same
  locked-read-then-write discipline `orgs.rs`'s existing `count_owners_for_update` already
  uses for its own concurrent-admin-action race. Without it, two admins concurrently
  deleting the org's two verified domains (or one deleting the sole connection while another
  enables enforcement) can each read "still safe" before either commits, and both succeed —
  landing the org in the locked-out state this whole guard exists to prevent. Locking the
  `orgs` row is what serializes all three mutations against each other cleanly, since they
  all answer the same underlying question about the same org.

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
  lockout-refusal error surfaced inline if the API refuses it. The connection- and
  domain-delete actions surface the same lockout-refusal shape.
- **An authenticated "Link SSO identity" action** in account settings (wherever the
  passkey list already lives) — calls `POST /api/me/sso/link/start`, same
  `window.location.assign(redirect_url)` pattern as the login page. This is the UI for the
  authenticated-ceremony path (§5), and the only self-service way an existing passkey
  account gains a federated identity once its email collides with something the anonymous
  path refuses.
- **`/sso/callback`** needs **no console page** — it's a server response, per §5/Assumptions.

## Error Handling & Edge Cases

- Claiming a domain another org already holds: `Error::DomainAlreadyClaimed`, generic
  message, no confirmation of which org (§2).
- `enforce_sso` turned on with no working IdP path for the org: refused at the API layer
  (§5), not silently accepted and discovered later as a lockout.
- OIDC callback with a mismatched/expired/reused `state`, or a missing/mismatched
  `__Host-of_sso_binding` cookie: generic error page, no org/domain information disclosed
  and no distinction surfaced between the four possible causes (this is the unauthenticated
  bootstrap path; treat it with the same care as a bad bearer token). A cleared or absent
  cookie (private browsing, a cross-browser copy-paste of the callback URL) fails the same
  way a stolen/replayed one does — restart from `sso/start`.
- Authenticated-link callback whose verified email doesn't match the caller's own stored
  `users.email`: refused, naming the mismatch (safe to be specific here — the caller is
  already authenticated, this isn't the anonymous enumeration surface).
- `email_verified` false or absent: refused, pointing the person at their org's own sign-in
  instructions (whatever the IdP's own login support says) rather than at otto-factory,
  since otto-factory isn't what's misconfigured.
- Discovery document missing `authorization_endpoint`/`token_endpoint`/`jwks_uri`: refused
  at bind time (`PUT .../sso/connection`), never silently deferred to first login attempt.
- A user's existing passkey account explicitly links a federated identity (the
  authenticated path): no passkey is removed or disabled by this — the account can still
  sign in either way, unless its org turns on `enforce_sso`, in which case only the
  federated path works for that org's purposes (the passkey itself is untouched and still
  works for any other org the person belongs to that isn't `enforce_sso`).
- A verified federated email collides with an existing, unlinked `users` row (the anonymous
  path): refused outright, no session opened. Directed at the authenticated link path
  instead. This includes the case where that existing row is already a member of the
  federating org via an earlier invitation — membership does not substitute for the
  ownership proof a session provides (see Assumptions).
- A pending invitation for an email specified a role above `member` (e.g. `admin`): the
  anonymous federated-signup path still only ever provisions `role: member` for a brand-new
  user row. The invitation is not consulted or reconciled by this feature. Accepted, minor
  gap — it under-provisions rather than over-provisions, and an admin can promote the new
  member afterward the same way they would anyone else.
- Deleting an `idp_connection` cascades `sso_ceremonies` (in-flight ceremonies for it become
  meaningless) and — via `user_identities.idp_connection_id ON DELETE CASCADE` — existing
  identity pins; those users keep their `users`/`org_members` rows, they simply have no
  federated identity to sign in with until the org re-binds an IdP or removes `enforce_sso`.
  Refused outright while `enforce_sso = true` (§5) rather than left to produce this state
  for every member simultaneously.

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
- **No pre-existing-account collision is ever resolved automatically**, by design (see
  Assumptions) — an org's first wave of federated sign-ins for people who already hold
  passkey accounts under matching emails each need one explicit "link SSO identity" action
  from an authenticated session, rather than "just working" on first SSO attempt. This is
  the accepted cost of closing the `PATCH /api/me` pre-hijacking path with no mail-based
  verification available anywhere in this product. If that endpoint ever gains real
  ownership verification for email changes, this restriction could be relaxed in a later
  spec — not assumed here.
- **Which exact `of_core` function owns each `SELECT 1 FROM orgs WHERE id = $1 FOR UPDATE`
  lockout guard is left to the plan, not pinned here.** §5 describes the *behavior* three
  endpoints need (lock the org row, then check "does a working SSO path still exist" before
  writing); §2 names only `set_enforce_sso` as a new `of_core::orgs` function. The natural
  homes are inside `set_enforce_sso` itself for the enable path, and a small shared helper
  (or logic inside `idp::delete_connection`/`domains::delete`) for the two deletes — the
  plan should name the exact split so an implementer isn't inventing three slightly
  different shapes for the same guard. Not a design gap, just not yet pinned to a signature.
- **The `__Host-of_sso_binding` cookie isn't explicitly cleared by the callback response.**
  It becomes useless the moment its ceremony is consumed (single-use), so this is hygiene,
  not a security gap — but the callback's `302` response should still clear it (`Max-Age=0`)
  so a stale cookie doesn't linger in the browser past its purpose.
- **`CLAUDE.md`'s list of RLS-exempt auth tables should be updated to include
  `sso_ceremonies`** once this ships, alongside `idp_connections`/`claimed_domains` — a
  documentation follow-up for the record-as-shipped step, not a code change.
