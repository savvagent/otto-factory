//! Recorded-fixture tests for `of_auth::oidc` — discovery fetch, authorization
//! URL construction, code exchange, and `id_token` verification. No live
//! network: every HTTP call in here goes to `support::TestServer`, a local
//! mock server started fresh per test. See `docs/specs/2026-09-16-oidc-federation-design.md`
//! §4 for the shapes under test.

mod support;

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use of_auth::error::AuthError;
use of_auth::oidc;
use serde_json::json;
use support::{MockResponse, TestServer};

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the epoch")
        .as_secs() as i64
}

// ---- fetch_discovery ------------------------------------------------------

#[tokio::test]
async fn fetch_discovery_returns_the_document_verbatim() {
    let server = TestServer::start().await;
    let doc = support::discovery_document(&server.base_url);
    server.push(MockResponse::json(200, doc.clone()));

    let fetched = oidc::fetch_discovery(&server.base_url)
        .await
        .expect("discovery fetch succeeds");
    assert_eq!(fetched, doc);

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, "/.well-known/openid-configuration");
}

#[tokio::test]
async fn fetch_discovery_surfaces_a_non_2xx_status_as_oidc_api() {
    let server = TestServer::start().await;
    server.push(MockResponse::text(503, "issuer is down for maintenance"));

    let err = oidc::fetch_discovery(&server.base_url)
        .await
        .expect_err("a 503 must not be treated as a document");
    match err {
        AuthError::OidcApi { status, body, .. } => {
            assert_eq!(status, 503);
            assert!(body.contains("maintenance"));
        }
        other => panic!("expected AuthError::OidcApi, got {other:?}"),
    }
}

// ---- authorization_url -----------------------------------------------------

#[test]
fn authorization_url_has_the_exact_query_shape_from_spec_4() {
    let discovery = support::discovery_document("https://idp.example.test");

    let url = oidc::authorization_url(
        &discovery,
        "client-123",
        "https://otto.example/sso/callback",
        "state-abc",
        "nonce-xyz",
    )
    .expect("discovery has an authorization_endpoint");

    assert_eq!(
        format!(
            "{}://{}{}",
            url.scheme(),
            url.host_str().unwrap(),
            url.path()
        ),
        "https://idp.example.test/authorize"
    );

    let pairs: HashMap<String, String> = url.query_pairs().into_owned().collect();
    assert_eq!(pairs.get("response_type").map(String::as_str), Some("code"));
    assert_eq!(
        pairs.get("scope").map(String::as_str),
        Some("openid email profile")
    );
    assert_eq!(
        pairs.get("client_id").map(String::as_str),
        Some("client-123")
    );
    assert_eq!(
        pairs.get("redirect_uri").map(String::as_str),
        Some("https://otto.example/sso/callback")
    );
    assert_eq!(pairs.get("state").map(String::as_str), Some("state-abc"));
    assert_eq!(pairs.get("nonce").map(String::as_str), Some("nonce-xyz"));
}

#[test]
fn authorization_url_refuses_a_discovery_document_missing_the_endpoint() {
    let discovery = json!({});

    let err = oidc::authorization_url(
        &discovery,
        "client-123",
        "https://otto.example/sso/callback",
        "state-abc",
        "nonce-xyz",
    )
    .expect_err("no authorization_endpoint to build a URL from");

    assert!(matches!(
        err,
        AuthError::OidcDiscoveryField("authorization_endpoint")
    ));
}

// ---- exchange_code ----------------------------------------------------------

