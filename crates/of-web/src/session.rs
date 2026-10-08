//! Who is calling the console API.
//!
//! # The seam
//!
//! The console used to sign people in itself: passkeys, a session cookie, a
//! login page. All of that is the otto platform's now, and **console login is a
//! separate, later change** (an OAuth authorization-code + PKCE flow against the
//! platform that leaves this service holding its own session). Until then the
//! console API needs *some* way to know who is asking, and the one the platform
//! already provides is a bearer token.
//!
//! So this module accepts a platform-issued token on the console API:
//!
//! ```text
//!   Authorization: Bearer otto_at_…  (or otto_pat_…)
//!       └─ PlatformClient::introspect ── active? which user, org, role, scopes?
//!            └─ the `{org}` in the path must be the token's org
//! ```
//!
//! That is the same credential, validated by the same call, as the MCP surface
//! (`of_mcp::auth`), so the console API cannot be reached by anything an agent
//! could not already reach it with. **Everything marked `SEAM` below is what the
//! console-login change replaces**: it will resolve a session cookie to a held
//! platform token and feed it through [`authenticate`] unchanged, and nothing
//! downstream of [`OrgCtx`] needs to know the difference.
//!
//! # What an extractor guarantees
//!
//! [`OrgCtx`] resolves the caller, the org in the path, and their role before a
//! handler body runs, and a handler that needs more than membership says so in
//! one line. A handler that forgets is a handler that serves another tenant's
//! data, and a type is a better place for that than a review checklist.
//!
//! - **An org that is not the token's is `404`.** A token opens exactly one org,
//!   fixed when it was issued. Answering `403` on a real slug and `404` on a fake
//!   one turns any token into a directory of who uses the product.
//! - **A platform outage is `503`, never `401`.** Same reasoning as the MCP
//!   surface: `401` sends a client off to sign in again against the thing that is
//!   down.
//! - **Scopes are checked as the MCP tools check them.** A token that could not
//!   `repos:write` over MCP cannot over the console.

use axum::extract::{FromRef, FromRequestParts};
use http::request::Parts;
use otto_resource::{Role, TokenClaims};
use otto_tenant::ids::{OrgId, UserId};

use crate::error::ApiError;
use crate::state::AppState;

/// SEAM: the bearer token on the request, if any. The console-login change adds
/// the session-cookie path beside this one.
pub fn bearer_token(parts: &Parts) -> Option<&str> {
    let raw = parts
        .headers
        .get(http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let (scheme, token) = raw.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

/// Resolve the request's credential to the platform's claims about it.
///
/// SEAM: the single place a credential becomes an identity. Everything else in
/// the console API takes the result.
pub async fn authenticate(state: &AppState, parts: &Parts) -> Result<TokenClaims, ApiError> {
    let token = bearer_token(parts).ok_or_else(ApiError::unauthenticated)?;
    match state.platform.introspect(token).await {
        Ok(Some(claims)) => Ok(claims),
        // The platform answered "no": one answer for unknown, expired, revoked,
        // wrong audience, and a member who has since left.
        Ok(None) => Err(ApiError::unauthenticated()),
        Err(e) => Err(ApiError::platform_unavailable(
            "introspect a console token",
            e,
        )),
    }
}

/// The caller. Only an id: the user is the platform's record.
#[derive(Debug, Clone, Copy)]
pub struct Caller {
    pub id: UserId,
}

/// The org a request acts in.
#[derive(Debug, Clone)]
pub struct OrgRef {
    pub id: OrgId,
    pub slug: String,
}

/// The caller, acting in the one org their token opens.
#[derive(Debug, Clone)]
pub struct OrgCtx {
    pub user: Caller,
    pub org: OrgRef,
    /// The caller's role in the org as of the platform's last answer.
    pub role: Role,
    pub scopes: Vec<String>,
}

impl OrgCtx {
    /// Open a transaction pinned to this org, refused if the org was deleted or
    /// the caller removed since authentication (see
    /// `of_core::platform_events::begin_live`). Every handler opens its
    /// transaction here, never with `Db::begin` directly.
    pub async fn begin(&self, db: &otto_tenant::Db) -> Result<otto_tenant::Tx<'static>, ApiError> {
        Ok(of_core::platform_events::begin_live(db, self.org.id, Some(self.user.id)).await?)
    }

    /// Refuse unless the caller administers this org.
    ///
    /// The message names the role they have, because "you need to be an admin"
    /// without saying what you are leaves a person guessing whether they are
    /// looking at the right org.
    pub fn require_admin(&self) -> Result<(), ApiError> {
        if self.role.can_administer() {
            return Ok(());
        }
        Err(ApiError::forbidden(format!(
            "this needs an owner or admin of {}; you are a {}",
            self.org.slug,
            role_name(self.role)
        )))
    }

    /// Refuse unless the token carries `scope`. Named in the message, because
    /// it is the one thing the caller can act on: ask for a token that has it.
    pub fn require_scope(&self, scope: &str) -> Result<(), ApiError> {
        if self.scopes.iter().any(|s| s == scope) {
            return Ok(());
        }
        Err(ApiError::forbidden(format!(
            "this token lacks the {scope} scope"
        )))
    }
}

pub fn role_name(role: Role) -> &'static str {
    match role {
        Role::Owner => "owner",
        Role::Admin => "admin",
        Role::Member => "member",
    }
}

