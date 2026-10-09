//! The assembled application, exercised as one router.
//!
//! Everything below is about the seams *between* crates — the places a bug can
//! only exist once `of-web`, `of-mcp`, the health routes, and the console
//! bundle share an origin. Each crate's own behaviour is tested in that crate.

use axum::body::Body;
use of_core::watch::Watcher;
use of_server::config::LogFormat;
use of_server::Config;
use of_testkit::MockPlatform;
use otto_resource::Role;
use otto_tenant::Db;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tower::ServiceExt;

const PUBLIC: &str = "https://factory.test";
const RESOURCE: &str = of_testkit::RESOURCE_URI;
const PLATFORM: &str = "https://otto.test";

/// A 32-byte base64 key. Test material only.
const KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

fn config(static_dir: &str) -> Config {
    Config {
        database_url: "postgres://unused".into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        public_url: PUBLIC.into(),
        resource_uri: RESOURCE.into(),
        platform_url: PLATFORM.into(),
        introspection_secret: of_testkit::SECRET.into(),
        platform_webhook_secret: of_testkit::WEBHOOK_SECRET.into(),
        encryption_key: KEY.into(),
        github_app_id: None,
        github_app_private_key: None,
        github_app_webhook_secret: None,
        github_app_slug: None,
        github_app_client_id: None,
        github_app_client_secret: None,
        jira_client_id: None,
        jira_client_secret: None,
        console_client_id: None,
        enforce_quotas: false,
        upgrade_url: format!("{PLATFORM}/settings/billing"),
        extra_allowed_hosts: vec![],
        allowed_origins: vec![],
        static_dir: static_dir.into(),
        run_migrations: false,
        log_format: LogFormat::Text,
    }
}

/// A pool that will never connect. Port 1 refuses immediately rather than
/// hanging, so a test that wants a database failure gets one in milliseconds.
fn dead_pool() -> PgPool {
    PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy("postgres://localhost:1/nope")
        .expect("lazy pool")
}

/// A directory holding a stand-in `index.html`, removed on drop so a failing
/// assertion does not leave one behind.
struct Bundle(std::path::PathBuf);

impl Bundle {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("of-server-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("index.html"),
            "<!doctype html><title>console</title>",
        )
        .unwrap();
        Self(dir)
    }

    fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }
}

impl Drop for Bundle {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

async fn get(app: axum::Router, uri: &str) -> http::Response<Body> {
    app.oneshot(
        http::Request::builder()
            .uri(uri)
            .header("host", "factory.test")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap()
}

async fn body_json(response: http::Response<Body>) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), 256 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).expect("body was not JSON")
}

async fn body_text(response: http::Response<Body>) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), 256 * 1024)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// The whole router assembles. Axum panics on a route registered twice rather
/// than choosing, and without this test the panic is found by a deploy.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn the_whole_router_assembles(pool: PgPool) {
    let db = Db::from_pool(pool.clone());
    let platform = MockPlatform::start().await;
    let watcher = Watcher::spawn(pool).await.unwrap();

    let _app = of_server::router(db, watcher.clone(), platform.client(), &config("web/build"))
        .expect("router");

    watcher.shutdown().await;
}