#[tokio::test]
async fn exchange_code_happy_path() {
    let server = TestServer::start().await;
    let discovery = support::discovery_document(&server.base_url);
    server.push(MockResponse::json(
        200,
        json!({
            "access_token": "at-1",
            "token_type": "Bearer",
            "expires_in": 3600,
            "id_token": "header.payload.signature",
        }),
    ));

    let token = oidc::exchange_code(
        &discovery,
        "client-1",
        "shh-its-a-secret",
        "auth-code-1",
        "https://otto.example/sso/callback",
    )
    .await
    .expect("token exchange succeeds");

    assert_eq!(token.access_token, "at-1");
    assert_eq!(token.token_type, "Bearer");
    assert_eq!(token.expires_in, Some(3600));
    assert_eq!(token.id_token, "header.payload.signature");

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/token");
    let body = String::from_utf8(requests[0].body.clone()).expect("form body is utf8");
    assert!(body.contains("grant_type=authorization_code"));
    assert!(body.contains("code=auth-code-1"));
    assert!(body.contains("client_id=client-1"));
    assert!(body.contains("client_secret=shh-its-a-secret"));
    assert!(body.contains("redirect_uri="));
}

#[tokio::test]
async fn exchange_code_surfaces_a_rejected_authorization_code() {
    let server = TestServer::start().await;
    let discovery = support::discovery_document(&server.base_url);
    server.push(MockResponse::json(
        400,
        json!({"error": "invalid_grant", "error_description": "code already redeemed"}),
    ));

    let err = oidc::exchange_code(
        &discovery,
        "client-1",
        "shh",
        "spent-code",
        "https://otto.example/sso/callback",
    )
    .await
    .expect_err("a spent code must not produce a token");

    match err {
        AuthError::OidcApi { status, body, .. } => {
            assert_eq!(status, 400);
            assert!(body.contains("invalid_grant"));
        }
        other => panic!("expected AuthError::OidcApi, got {other:?}"),
    }
}

// ---- verify_id_token ---------------------------------------------------------

/// Every id_token test below starts a fresh `TestServer` (a fresh, unique
/// `jwks_uri`) specifically so `oidc`'s process-wide JWKS cache never lets
/// one test's fixture answer another's lookup.
async fn jwks_server() -> TestServer {
    let server = TestServer::start().await;
    server.push(MockResponse::json(200, support::jwks_document()));
    server
}

#[tokio::test]
async fn verify_id_token_accepts_a_valid_token() {
    let server = jwks_server().await;
    let discovery = support::discovery_document(&server.base_url);

    let claims = json!({
        "iss": server.base_url,
        "aud": "client-1",
        "sub": "user-42",
        "email": "alice@acme.test",
        "email_verified": true,
        "nonce": "nonce-1",
        "iat": now(),
        "exp": now() + 300,
    });
    let id_token = support::sign_id_token(&claims);

    let verified = oidc::verify_id_token(&discovery, "client-1", &id_token, "nonce-1")
        .await
        .expect("a correctly signed, correctly claimed token verifies");

    assert_eq!(verified.sub, "user-42");
    assert_eq!(verified.email.as_deref(), Some("alice@acme.test"));
    assert_eq!(verified.email_verified, Some(true));
}

#[tokio::test]
async fn verify_id_token_reuses_a_cached_jwks_document() {
    // Deliberately deviates from `jwks_server()`'s helper: only ONE response
    // is queued, so a second live fetch would fail the request outright
    // (the mock server answers an unqueued request with 500). Two
    // successful verifications against this server is only possible if the
    // second call served the JWKS document from `oidc`'s cache instead of
    // fetching it again — proving the cache is actually consulted, not just
    // present and unused (a regression that quietly deleted the caching
    // would still compile and would still pass every other test here, since
    // none of them call `verify_id_token` twice against one server).
    let server = TestServer::start().await;
    server.push(MockResponse::json(200, support::jwks_document()));
    let discovery = support::discovery_document(&server.base_url);

    for nonce in ["nonce-1", "nonce-2"] {
        let claims = json!({
            "iss": server.base_url,
            "aud": "client-1",
            "sub": "user-42",
            "nonce": nonce,
            "exp": now() + 300,
        });
        let id_token = support::sign_id_token(&claims);

        oidc::verify_id_token(&discovery, "client-1", &id_token, nonce)
            .await
            .expect("cached JWKS still verifies a second, freshly signed token");
    }

    let jwks_requests = server
        .requests()
        .into_iter()
        .filter(|r| r.path == "/jwks")
        .count();
    assert_eq!(
        jwks_requests, 1,
        "the second verify_id_token call should have hit the cache, not refetched the JWKS document"
    );
}

