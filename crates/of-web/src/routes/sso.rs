//! Enterprise OIDC federation — the console's half.
//!
//! See `docs/specs/2026-09-16-oidc-federation-design.md` §5 for the design,
//! and `docs/plans/2026-09-16-oidc-federation.md`'s Task 3 for the literal
//! callback walkthrough this module follows step by step. Three things hold
//! across every handler here:
//!
//! - **A federated identity is never linked to a pre-existing `users` row by
//!   email match, under any circumstance.** [`identities::resolve_by_email`]
//!   is consulted exactly once, on the anonymous ceremony path, and only to
//!   decide "create a new user" vs. "refuse" — never to choose a link
//!   target. See `of_core::identities::create_user_for_federation`'s doc
//!   comment for the account-takeover shape this closes.
//! - **No `Tx` is held open across an outbound HTTP call.** [`callback`]'s
//!   only `Tx`s are the short, read-only one that opens the sealed client
//!   secret (before `exchange_code`/`verify_id_token` run, and released
//!   before either does) and the short one that provisions `org_members`
//!   for a brand-new federated user (after every IdP call has already
//!   returned).
//! - **The callback needs both `state` and the `__Host-of_sso_binding`
//!   cookie to agree before it does anything** — see [`session`]'s module
//!   docs and the design spec's Assumptions for the login-CSRF hole a bare
//!   `state` check leaves open.