/// The protected-resource document answers without a credential and names the
/// *platform* as the authorization server. This is the whole of zero-install
/// onboarding: an agent given nothing but the MCP URL reads its way from here to
/// the platform and on to a token. This service serves no authorization-server
/// metadata of its own.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn discovery_is_open_and_points_at_the_platform(pool: PgPool) {
    let db = Db::from_pool(pool.clone());
    let platform = MockPlatform::start().await;
    let watcher = Watcher::spawn(pool).await.unwrap();
    let app = of_server::router(db, watcher.clone(), platform.client(), &config("web/build"))
        .expect("router");

    let resource = get(app.clone(), "/.well-known/oauth-protected-resource").await;
    assert_eq!(resource.status(), http::StatusCode::OK);
    let doc = body_json(resource).await;
    assert_eq!(doc["resource"], RESOURCE);
    // The console's signed-out "Back to otto" link is built from exactly this
    // value. The signed-in side reads `/api/session`'s `platformUrl`:
    // `the_console_and_the_discovery_document_name_the_same_platform` pins the
    // config that feeds it, and `of-web`'s `console_login.rs` checks the served
    // value against the platform.
    assert_eq!(doc["authorization_servers"][0], PLATFORM);

    // Not an authorization server any more, and says so as JSON.
    for path in [
        "/.well-known/oauth-authorization-server",
        "/oauth/authorize",
        "/oauth/token",
        "/oauth/register",
        "/sso/callback",
    ] {
        let response = get(app.clone(), path).await;
        assert!(
            response.status() == http::StatusCode::NOT_FOUND
                || response.status() == http::StatusCode::OK,
            "{path}"
        );
        if path.starts_with("/oauth") || path.starts_with("/.well-known") {
            assert_eq!(response.status(), http::StatusCode::NOT_FOUND, "{path}");
            assert_eq!(body_json(response).await["error"], "not_found", "{path}");
        }
    }

    watcher.shutdown().await;
}

/// The `401` challenge points at a document served on this same origin. It is
/// built from `OF_PUBLIC_URL`, so a deployment that gets that wrong sends every
/// client somewhere that does not answer — and nothing else would notice.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn the_mcp_endpoint_points_an_unauthenticated_caller_at_this_origin(pool: PgPool) {
    let db = Db::from_pool(pool.clone());
    let platform = MockPlatform::start().await;
    let watcher = Watcher::spawn(pool).await.unwrap();
    let app = of_server::router(db, watcher.clone(), platform.client(), &config("web/build"))
        .expect("router");

    let response = app
        .clone()
        .oneshot(
            http::Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("host", "factory.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), http::StatusCode::UNAUTHORIZED);
    let challenge = response
        .headers()
        .get(http::header::WWW_AUTHENTICATE)
        .expect("no WWW-Authenticate header")
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        challenge.contains(&format!(
            r#"resource_metadata="{PUBLIC}/.well-known/oauth-protected-resource""#
        )),
        "{challenge}"
    );

    // And the document it names is reachable here, unauthenticated. A pointer
    // whose target lives on another origin is a closed loop for the client.
    let pointed_at = get(app, "/.well-known/oauth-protected-resource").await;
    assert_eq!(pointed_at.status(), http::StatusCode::OK);

    watcher.shutdown().await;
}

