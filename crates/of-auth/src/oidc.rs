//! The OIDC authorization-code client for enterprise SSO federation.
//!
//! Pure client logic, no SQL — see `docs/specs/2026-09-16-oidc-federation-design.md`
//! §4. `of_core::idp` owns `idp_connections.discovery`'s storage; this module only
//! fetches it, builds the authorization URL from it, exchanges a code for tokens
//! against it, and verifies a returned `id_token` against the IdP's JWKS.
//!
//! Four steps, one function each: [`fetch_discovery`], [`authorization_url`],
//! [`exchange_code`], [`verify_id_token`].

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant};

use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::error::{AuthError, Result};

/// The outbound calls here happen inline in a browser-facing request (the
/// admin binding a connection, or the callback exchanging a code) — a
/// stalled IdP must not hang that request forever. reqwest sets no timeout
/// by default.
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// An operator-facing error body is diagnostic text, not something to echo
/// back whole — bounded the same way `of_trackers`'s `MAX_ERROR_BODY_BYTES`
/// bounds a GitHub/JIRA error body.
const MAX_ERROR_BODY_BYTES: usize = 256;

/// How long a fetched JWKS document is trusted before this module fetches it
/// again. Short-lived, per spec §4 — long enough that a login doesn't refetch
/// the JWKS on every callback, short enough that a key rotation on the IdP's
/// side propagates without otto-factory needing a restart.
const JWKS_CACHE_TTL: Duration = Duration::from_secs(300);

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|source| AuthError::OidcHttp {
            action: "building the HTTP client",
            source,
        })
}

async fn response_body_snippet(response: reqwest::Response) -> String {
    match response.text().await {
        Ok(body) => body.chars().take(MAX_ERROR_BODY_BYTES).collect(),
        Err(_) => String::new(),
    }
}

/// Reads a required string field out of a cached discovery document.
/// Every one of `authorization_url`/`exchange_code`/`verify_id_token` calls
/// this for the one field it needs, which is what makes a discovery document
/// missing `authorization_endpoint`/`token_endpoint`/`jwks_uri` fail at the
/// exact point it's used — `of-web`'s `PUT .../sso/connection` handler (Task
/// 3) is expected to exercise all three at bind time so a connection is
/// never saved half-configured, per the design spec's Error Handling section.
fn discovery_str<'a>(discovery: &'a Value, field: &'static str) -> Result<&'a str> {
    discovery
        .get(field)
        .and_then(Value::as_str)
        .ok_or(AuthError::OidcDiscoveryField(field))
}

/// Fetches `{issuer}/.well-known/openid-configuration` and returns the raw
/// JSON body verbatim — `of_core::idp::upsert_connection`'s caller stores
/// this directly into `idp_connections.discovery`. No field validation here;
/// see [`discovery_str`] for where a missing field actually surfaces.
pub async fn fetch_discovery(issuer: &str) -> Result<Value> {
    let url = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    );
    let client = http_client()?;
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|source| AuthError::OidcHttp {
            action: "fetching the discovery document",
            source,
        })?;

    let status = response.status();
    if !status.is_success() {
        let body = response_body_snippet(response).await;
        return Err(AuthError::OidcApi {
            action: "fetching the discovery document",
            status: status.as_u16(),
            body,
        });
    }

    response
        .json::<Value>()
        .await
        .map_err(|source| AuthError::OidcHttp {
            action: "parsing the discovery document",
            source,
        })
}

/// Builds the authorization-code redirect URL from a cached discovery
/// document. `scope=openid email profile` — otto-factory only ever needs the
/// caller's email for the account it's pinning, and `profile` for a display
/// name; nothing wider is requested.
pub fn authorization_url(
    discovery: &Value,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    nonce: &str,
) -> Result<Url> {
    let endpoint = discovery_str(discovery, "authorization_endpoint")?;
    let mut url = Url::parse(endpoint)
        .map_err(|_| AuthError::OidcDiscoveryField("authorization_endpoint"))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("scope", "openid email profile")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("state", state)
        .append_pair("nonce", nonce);
    Ok(url)
}

/// The token endpoint's response. Only the fields this design ever reads are
/// named explicitly; `serde` ignores anything else the IdP sends.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: String,
    #[serde(default)]
    pub expires_in: Option<i64>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    pub id_token: String,
}

/// Exchanges an authorization code for tokens. `client_secret` is the
/// **opened** plaintext — the caller (`of-web`'s callback handler) holds it
/// only for the duration of this call, having just opened it via
/// `of_core::crypto::Cipher::open` from `idp::get_connection_secret`'s sealed
/// pair; it is never logged and never returned by this function.
pub async fn exchange_code(
    discovery: &Value,
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: &str,
) -> Result<TokenResponse> {
    let token_endpoint = discovery_str(discovery, "token_endpoint")?;
    let client = http_client()?;
    let response = client
        .post(token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", client_id),
            ("client_secret", client_secret),
        ])
        .send()
        .await
        .map_err(|source| AuthError::OidcHttp {
            action: "exchanging the authorization code",
            source,
        })?;

    let status = response.status();
    if !status.is_success() {
        let body = response_body_snippet(response).await;
        return Err(AuthError::OidcApi {
            action: "exchanging the authorization code",
            status: status.as_u16(),
            body,
        });
    }

    response
        .json::<TokenResponse>()
        .await
        .map_err(|source| AuthError::OidcHttp {
            action: "parsing the token response",
            source,
        })
}