use axum::extract::{Json, Path, Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use http::request::Parts;
use serde::{Deserialize, Serialize};

use of_auth::crypto::{self, prefix};
use of_auth::{dns, oidc, sessions, AuthError};
use of_core::audit::{action, Entry};
use of_core::ids::{OrgId, UserId};
use of_core::orgs::{Org, Role};
use of_core::{ceremonies, domains, identities, idp};

use crate::error::{ApiError, ApiResult};
use crate::oauth::escape;
use crate::session::{self, CurrentUser, OrgCtx};
use crate::state::{client_ip, AppState};

/// How long a ceremony (and its binding cookie) lives between "redirect to
/// the IdP" and "the IdP redirects back" — the design's 10-minute window
/// (spec §5's Assumptions), matching the "ten single-use codes"-era links'
/// own TTL convention this repo already follows for a single-use token.
const CEREMONY_TTL_SECS: i64 = 600;

/// `_otto-factory-verify` — must match `of_auth::dns::verify_txt_record`'s
/// own subdomain exactly, since this is what the console tells an admin to
/// put in DNS and that function is what actually checks it.
const VERIFY_SUBDOMAIN: &str = "_otto-factory-verify";

fn redirect_uri(config: &crate::state::Config) -> String {
    config.url("/sso/callback")
}

/// The domain half of an email address, lowercased comparison is the
/// database's job (`resolve_for_domain`'s `lower(...)`) — this just splits.
fn email_domain(email: &str) -> Option<&str> {
    email.rsplit_once('@').map(|(_, domain)| domain)
}

/// The exact TXT record name/value an admin must publish — computed the same
/// way in both [`claim_domain`] and [`list_domains`], and matching
/// `of_auth::dns::verify_txt_record`'s own format string.
fn txt_instructions(domain: &str, token: &str) -> (String, String) {
    (
        format!("{VERIFY_SUBDOMAIN}.{domain}"),
        format!("otto-factory-verify={token}"),
    )
}

// ---------------------------------------------------------------------------
// sso/start and me/sso/link/start
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SsoStartRequest {
    pub email: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SsoStartResponse {
    pub redirect_url: String,
}

/// `POST /api/auth/sso/start` — the anonymous "sign in with SSO" entry point.
///
/// Unauthenticated: resolves the IdP purely from the email's domain. No
/// account is looked up and no session is required, which is what lets a
/// signed-out visitor reach it from the login page.
pub async fn sso_start(
    State(state): State<AppState>,
    Json(req): Json<SsoStartRequest>,
) -> ApiResult<Response> {
    start_ceremony(&state, &req.email, None).await
}

/// `POST /api/me/sso/link/start` — the authenticated "link my SSO identity"
/// entry point.
///
/// The caller's own `user_id` rides along on the minted ceremony
/// (`sso_ceremonies.user_id`), which is what makes the callback link to
/// *this* account by construction rather than by any email lookup — see the
/// design spec's Assumptions on why an email match alone must never
/// establish or extend account access.
pub async fn sso_link_start(
    State(state): State<AppState>,
    caller: CurrentUser,
) -> ApiResult<Response> {
    let email = caller.user.email.clone().ok_or_else(|| {
        ApiError::bad_request(
            "your account has no email address yet — set one with PATCH /api/me before \
             linking a single sign-on identity, so there is something to match your \
             identity provider's verified email against.",
        )
    })?;
    start_ceremony(&state, &email, Some(caller.user.id)).await
}

async fn start_ceremony(
    state: &AppState,
    email: &str,
    caller_user_id: Option<UserId>,
) -> ApiResult<Response> {
    let domain = email_domain(email)
        .filter(|d| !d.is_empty())
        .ok_or_else(|| ApiError::bad_request("not a valid email address"))?;

    let Some((org_id, connection)) = idp::resolve_for_domain(&state.db, domain).await? else {
        return Err(ApiError::new(
            http::StatusCode::BAD_REQUEST,
            "sso_not_configured",
            "no SSO configured for this address — sign in with a passkey instead",
        ));
    };

    let state_secret = crypto::generate(prefix::SSO_STATE);
    let binding_secret = crypto::generate(prefix::SSO_BINDING);
    let nonce = crypto::generate_nonce();
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(CEREMONY_TTL_SECS);

    ceremonies::create(
        &state.db,
        org_id,
        connection.id,
        caller_user_id,
        &state_secret.hash,
        &binding_secret.hash,
        &nonce,
        expires_at,
    )
    .await?;

    let redirect_url = oidc::authorization_url(
        &connection.discovery,
        &connection.client_id,
        &redirect_uri(&state.config),
        state_secret.expose(),
        &nonce,
    )?;

    let body = Json(SsoStartResponse {
        redirect_url: redirect_url.to_string(),
    });
    Ok(session::with_cookie(
        body.into_response(),
        session::set_binding_cookie(binding_secret.expose(), CEREMONY_TTL_SECS),
    ))
}

// ---------------------------------------------------------------------------
// GET /sso/callback
// ---------------------------------------------------------------------------

/// Why a callback attempt did not open a session.
///
/// `Refuse` carries the exact text the browser sees. Deliberately **not**
/// distinguished for the "four possible causes" family (no/expired/consumed/
/// mismatched `state`, missing/mismatched binding cookie) — those all share
/// [`GENERIC_REFUSAL`], per spec §5/Error Handling: this is the
/// unauthenticated bootstrap surface, and telling the four apart is only
/// useful to an attacker probing it. Every later check gets its own,
/// specific text where the spec calls for one.
enum Outcome {
    Refuse(&'static str),
    Error(ApiError),
}

impl From<of_core::Error> for Outcome {
    fn from(e: of_core::Error) -> Self {
        Outcome::Error(e.into())
    }
}

impl From<AuthError> for Outcome {
    fn from(e: AuthError) -> Self {
        Outcome::Error(e.into())
    }
}

const GENERIC_REFUSAL: &str = "This sign-in link is invalid, expired, or has already been \
     used. Start again from your organization's sign-in page.";
const NOT_EMAIL_VERIFIED: &str = "Your identity provider did not confirm your email address. \
     Contact your organization's sign-in support.";
const EMAIL_MISMATCH: &str = "The identity provider account you used does not match the email \
     address on this otto-factory account. Sign in with the identity provider account that \
     matches, or update your account's email first.";
const EMAIL_COLLISION: &str = "An account already exists for this email address. Sign in with \
     your existing credentials, then link single sign-on from account settings.";
const IDENTITY_LINKED_ELSEWHERE: &str = "That identity provider account is already linked to a \
     different otto-factory account.";

#[derive(Debug, Deserialize)]
pub struct CallbackParams {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
}

/// `GET /sso/callback?code=...&state=...` — the IdP's redirect back.
///
/// Follows `docs/plans/2026-09-16-oidc-federation.md`'s Task 3 walkthrough
/// step by step; see [`callback_inner`] for the numbered steps themselves.
/// Every outcome — success or refusal — clears the binding cookie, since it
/// is useless the moment its ceremony is consumed (single-use) and a stale
/// copy left in the browser past its purpose is hygiene worth doing anyway
/// (spec's Risks & Open Questions).
pub async fn callback(
    State(state): State<AppState>,
    parts: Parts,
    Query(params): Query<CallbackParams>,
) -> Response {
    match callback_inner(&state, &parts, &params).await {
        Ok((user_id, token)) => {
            let ip = client_ip(&parts, &state.config);
            // Best-effort, matching `login::with_passkey`'s own audit write
            // for the passkey path — a lost row here is worse to compound
            // into a second failure on an otherwise-successful sign-in than
            // it is to simply lose it.
            let entry = Entry::new(action::LOGIN_SUCCEEDED)
                .actor(user_id)
                .from_request(ip.as_deref(), None)
                .detail(serde_json::json!({ "method": "sso" }));
            if let Err(e) = state.db.audit_global(entry).await {
                tracing::error!(error = %e, "failed to write audit event for an SSO sign-in");
            }

            let mut response = Redirect::to(&state.config.url("/")).into_response();
            response = session::with_cookie(response, session::set_cookie(&token));
            session::with_cookie(response, session::clear_binding_cookie())
        }
        Err(Outcome::Refuse(message)) => {
            // Every refusal branch in callback_inner funnels through here —
            // a single log line for all of them, uniform coverage even for
            // the branches that don't have their own tracing::warn! below.
            // The browser's response deliberately collapses several distinct
            // causes into one generic message (spec §5); this log line does
            // not — an operator troubleshooting "SSO stopped working," or
            // investigating a suspected identity-theft attempt, needs to
            // tell them apart even though a caller probing the endpoint must
            // not be able to.
            tracing::warn!(message, "SSO callback refused");
            refusal_response(message)
        }
        Err(Outcome::Error(e)) => {
            session::with_cookie(e.into_response(), session::clear_binding_cookie())
        }
    }
}

fn refusal_response(message: &str) -> Response {
    let body = format!(
        "<!doctype html><html lang=en><meta charset=utf-8><title>Sign-in problem</title>\
         <main><h1>Sign-in problem</h1><p>{}</p></main>",
        escape(message)
    );
    (http::StatusCode::BAD_REQUEST, Html(body)).into_response()
}

/// The numbered steps from `docs/plans/2026-09-16-oidc-federation.md`'s Task
/// 3 — literal, in order, every branch present. Returns the resolved
/// `user_id` and the freshly minted session's plaintext token on success.
async fn callback_inner(
    state: &AppState,
    parts: &Parts,
    params: &CallbackParams,
) -> Result<(UserId, String), Outcome> {
    // Step 1: hash the incoming `state`, resolve and burn the ceremony.
    let raw_state = params
        .state
        .as_deref()
        .ok_or(Outcome::Refuse(GENERIC_REFUSAL))?;
    let state_hash = crypto::hash(raw_state);
    let ceremony = ceremonies::consume_by_state_hash(&state.db, &state_hash)
        .await?
        .ok_or(Outcome::Refuse(GENERIC_REFUSAL))?;

    // Step 2: the binding cookie must match — checked *after* the ceremony
    // is already consumed above, so a mismatched attempt cannot be retried
    // against the same ceremony either (spec §5).
    let binding_matches = session::binding_token_from(parts)
        .map(|token| crypto::verify(&crypto::hash(&token), &ceremony.binding_hash))
        .unwrap_or(false);
    if !binding_matches {
        return Err(Outcome::Refuse(GENERIC_REFUSAL));
    }

    let code = params
        .code
        .as_deref()
        .ok_or(Outcome::Refuse(GENERIC_REFUSAL))?;

    // Step 3: a short, read-only Tx pinned to the ceremony's own org_id
    // reads the connection and its sealed secret, and is released — nothing
    // below this point holds a Tx open while talking to the IdP.
    let mut tx = state.db.begin(ceremony.org_id).await?;
    let Some(connection) = idp::get_connection(&mut tx).await? else {
        return Err(Outcome::Refuse(GENERIC_REFUSAL));
    };
    let Some(sealed) = idp::get_connection_secret(&mut tx).await? else {
        return Err(Outcome::Refuse(GENERIC_REFUSAL));
    };
    tx.commit().await?;

    let secret_bytes = state.cipher.open(&sealed.ciphertext, &sealed.nonce)?;
    let client_secret = String::from_utf8(secret_bytes).map_err(|_| {
        Outcome::Error(ApiError::internal(
            "sso callback",
            "stored IdP client secret is not valid utf-8",
        ))
    })?;

    let redirect = redirect_uri(&state.config);
    let token = oidc::exchange_code(
        &connection.discovery,
        &connection.client_id,
        &client_secret,
        code,
        &redirect,
    )
    .await
    .map_err(|e| {
        // The browser gets the same generic refusal every other cause in
        // this family does (spec §5); an operator troubleshooting "SSO
        // stopped working for this org" has nothing else to go on unless
        // the real cause is logged here.
        tracing::warn!(org_id = %ceremony.org_id, error = %e, "SSO code exchange failed");
        Outcome::Refuse(GENERIC_REFUSAL)
    })?;

    let claims = oidc::verify_id_token(
        &connection.discovery,
        &connection.client_id,
        &token.id_token,
        &ceremony.nonce,
    )
    .await
    .map_err(|e| {
        tracing::warn!(org_id = %ceremony.org_id, error = %e, "SSO id_token verification failed");
        Outcome::Refuse(GENERIC_REFUSAL)
    })?;

    // The one hard security rule from spec §4: never trust an email claim
    // that was not asserted as verified.
    if claims.email_verified != Some(true) {
        return Err(Outcome::Refuse(NOT_EMAIL_VERIFIED));
    }
    let email = claims
        .email
        .as_deref()
        .ok_or(Outcome::Refuse(NOT_EMAIL_VERIFIED))?;
    let domain = email_domain(email).ok_or(Outcome::Refuse(GENERIC_REFUSAL))?;

    // The verified email's domain must still resolve, fresh, to this
    // ceremony's own org — not "some org". A domain reassigned or unclaimed
    // since the ceremony started is a refusal, never a fallback to whatever
    // org the domain now belongs to (spec §5/Assumptions).
    match idp::resolve_for_domain(&state.db, domain).await? {
        Some((org_id, _)) if org_id == ceremony.org_id => {}
        _ => return Err(Outcome::Refuse(GENERIC_REFUSAL)),
    }

    // Step 4: a returning federated user, if this (idp_connection_id,
    // subject) pair is already linked — what a match *means* depends on the
    // ceremony kind.
    let existing =
        identities::resolve_user(&state.db, ceremony.idp_connection_id, &claims.sub).await?;

    let user_id = match (ceremony.user_id, existing) {
        // Anonymous ceremony: any match is a returning federated user.
        (None, Some(uid)) => uid,
        // Authenticated ceremony, already linked to the caller: idempotent.
        (Some(caller_uid), Some(uid)) if uid == caller_uid => uid,
        // Authenticated ceremony, linked to a DIFFERENT account: refuse.
        // This is the identity-theft guard — this ceremony's caller cannot
        // steal another account's IdP link by replaying a callback against
        // it. The single most important line in this whole function, so it
        // gets its own structured log line rather than relying only on the
        // generic one every Outcome::Refuse produces (callback()) — an
        // attempt against this specific branch is exactly the kind of event
        // an operator wants to be able to find without wading through every
        // ordinary "link expired" refusal.
        (Some(caller_uid), Some(other_uid)) => {
            tracing::warn!(
                org_id = %ceremony.org_id,
                idp_connection_id = %ceremony.idp_connection_id,
                caller_user_id = %caller_uid,
                already_linked_to = %other_uid,
                "SSO identity-theft guard fired: an authenticated link ceremony resolved to a different account's identity"
            );
            return Err(Outcome::Refuse(IDENTITY_LINKED_ELSEWHERE));
        }
        // Step 5: authenticated ceremony, not yet linked.
        (Some(caller_uid), None) => {
            link_authenticated(
                state,
                caller_uid,
                ceremony.idp_connection_id,
                &claims.sub,
                email,
            )
            .await?
        }
        // Step 6: anonymous ceremony, not yet linked.
        (None, None) => {
            link_anonymous(
                state,
                ceremony.org_id,
                ceremony.idp_connection_id,
                &claims.sub,
                email,
            )
            .await?
        }
    };

    // Step 7: success.
    let new_session = sessions::create(&state.db, user_id).await?;
    Ok((user_id, new_session.token))
}

/// Step 5: the authenticated-link ceremony, `resolve_user` returned `None`.
///
/// Requires the verified email to case-insensitively match the caller's own
/// current `users.email` before linking — not just the same domain. Without
/// this, a shared browser or a stale IdP session could silently link a
/// *different* person's real identity onto the caller's account with no
/// confirmation step.
async fn link_authenticated(
    state: &AppState,
    caller_user_id: UserId,
    idp_connection_id: uuid::Uuid,
    subject: &str,
    verified_email: &str,
) -> Result<UserId, Outcome> {
    let caller = state
        .db
        .get_user(caller_user_id)
        .await?
        .ok_or(Outcome::Refuse(GENERIC_REFUSAL))?;

    let matches = caller
        .email
        .as_deref()
        .is_some_and(|e| e.eq_ignore_ascii_case(verified_email));
    if !matches {
        return Err(Outcome::Refuse(EMAIL_MISMATCH));
    }

    // `link`'s `ON CONFLICT (idp_connection_id, subject) DO NOTHING` returns
    // the *existing* row on a race, which is not necessarily `caller_user_id`
    // — a concurrent callback could have linked this exact pair to a
    // different account in the window since the `resolve_user` check above.
    // Trust the row `link` actually reports, not the id this call intended;
    // a mismatch is the same identity-theft shape the main match's
    // `(Some(_), Some(_))` arm refuses, just narrowed to this race window.
    let identity = identities::link(&state.db, caller_user_id, idp_connection_id, subject).await?;
    if identity.user_id != caller_user_id {
        tracing::warn!(
            idp_connection_id = %idp_connection_id,
            caller_user_id = %caller_user_id,
            already_linked_to = %identity.user_id,
            "SSO identity-theft guard fired in the linking race window: a concurrent callback \
             linked this identity to a different account first"
        );
        return Err(Outcome::Refuse(IDENTITY_LINKED_ELSEWHERE));
    }
    Ok(caller_user_id)
}

/// Step 6: the anonymous ceremony, `resolve_user` returned `None`.
///
/// `resolve_by_email` is consulted **only** to decide "create a new user" vs.
/// "refuse" — `Some(_)` is never a link target. The `org_members` write is
/// the one point in steps 4–6 that needs a `Tx`; every other call here is
/// unscoped, per `of_core::identities`'s and `of_core::idp`'s own doc
/// comments.
async fn link_anonymous(
    state: &AppState,
    org_id: OrgId,
    idp_connection_id: uuid::Uuid,
    subject: &str,
    verified_email: &str,
) -> Result<UserId, Outcome> {
    if identities::resolve_by_email(&state.db, verified_email)
        .await?
        .is_some()
    {
        return Err(Outcome::Refuse(EMAIL_COLLISION));
    }

    // `None` here means someone else's row won the creation race in the
    // window since the check above — never a hint to adopt that row.
    let Some(new_user_id) =
        identities::create_user_for_federation(&state.db, verified_email).await?
    else {
        return Err(Outcome::Refuse(EMAIL_COLLISION));
    };

    // `link`'s `ON CONFLICT (idp_connection_id, subject) DO NOTHING` returns
    // the *existing* row on a race, which may not be the row we just
    // created — a concurrent callback could have linked this exact pair
    // first. Use the id `link` actually reports as the winner, not
    // `new_user_id` unconditionally, or a session/org_members row could be
    // minted for a freshly-created account that `user_identities` doesn't
    // actually point at. The loser (`new_user_id`, if it differs) is left as
    // a harmless identity-less user row — the same one-shot race-debris
    // tolerance `identities::link`'s own doc comment already accepts.
    let identity = identities::link(&state.db, new_user_id, idp_connection_id, subject).await?;
    let user_id = identity.user_id;

    // Not optional: without this, a federated user authenticates
    // successfully but sees no orgs in the console — the concrete mechanism
    // behind "self-service enterprise onboarding" (spec's Goal).
    let mut tx = state.db.begin(org_id).await?;
    tx.add_member(user_id, Role::Member).await?;
    tx.audit(
        Entry::new(action::MEMBER_JOINED)
            .actor(user_id)
            .target("user", user_id.to_string())
            .detail(serde_json::json!({ "via": "sso" })),
    )
    .await?;
    tx.commit().await?;

    Ok(user_id)
}

// ---------------------------------------------------------------------------
// Admin: connection, domains, enforce_sso
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionRequest {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
}

/// The three discovery fields the OIDC flow needs — checked here, at bind
/// time, so a connection is never saved half-configured
/// (`of_auth::oidc::discovery_str`'s own doc comment names this handler as
/// the one expected to do it).
fn require_discovery_fields(discovery: &serde_json::Value) -> ApiResult<()> {
    let missing: Vec<&str> = ["authorization_endpoint", "token_endpoint", "jwks_uri"]
        .into_iter()
        .filter(|field| !discovery.get(field).is_some_and(|v| v.is_string()))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(ApiError::bad_request(format!(
            "the identity provider's discovery document is missing required field(s): {}. \
             Fix the issuer's /.well-known/openid-configuration and try again.",
            missing.join(", ")
        )))
    }
}

/// `PUT /api/orgs/{org}/sso/connection` — bind or replace this org's IdP.
///
/// The discovery fetch happens before any transaction opens, matching
/// `routes::trackers`'s "network work happens before the transaction, not
/// inside it" rule.
pub async fn upsert_connection(
    State(state): State<AppState>,
    ctx: OrgCtx,
    Json(req): Json<ConnectionRequest>,
) -> ApiResult<Json<idp::IdpConnection>> {
    ctx.require_admin()?;

    let discovery = oidc::fetch_discovery(&req.issuer).await?;
    require_discovery_fields(&discovery)?;
    let sealed = state.cipher.seal(req.client_secret.as_bytes())?;

    let mut tx = state.db.begin(ctx.org.id).await?;
    let connection =
        idp::upsert_connection(&mut tx, &req.issuer, &req.client_id, sealed, discovery).await?;
    tx.audit(
        Entry::new(action::IDP_CONNECTED)
            .actor(ctx.user.id)
            .target("idp_connection", req.issuer.clone()),
    )
    .await?;
    tx.commit().await?;

    Ok(Json(connection))
}

/// `DELETE /api/orgs/{org}/sso/connection`.
///
/// The lockout guard (refusing while `enforce_sso` is on) lives inside
/// `of_core::idp::delete_connection` itself, which locks the org row before
/// deciding — this handler only calls it and maps the resulting
/// `Error::SsoLockout` the same way every other `of-core` error is mapped.
pub async fn delete_connection(State(state): State<AppState>, ctx: OrgCtx) -> ApiResult<Response> {
    ctx.require_admin()?;

    let mut tx = state.db.begin(ctx.org.id).await?;
    idp::delete_connection(&mut tx).await?;
    tx.audit(Entry::new(action::IDP_DISCONNECTED).actor(ctx.user.id))
        .await?;
    tx.commit().await?;

    Ok(http::StatusCode::NO_CONTENT.into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimDomainRequest {
    pub domain: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimedDomainView {
    #[serde(flatten)]
    pub domain: domains::ClaimedDomain,
    /// The exact TXT record name/value the console renders as setup
    /// instructions.
    pub txt_record_name: String,
    pub txt_record_value: String,
}

impl From<domains::ClaimedDomain> for ClaimedDomainView {
    fn from(domain: domains::ClaimedDomain) -> Self {
        let (txt_record_name, txt_record_value) =
            txt_instructions(&domain.domain, &domain.verification_token);
        Self {
            domain,
            txt_record_name,
            txt_record_value,
        }
    }
}

/// `POST /api/orgs/{org}/sso/domains` — claim a domain and mint its
/// verification token.
pub async fn claim_domain(
    State(state): State<AppState>,
    ctx: OrgCtx,
    Json(req): Json<ClaimDomainRequest>,
) -> ApiResult<Response> {
    ctx.require_admin()?;
    let token = domains::generate_verification_token();

    let mut tx = state.db.begin(ctx.org.id).await?;
    let claimed = domains::claim(&mut tx, &req.domain, &token).await?;
    tx.audit(
        Entry::new(action::DOMAIN_CLAIMED)
            .actor(ctx.user.id)
            .target("domain", claimed.domain.clone()),
    )
    .await?;
    tx.commit().await?;

    Ok((
        http::StatusCode::CREATED,
        Json::<ClaimedDomainView>(claimed.into()),
    )
        .into_response())
}

/// `GET /api/orgs/{org}/sso/domains` — this org's claimed domains, verified
/// or not.
///
/// Admin-only, matching `require_admin` for consistency with the rest of
/// this settings surface — spec §5's own explicit decision, made even though
/// nothing in the response is a secret.
pub async fn list_domains(
    State(state): State<AppState>,
    ctx: OrgCtx,
) -> ApiResult<Json<Vec<ClaimedDomainView>>> {
    ctx.require_admin()?;

    let mut tx = state.db.begin(ctx.org.id).await?;
    let domains = domains::list(&mut tx).await?;
    tx.commit().await?;

    Ok(Json(domains.into_iter().map(Into::into).collect()))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyDomainResponse {
    pub verified: bool,
}

/// `POST /api/orgs/{org}/sso/domains/{domain}/verify`.
///
/// The DNS lookup runs outside any `Tx` — the same rule the tracker sync
/// engine's own outbound calls follow, and the same rule this design's own
/// callback follows for its two outbound calls to the IdP.
pub async fn verify_domain(
    State(state): State<AppState>,
    ctx: OrgCtx,
    Path((_org, domain)): Path<(String, String)>,
) -> ApiResult<Json<VerifyDomainResponse>> {
    ctx.require_admin()?;

    let mut tx = state.db.begin(ctx.org.id).await?;
    let claimed = domains::list(&mut tx)
        .await?
        .into_iter()
        .find(|d| d.domain.eq_ignore_ascii_case(&domain))
        .ok_or_else(|| {
            ApiError::not_found(format!("domain {domain:?} is not claimed by this org"))
        })?;
    tx.commit().await?;

    let verified = dns::verify_txt_record(&claimed.domain, &claimed.verification_token).await?;

    if verified {
        let mut tx = state.db.begin(ctx.org.id).await?;
        domains::mark_verified(&mut tx, &claimed.domain).await?;
        tx.audit(
            Entry::new(action::DOMAIN_VERIFIED)
                .actor(ctx.user.id)
                .target("domain", claimed.domain.clone()),
        )
        .await?;
        tx.commit().await?;
    }

    Ok(Json(VerifyDomainResponse { verified }))
}

/// `DELETE /api/orgs/{org}/sso/domains/{domain}`.
///
/// The lockout guard (refusing while `enforce_sso` is on and this is the
/// org's only verified domain) lives inside `of_core::domains::delete`
/// itself, the same as [`delete_connection`]'s.
pub async fn delete_domain(
    State(state): State<AppState>,
    ctx: OrgCtx,
    Path((_org, domain)): Path<(String, String)>,
) -> ApiResult<Response> {
    ctx.require_admin()?;

    let mut tx = state.db.begin(ctx.org.id).await?;
    domains::delete(&mut tx, &domain).await?;
    tx.audit(
        Entry::new(action::DOMAIN_UNCLAIMED)
            .actor(ctx.user.id)
            .target("domain", domain.clone()),
    )
    .await?;
    tx.commit().await?;

    Ok(http::StatusCode::NO_CONTENT.into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnforceSsoRequest {
    pub enforce_sso: bool,
}

/// `PUT /api/orgs/{org}/sso/enforce`.
///
/// The lockout guard (refusing to turn this on with no working IdP path)
/// lives inside `of_core::orgs::set_enforce_sso` itself.
pub async fn set_enforce(
    State(state): State<AppState>,
    ctx: OrgCtx,
    Json(req): Json<EnforceSsoRequest>,
) -> ApiResult<Json<Org>> {
    ctx.require_admin()?;

    let mut tx = state.db.begin(ctx.org.id).await?;
    let org = of_core::orgs::set_enforce_sso(&mut tx, req.enforce_sso).await?;
    tx.audit(
        Entry::new(action::ENFORCE_SSO_CHANGED)
            .actor(ctx.user.id)
            .detail(serde_json::json!({ "enforceSso": req.enforce_sso })),
    )
    .await?;
    tx.commit().await?;

    Ok(Json(org))
}