/// The three surfaces that need the platform, through the one assembled
/// router: a real token opens the agent surface and the console API, and the
/// platform's signed webhook reaches its handler. Proves the pieces share one
/// platform client and one database without any crate's own test seeing the seam.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn one_platform_token_works_on_every_surface(pool: PgPool) {
    let db = Db::from_pool(pool.clone());
    let platform = MockPlatform::start().await;
    let watcher = Watcher::spawn(pool).await.unwrap();
    let app = of_server::router(db, watcher.clone(), platform.client(), &config("web/build"))
        .expect("router");

    let (org, user) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
    platform.add_org(org, "acme", "Acme");
    platform.add_member(org, user, "rob@acme.test", Role::Owner);
    let token = platform.issue(org, user, Role::Owner, of_core::scopes::KNOWN);

    // Console API.
    let console = app
        .clone()
        .oneshot(
            http::Request::builder()
                .uri("/api/orgs/acme/repos")
                .header("host", "factory.test")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(console.status(), http::StatusCode::OK);

    // Agent surface: past the bearer middleware (the empty body is then refused
    // by the MCP transport, which is not a 401 or a 503).
    let mcp = app
        .clone()
        .oneshot(
            http::Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("host", "factory.test")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(mcp.status(), http::StatusCode::UNAUTHORIZED);
    assert_ne!(mcp.status(), http::StatusCode::SERVICE_UNAVAILABLE);

    // Platform webhook: unsigned is refused, signed is accepted.
    let unsigned = app
        .clone()
        .oneshot(
            http::Request::builder()
                .method("POST")
                .uri("/platform/webhooks")
                .header("host", "factory.test")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unsigned.status(), http::StatusCode::UNAUTHORIZED);

    let (header, body) = MockPlatform::webhook(
        "member.removed",
        serde_json::json!({ "org_id": org, "user_id": user }),
    );
    let signed = app
        .oneshot(
            http::Request::builder()
                .method("POST")
                .uri("/platform/webhooks")
                .header("host", "factory.test")
                .header("otto-signature", header)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(signed.status(), http::StatusCode::OK);

    watcher.shutdown().await;
}

/// Liveness must not depend on the database, or one database blip restarts
/// every replica at once. Asked against a pool that cannot connect.
#[tokio::test]
async fn liveness_does_not_touch_the_database() {
    let app = of_server::health::router(Db::from_pool(dead_pool()));

    let response = get(app, "/healthz").await;
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(body_json(response).await["status"], "ok");
}

/// Readiness must, or a replica that can serve nothing keeps taking traffic.
#[tokio::test]
async fn readiness_fails_when_the_database_is_unreachable() {
    let app = of_server::health::router(Db::from_pool(dead_pool()));

    let response = get(app, "/readyz").await;
    assert_eq!(response.status(), http::StatusCode::SERVICE_UNAVAILABLE);
    let doc = body_json(response).await;
    assert_eq!(doc["status"], "unready");
    // The reason distinguishes a refused connection from a timeout, because
    // "not ready" alone sends whoever is paged to look at the wrong thing.
    assert!(doc["reason"].is_string(), "{doc}");
}

#[sqlx::test(migrations = "../of-core/migrations")]
async fn readiness_passes_against_a_live_database(pool: PgPool) {
    let db = Db::from_pool(pool);
    let app = of_server::health::router(db);

    let response = get(app, "/readyz").await;
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(body_json(response).await["status"], "ready");
}

/// The console's `index.html` answers any path the router does not, which is
/// what makes a hard refresh of a deep link work — and must *not* answer for an
/// API path, where `200 text/html` is the shape that makes an agent retry
/// forever against a route that will never exist.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn the_console_fallback_stops_at_the_api(pool: PgPool) {
    let bundle = Bundle::new("spa");
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool.clone());
    let watcher = Watcher::spawn(pool).await.unwrap();
    let app = of_server::router(
        db,
        watcher.clone(),
        platform.client(),
        &config(bundle.path()),
    )
    .expect("router");

    // A console route the server has never heard of renders the app.
    let deep_link = get(app.clone(), "/o/acme/queue").await;
    assert_eq!(deep_link.status(), http::StatusCode::OK);
    assert!(body_text(deep_link)
        .await
        .contains("<title>console</title>"));

    // An API route that does not exist is a JSON 404 an agent can read.
    // Paths that match no route at all — `/api/orgs/nope` would be a `401`,
    // because it *is* a route and an unauthenticated caller is turned away
    // before anyone asks whether the org exists.
    for path in [
        "/api/no/such/thing",
        "/oauth/nope",
        // The console's own sign-in routes are API-shaped too: a mistyped one is
        // a JSON 404, never `index.html` served into a redirect.
        "/auth/nope",
        "/.well-known/nope",
        "/platform/nope",
    ] {
        let response = get(app.clone(), path).await;
        assert_eq!(
            response.status(),
            http::StatusCode::NOT_FOUND,
            "{path} should be a 404"
        );
        assert_eq!(body_json(response).await["error"], "not_found", "{path}");
    }

    // A console path that merely shares a prefix with an API one is still the
    // console: `apiary` is a legal org slug.
    for path in ["/apiary", "/authors"] {
        let lookalike = get(app.clone(), path).await;
        assert_eq!(lookalike.status(), http::StatusCode::OK, "{path}");
        assert!(body_text(lookalike)
            .await
            .contains("<title>console</title>"));
    }

    watcher.shutdown().await;
}
