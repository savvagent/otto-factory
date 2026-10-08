//! Test harness: the assembled router, driven the way a client drives it.
//!
//! Requests go through `tower::ServiceExt::oneshot` against the real router
//! rather than calling handlers directly, so extractors, path matching, method
//! routing, and status codes are all under test. A handler tested in isolation
//! cannot tell you that its route is mounted, that its extractor resolves, or
//! that its `404` is not a `403`.
//!
//! Identity is the platform's, so the harness stands up a mock of it
//! (`of_testkit::MockPlatform`) and "signs people in" by minting them platform
//! tokens. A fixture person ([`Account`]) holds one token that opens whichever
//! org they joined most recently; tests about *which* org a token opens use
//! explicit tokens from `h.platform.issue(..)`.

#![allow(dead_code)]

use axum::body::Body;
use axum::Router;
use base64::Engine;
use http::{Request, Response, StatusCode};
use of_testkit::MockPlatform;
use of_web::{AppState, Config};
use otto_resource::Role;
use otto_tenant::crypto::Cipher;
use otto_tenant::ids::{OrgId, UserId};
use otto_tenant::Db;
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;

pub const RESOURCE: &str = of_testkit::RESOURCE_URI;
pub const PUBLIC_URL: &str = "https://console.otto-factory.test";
pub const PLATFORM_URL: &str = "https://otto.test";

pub struct Harness {
    pub db: Db,
    pub router: Router,
    pub cipher: Cipher,
    pub platform: MockPlatform,
}

pub async fn harness(pool: PgPool) -> Harness {
    harness_with(pool, |_| {}).await
}

pub async fn harness_with(pool: PgPool, configure: impl FnOnce(&mut Config)) -> Harness {
    let db = Db::from_pool(pool);
    let platform = MockPlatform::start().await;
    let mut config = Config::new(
        PUBLIC_URL,
        RESOURCE,
        PLATFORM_URL,
        of_testkit::WEBHOOK_SECRET,
    );
    configure(&mut config);
    let state = AppState::new(db.clone(), cipher(), platform.client(), config);

    Harness {
        db,
        router: of_web::router(state),
        cipher: cipher(),
        platform,
    }
}

/// A harness whose deployment can actually take an admin through connecting a
/// tracker.
///
/// The plain [`harness`] configures no provider, which is a real deployment
/// shape worth testing (and what `a_deployment_that_cannot_connect_a_provider_says_so`
/// asserts) — but it means every connect request is refused for the deployment's
/// gap before the request itself is ever looked at. This one gets past that.
pub async fn harness_with_trackers(pool: PgPool) -> Harness {
    harness_with(pool, |config| {
        config.github_app_slug = Some("otto-factory".into());
        config.github_app_client_id = Some("gh-client".into());
        config.github_app_client_secret = Some("gh-secret".into());
        config.jira_client_id = Some("jira-client".into());
        config.jira_client_secret = Some("jira-secret".into());
    })
    .await
}

pub fn cipher() -> Cipher {
    Cipher::from_base64_key(&base64::engine::general_purpose::STANDARD.encode([9u8; 32])).unwrap()
}

/// A response, already read into memory so a test can assert on both the status
/// and the body without threading a body future through every assertion.
pub struct Reply {
    pub status: StatusCode,
    pub headers: http::HeaderMap,
    pub body: Value,
    /// The raw body, for the endpoints that answer with HTML.
    pub text: String,
}

impl Reply {
    pub fn error_code(&self) -> Option<&str> {
        self.body.get("error")?.get("code")?.as_str()
    }

    /// Assert a status, printing the body when it does not match — a bare
    /// "expected 200, got 400" from an API test is a test that wastes an hour.
    pub fn expect(&self, status: StatusCode) -> &Self {
        assert_eq!(
            self.status,
            status,
            "unexpected status; body was: {}",
            if self.text.is_empty() {
                "(empty)"
            } else {
                &self.text
            }
        );
        self
    }
}

