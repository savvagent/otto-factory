-- State held between "redirect to the IdP" and "the IdP redirects back" for
-- enterprise OIDC federation. Parallel to webauthn_ceremonies (0010_passkeys.sql)
-- but a distinct shape: an OIDC state/nonce pair, not a serialized WebAuthn
-- challenge. No org_id-based RLS -- like webauthn_ceremonies, this is
-- short-lived correlation data read by state alone, before any session exists
-- to pin an org to. org_id is denormalized onto the row (not re-derived from
-- the domain a second time at callback) so a domain reassigned mid-flow can't
-- retarget an in-flight ceremony.
CREATE TABLE sso_ceremonies (
  id                uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  org_id            uuid NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
  idp_connection_id uuid NOT NULL REFERENCES idp_connections (id) ON DELETE CASCADE,
  -- Set only by the authenticated "link my SSO identity" path (an existing
  -- session proving account ownership); NULL for the anonymous "sign in with
  -- SSO" path. The callback links to this user_id directly when set, and
  -- never resolves by email at all in that case -- see the design spec's
  -- Assumptions on why an email match alone must never establish or extend
  -- account access.
  user_id           uuid REFERENCES users (id) ON DELETE CASCADE,
  -- Single-use, bearer-shaped, hashed at rest -- same convention as
  -- sessions/access_tokens.
  state_hash        bytea NOT NULL,
  -- A second secret, distinct from state_hash, carried by an HttpOnly cookie
  -- set at ceremony-start and checked at callback. state travels in a URL and
  -- can be captured and replayed by an attacker into a victim's browser
  -- (login CSRF); this is what proves the browser completing the callback is
  -- the same one that started the ceremony. See the design spec's
  -- Assumptions for the full threat this closes.
  binding_hash      bytea NOT NULL,
  -- Anti-replay on the returned id_token. Not a bearer credential (sent to
  -- the IdP as a plaintext query parameter); no confidentiality property to
  -- protect here.
  nonce             text NOT NULL,
  expires_at        timestamptz NOT NULL,
  consumed_at       timestamptz,
  created_at        timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX sso_ceremonies_state_key ON sso_ceremonies (state_hash);
CREATE INDEX sso_ceremonies_expiry_idx ON sso_ceremonies (expires_at);

-- Deliberately not added to 0007_rls.sql's tenant_tables array -- same
-- reasoning as webauthn_ceremonies: this table must be readable by
-- state_hash alone, before any org is known, and it holds no secret worth an
-- RLS policy protecting (the state token itself is what's confidential, and
-- it's hashed). Every write still carries an explicit org_id/id predicate
-- (guard 1) -- see crates/of-core/src/ceremonies.rs.