#[tokio::test]
async fn verify_id_token_rejects_wrong_issuer() {
    let server = jwks_server().await;
    let discovery = support::discovery_document(&server.base_url);

    let claims = json!({
        "iss": "https://not-the-real-idp.test",
        "aud": "client-1",
        "sub": "user-42",
        "nonce": "nonce-1",
        "exp": now() + 300,
    });
    let id_token = support::sign_id_token(&claims);

    let err = oidc::verify_id_token(&discovery, "client-1", &id_token, "nonce-1")
        .await
        .expect_err("a token from a different issuer must be refused");
    assert_id_token_invalid(&err, "issuer");
}

#[tokio::test]
async fn verify_id_token_rejects_wrong_audience() {
    let server = jwks_server().await;
    let discovery = support::discovery_document(&server.base_url);

    let claims = json!({
        "iss": server.base_url,
        "aud": "some-other-client",
        "sub": "user-42",
        "nonce": "nonce-1",
        "exp": now() + 300,
    });
    let id_token = support::sign_id_token(&claims);

    let err = oidc::verify_id_token(&discovery, "client-1", &id_token, "nonce-1")
        .await
        .expect_err("a token issued for a different client_id must be refused");
    assert_id_token_invalid(&err, "audience");
}

#[tokio::test]
async fn verify_id_token_rejects_wrong_nonce() {
    let server = jwks_server().await;
    let discovery = support::discovery_document(&server.base_url);

    let claims = json!({
        "iss": server.base_url,
        "aud": "client-1",
        "sub": "user-42",
        "nonce": "a-different-nonce",
        "exp": now() + 300,
    });
    let id_token = support::sign_id_token(&claims);

    let err = oidc::verify_id_token(&discovery, "client-1", &id_token, "nonce-1")
        .await
        .expect_err("a replayed/mismatched nonce must be refused");
    assert_id_token_invalid(&err, "nonce");
}

#[tokio::test]
async fn verify_id_token_rejects_an_expired_token() {
    let server = jwks_server().await;
    let discovery = support::discovery_document(&server.base_url);

    let claims = json!({
        "iss": server.base_url,
        "aud": "client-1",
        "sub": "user-42",
        "nonce": "nonce-1",
        "exp": now() - 300,
    });
    let id_token = support::sign_id_token(&claims);

    let err = oidc::verify_id_token(&discovery, "client-1", &id_token, "nonce-1")
        .await
        .expect_err("an expired token must be refused");
    assert_id_token_invalid(&err, "expired");
}

#[tokio::test]
async fn verify_id_token_rejects_a_bad_signature() {
    let server = jwks_server().await;
    let discovery = support::discovery_document(&server.base_url);

    let claims = json!({
        "iss": server.base_url,
        "aud": "client-1",
        "sub": "user-42",
        "nonce": "nonce-1",
        "exp": now() + 300,
    });
    let mut id_token = support::sign_id_token(&claims);
    // Flip the last character of the signature segment.
    let flipped = match id_token.pop() {
        Some('a') => 'b',
        Some(_) => 'a',
        None => panic!("token unexpectedly empty"),
    };
    id_token.push(flipped);

    let err = oidc::verify_id_token(&discovery, "client-1", &id_token, "nonce-1")
        .await
        .expect_err("a tampered signature must be refused");
    assert!(matches!(err, AuthError::IdTokenInvalid(_)));
}

fn assert_id_token_invalid(err: &AuthError, expected_substring: &str) {
    match err {
        AuthError::IdTokenInvalid(message) => assert!(
            message.to_lowercase().contains(expected_substring),
            "expected {expected_substring:?} in the failure message, got {message:?}"
        ),
        other => panic!("expected AuthError::IdTokenInvalid, got {other:?}"),
    }
}