/// A request under construction.
pub struct Call {
    method: http::Method,
    uri: String,
    session: Option<String>,
    body: Option<Body>,
    content_type: Option<&'static str>,
    headers: Vec<(&'static str, String)>,
}

impl Call {
    pub fn get(uri: impl Into<String>) -> Self {
        Self::new(http::Method::GET, uri)
    }
    pub fn post(uri: impl Into<String>) -> Self {
        Self::new(http::Method::POST, uri)
    }
    pub fn put(uri: impl Into<String>) -> Self {
        Self::new(http::Method::PUT, uri)
    }
    pub fn patch(uri: impl Into<String>) -> Self {
        Self::new(http::Method::PATCH, uri)
    }
    pub fn delete(uri: impl Into<String>) -> Self {
        Self::new(http::Method::DELETE, uri)
    }

    fn new(method: http::Method, uri: impl Into<String>) -> Self {
        Self {
            method,
            uri: uri.into(),
            session: None,
            body: None,
            content_type: None,
            headers: Vec::new(),
        }
    }

    /// An arbitrary request header.
    pub fn header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }

    pub fn json(mut self, body: Value) -> Self {
        self.body = Some(Body::from(serde_json::to_vec(&body).unwrap()));
        self.content_type = Some("application/json");
        self
    }

    /// A raw JSON body, byte for byte -- for the signed webhook, where
    /// re-serializing would change the bytes the signature covers.
    pub fn raw_json(mut self, body: Vec<u8>) -> Self {
        self.body = Some(Body::from(body));
        self.content_type = Some("application/json");
        self
    }

    /// Authenticate as the holder of this platform token.
    pub fn with_session(mut self, token: &str) -> Self {
        self.session = Some(token.to_string());
        self
    }

    pub async fn send(self, router: &Router) -> Reply {
        let mut builder = Request::builder().method(self.method).uri(&self.uri);

        if let Some(ct) = self.content_type {
            builder = builder.header(http::header::CONTENT_TYPE, ct);
        }
        if let Some(token) = &self.session {
            builder = builder.header(http::header::AUTHORIZATION, format!("Bearer {token}"));
        }
        for (name, value) in &self.headers {
            builder = builder.header(*name, value.as_str());
        }

        let request = builder.body(self.body.unwrap_or_else(Body::empty)).unwrap();
        let response: Response<Body> = router.clone().oneshot(request).await.unwrap();

        let status = response.status();
        let headers = response.headers().clone();
        let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&bytes).to_string();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);

        Reply {
            status,
            headers,
            body,
            text,
        }
    }
}

// ---------------------------------------------------------------------------
// Account fixtures
// ---------------------------------------------------------------------------

/// A fixture person: an id, an address, and a platform token that opens
/// whichever org they joined most recently.
pub struct Account {
    pub user: UserId,
    pub email: String,
    /// The bearer token. Named for the cookie it replaced.
    pub session: String,
}

/// Create a person. They belong to no org until [`org_with_owner`] or
/// [`add_member`] places them in one.
pub async fn onboard(h: &Harness, email: &str) -> Account {
    let user = UserId::new();
    let session = h
        .platform
        .issue_floating(user.as_uuid(), of_core::scopes::KNOWN);
    Account {
        user,
        email: email.to_string(),
        session,
    }
}

/// An org with `owner` as its owner.
pub async fn org_with_owner(h: &Harness, slug: &str, owner: &Account) -> OrgId {
    let org = OrgId::new();
    h.platform.add_org(org.as_uuid(), slug, slug);
    h.platform.add_member(
        org.as_uuid(),
        owner.user.as_uuid(),
        &owner.email,
        Role::Owner,
    );
    org
}

/// Put someone in an org, for tests about what a role may do rather than about
/// how someone got it. The email is derived from the id; use
/// [`add_member_as`] when a test cares about it.
pub async fn add_member(h: &Harness, org: OrgId, user: UserId, role: Role) {
    h.platform.add_member(
        org.as_uuid(),
        user.as_uuid(),
        &format!("{}@test.example", &user.to_string()[..8]),
        role,
    );
}

pub async fn add_member_as(h: &Harness, org: OrgId, who: &Account, role: Role) {
    h.platform
        .add_member(org.as_uuid(), who.user.as_uuid(), &who.email, role);
}