impl<S> FromRequestParts<S> for OrgCtx
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state = AppState::from_ref(state);

        let wanted = org_param(parts, &state).await.ok_or_else(|| {
            // A route mounted without an `{org}` segment reaching this
            // extractor is a wiring bug, not a caller error.
            ApiError::internal("org path parameter", "route has no {org} segment")
        })?;

        let claims = authenticate(&state, parts).await?;

        // A deleted org or a just-removed member is refused now, though the
        // platform's cached introspection may still vouch for the token.
        match of_core::platform_events::revoked(
            &state.db,
            claims.org_id.into(),
            claims.user_id.into(),
        )
        .await
        {
            Ok(false) => {}
            Ok(true) => return Err(ApiError::unauthenticated()),
            Err(e) => return Err(ApiError::unavailable("check revocation tombstones", e)),
        }

        // **An org that is not the token's is reported as not found, not as
        // forbidden** — see the module docs.
        let missing = || ApiError::not_found(format!("no org {wanted:?} that this token opens"));

        // The path may name the org by id or by slug. The platform is asked for
        // the slug rather than trusting anything the client sent.
        let (slug, role) = match state
            .platform
            .member(claims.org_id, claims.user_id)
            .await
            .map_err(|e| ApiError::platform_unavailable("look up the token's org", e))?
        {
            // The role comes from this fresh lookup, not the (up to 60 s old)
            // introspection claims: a demoted admin loses admin routes at once.
            Some(member) if member.org.id == claims.org_id => (member.org.slug, member.role),
            // The platform no longer sees this user in this org: the token is
            // dead, whatever the introspection cache still says.
            _ => return Err(ApiError::unauthenticated()),
        };
        let by_id = wanted
            .parse::<uuid::Uuid>()
            .is_ok_and(|id| id == claims.org_id);
        if !(by_id || wanted.eq_ignore_ascii_case(&slug)) {
            return Err(missing());
        }

        Ok(OrgCtx {
            user: Caller {
                id: claims.user_id.into(),
            },
            org: OrgRef {
                id: claims.org_id.into(),
                slug,
            },
            role,
            scopes: claims.scopes,
        })
    }
}

/// Read the `{org}` path segment.
///
/// `RawPathParams` rather than `Path<T>`: it clones the parameters out of the
/// request extensions rather than deserializing them into a type, so a handler
/// that also takes `Path<…>` for its *own* segments still sees everything.
async fn org_param(parts: &mut Parts, state: &AppState) -> Option<String> {
    let params = axum::extract::RawPathParams::from_request_parts(parts, state)
        .await
        .ok()?;
    params
        .iter()
        .find(|(key, _)| *key == "org")
        .map(|(_, value)| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts_with(auth: Option<&str>) -> Parts {
        let mut request = http::Request::builder();
        if let Some(auth) = auth {
            request = request.header(http::header::AUTHORIZATION, auth);
        }
        request.body(()).unwrap().into_parts().0
    }

    #[test]
    fn the_bearer_token_is_found() {
        for prefix in ["Bearer", "bearer", "BEARER"] {
            assert_eq!(
                bearer_token(&parts_with(Some(&format!("{prefix} otto_at_abc")))),
                Some("otto_at_abc")
            );
        }
    }

    #[test]
    fn other_schemes_and_empty_values_are_not_tokens() {
        for bad in [
            None,
            Some("Basic dXNlcjpwdw=="),
            Some("otto_at_abc"),
            Some("Bearer"),
            Some("Bearer   "),
        ] {
            assert_eq!(bearer_token(&parts_with(bad)), None, "{bad:?}");
        }
    }
}