/// Claims extracted from a verified `id_token` — exactly the fields this
/// design consumes, nothing else deserialized.
///
/// **This struct's fields are never enforced by [`verify_id_token`] itself,
/// and `email_verified` is the one that matters: do not add a check here
/// that refuses on `email_verified != Some(true)`.** Whether an unverified
/// email is trustworthy is a question about *this feature's* trust
/// requirement, not about whether the token is genuinely, freshly, and
/// correctly-audienced from the issuer it claims to be from — those are two
/// separate, equally-required questions, and `verify_id_token`'s job is only
/// the second one. The caller — `of-web`'s `/sso/callback` handler — is the
/// one place that knows the first check is required before anything reads
/// `claims.email`; folding it in here would make it easy for a future
/// change to "clean up" by moving a callback-only requirement into a shared
/// library function, silently applying (or, worse, failing to apply) it to
/// every future caller of this module. See
/// `docs/specs/2026-09-16-oidc-federation-design.md` §4/§5 for the full
/// reasoning and the account-takeover shape this closes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub email: Option<String>,
    pub email_verified: Option<bool>,
}

/// Superset of [`Claims`] used only inside [`verify_id_token`], to read the
/// anti-replay `nonce` claim — `nonce` is not part of the public `Claims`
/// shape this design hands its callers, since nothing past this function
/// needs it once the check below has run.
#[derive(Debug, Deserialize)]
struct IdTokenClaims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: Option<bool>,
    #[serde(default)]
    nonce: Option<String>,
}

struct CachedJwks {
    jwks: JwkSet,
    fetched_at: Instant,
}

fn jwks_cache() -> &'static RwLock<HashMap<String, CachedJwks>> {
    static CACHE: OnceLock<RwLock<HashMap<String, CachedJwks>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

async fn fetch_jwks(jwks_uri: &str) -> Result<JwkSet> {
    if let Some(cached) = jwks_cache()
        .read()
        .map_err(|_| AuthError::IdTokenInvalid("JWKS cache lock was poisoned".into()))?
        .get(jwks_uri)
    {
        if cached.fetched_at.elapsed() < JWKS_CACHE_TTL {
            return Ok(cached.jwks.clone());
        }
    }

    let client = http_client()?;
    let response = client
        .get(jwks_uri)
        .send()
        .await
        .map_err(|source| AuthError::OidcHttp {
            action: "fetching the JWKS document",
            source,
        })?;

    let status = response.status();
    if !status.is_success() {
        let body = response_body_snippet(response).await;
        return Err(AuthError::OidcApi {
            action: "fetching the JWKS document",
            status: status.as_u16(),
            body,
        });
    }

    let jwks: JwkSet = response
        .json()
        .await
        .map_err(|source| AuthError::OidcHttp {
            action: "parsing the JWKS document",
            source,
        })?;

    jwks_cache()
        .write()
        .map_err(|_| AuthError::IdTokenInvalid("JWKS cache lock was poisoned".into()))?
        .insert(
            jwks_uri.to_string(),
            CachedJwks {
                jwks: jwks.clone(),
                fetched_at: Instant::now(),
            },
        );

    Ok(jwks)
}

/// Verifies an `id_token`'s signature against the IdP's JWKS (matched by the
/// token header's `kid`), then checks `iss == discovery.issuer`, `aud`
/// contains `client_id`, `exp` has not passed, and `nonce == expected_nonce`.
///
/// The signing key's algorithm family (RSA/EC/HMAC, read from the matched
/// JWK) is cross-checked against the token header's claimed algorithm by
/// `jsonwebtoken` itself before the signature is even checked — an attacker
/// cannot present an HMAC-signed token keyed off the IdP's RSA public key and
/// have it accepted (the classic "alg confusion" attack), because the key
/// family and the claimed algorithm's family must agree.
///
/// Does **not** enforce `email_verified` — see [`Claims`]'s doc comment.
pub async fn verify_id_token(
    discovery: &Value,
    client_id: &str,
    id_token: &str,
    expected_nonce: &str,
) -> Result<Claims> {
    let issuer = discovery_str(discovery, "issuer")?;
    let jwks_uri = discovery_str(discovery, "jwks_uri")?;
    let jwks = fetch_jwks(jwks_uri).await?;

    let header = jsonwebtoken::decode_header(id_token).map_err(|source| {
        AuthError::IdTokenInvalid(format!("could not read the token header: {source}"))
    })?;
    let kid = header
        .kid
        .ok_or_else(|| AuthError::IdTokenInvalid("token header has no kid".into()))?;
    let jwk = jwks
        .find(&kid)
        .ok_or_else(|| AuthError::IdTokenInvalid(format!("no signing key matches kid {kid:?}")))?;
    let decoding_key = DecodingKey::from_jwk(jwk).map_err(|source| {
        AuthError::IdTokenInvalid(format!("signing key is unusable: {source}"))
    })?;

    let mut validation = Validation::new(header.alg);
    validation.set_audience(&[client_id]);
    validation.set_issuer(&[issuer]);
    validation.validate_exp = true;

    let token_data = jsonwebtoken::decode::<IdTokenClaims>(id_token, &decoding_key, &validation)
        .map_err(|source| {
            AuthError::IdTokenInvalid(format!("token failed verification: {source}"))
        })?;

    if token_data.claims.nonce.as_deref() != Some(expected_nonce) {
        return Err(AuthError::IdTokenInvalid(
            "nonce did not match this sign-in attempt".into(),
        ));
    }

    Ok(Claims {
        sub: token_data.claims.sub,
        email: token_data.claims.email,
        email_verified: token_data.claims.email_verified,
    })
}
