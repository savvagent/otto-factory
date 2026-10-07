//! `of-server` — one binary, one port, every surface.
//!
//! The crates are a compile-time layering discipline, not separate services, so
//! this is where they stop being libraries:
//!
//! ```text
//!   /healthz /readyz          health      no database on the liveness path
//!   /api/…                    of-web      the console API: platform bearer tokens
//!   /webhooks/…               of-web      tracker deliveries, provider-signed
//!   /platform/webhooks        of-web      the platform's lifecycle events, signed
//!   /.well-known/…            of-mcp      discovery, open by necessity
//!   /mcp                      of-mcp      bearer tokens, the agent surface
//!   everything else           web/build   the console SPA, index.html fallback
//! ```
//!
//! otto-factory is a resource server of the otto platform, which is the OAuth
//! authorization server and the system of record for identity and billing. There
//! is no `/oauth/*`, login, or account surface here; both HTTP surfaces validate
//! the platform's tokens by introspection.
//!
//! Assembly is a library function rather than something buried in `main` so a
//! test can build the whole router. Axum panics on a route registered twice, and
//! a panic at startup is only a good failure if something other than a
//! deployment reaches it first.

pub mod config;
pub mod health;

use std::convert::Infallible;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use of_core::watch::Watcher;
use otto_resource::{ClientConfig, PlatformClient};
use otto_tenant::Db;
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

pub use config::{Config, LogFormat};

/// Path prefixes that belong to an API rather than to the console's routing.
///
/// Everything else falls through to the single-page app, which is what makes a
/// hard refresh of `/o/acme/queue` work. An unmatched path under one of these
/// must not: a client that `GET`s `/api/orgs/nope` needs a `404` it can parse,
/// and `200 text/html` is the shape that makes an agent retry forever against a
/// route that will never exist. `/oauth` is listed although nothing is served
/// there any more: an old client that still tries it should get a JSON `404`
/// naming where authorization lives now, not a console page.
const API_PREFIXES: [&str; 6] = [
    "/api",
    "/oauth",
    "/mcp",
    "/.well-known",
    "/platform",
    "/webhooks",
];

fn is_api_path(path: &str) -> bool {
    API_PREFIXES
        .iter()
        .any(|prefix| path == *prefix || path.starts_with(&format!("{prefix}/")))
}

/// The client every call to the otto platform goes through.
///
/// Built once and shared: it holds the introspection and usage-status caches,
/// so a second client would be a second, colder cache. Fallible because the
/// platform URL is only a `String` until something parses it.
pub fn platform_client(config: &Config) -> Result<Arc<PlatformClient>> {
    let client = PlatformClient::new(ClientConfig::new(
        &config.platform_url,
        &config.resource_uri,
        &config.introspection_secret,
    ))
    .context("OF_PLATFORM_URL is not usable")?;
    Ok(Arc::new(client))
}

/// Build the whole application.
///
/// Fallible because the encryption key is only a `String` until something tries
/// to use it. `Config::from_env` cannot prove the key parses without building a
/// `Cipher`, and a `Config` assembled by hand — a test, a future binary — need
/// not have come from the environment at all. Returning the error keeps this
/// crate's rule that a bad value is a named failure and never a panic.
pub fn router(
    db: Db,
    watcher: Arc<Watcher>,
    platform: Arc<PlatformClient>,
    config: &Config,
) -> Result<Router> {
    let web = of_web::router(web_state(db.clone(), platform.clone(), config)?);

    // The agent surface, plus `/.well-known/oauth-protected-resource`.
    let mcp = of_mcp::router(db.clone(), watcher, mcp_config(platform, config));

    Ok(health::router(db)
        .merge(web)
        .merge(mcp)
        .fallback_service(console(config))
        // Request spans, without headers. `DefaultMakeSpan::include_headers`
        // would put `Authorization` in the logs — every bearer token, in
        // plaintext, in whatever the log aggregator retains. Do not turn it on.
        .layer(TraceLayer::new_for_http()))
}

/// `of-web`'s state, with the settings that are this deployment's to decide.
fn web_state(db: Db, platform: Arc<PlatformClient>, config: &Config) -> Result<of_web::AppState> {
    let cipher = otto_tenant::crypto::Cipher::from_base64_key(&config.encryption_key)
        .context("OF_ENCRYPTION_KEY is not a valid 32-byte base64 key")?;

    Ok(of_web::AppState::new(
        db,
        cipher,
        platform,
        web_config(config),
    ))
}

/// Every setting `of-web` takes from this deployment, in one place so it can be
/// tested without a database.
///
/// Split out because the failure mode is silence: a field added to
/// `of_web::Config` that nothing here assigns keeps its `Default`, and the
/// console then reports that default as fact.
fn web_config(config: &Config) -> of_web::Config {
    let mut web = of_web::Config::new(
        &config.public_url,
        &config.resource_uri,
        &config.platform_url,
        &config.platform_webhook_secret,
    );
    web.github_app_webhook_secret = config.github_app_webhook_secret.clone();
    // The tracker console needs all five to take an admin through connecting a
    // provider: the slug and the OAuth pair for GitHub, the OAuth pair for JIRA.
    web.github_app_slug = config.github_app_slug.clone();
    web.github_app_client_id = config.github_app_client_id.clone();
    web.github_app_client_secret = config.github_app_client_secret.clone();
    web.jira_client_id = config.jira_client_id.clone();
    web.jira_client_secret = config.jira_client_secret.clone();
    web
}

fn mcp_config(platform: Arc<PlatformClient>, config: &Config) -> of_mcp::Config {
    let mut mcp = of_mcp::Config::new(
        platform,
        &config.platform_url,
        &config.resource_uri,
        &config.public_url,
    );
    mcp.allowed_hosts = config.allowed_hosts();
    mcp.allowed_origins = config.allowed_origins.clone();
    mcp.enforce_quotas = config.enforce_quotas;
    mcp.upgrade_url = config.upgrade_url.clone();
    mcp.github_app_id = config.github_app_id;
    mcp.github_app_private_key = config.github_app_private_key.clone();
    mcp.jira_client_id = config.jira_client_id.clone();
    mcp.jira_client_secret = config.jira_client_secret.clone();
    mcp.encryption_key = Some(config.encryption_key.clone());
    mcp
}

/// The console bundle, or a JSON `404` for anything API-shaped.
///
/// `ServeDir` falls back to `index.html` for any path it has no file for, which
/// is what `adapter-static` produces and what client-side routing needs. That
/// fallback is exactly why the API prefixes are checked first.
fn console(
    config: &Config,
) -> impl tower::Service<Request<Body>, Response = Response, Error = Infallible, Future = impl Send>
       + Clone
       + Send
       + 'static {
    let assets = ServeDir::new(&config.static_dir)
        .append_index_html_on_directories(true)
        .fallback(ServeFile::new(config.static_dir.join("index.html")));

    tower::service_fn(move |req: Request<Body>| {
        let assets = assets.clone();
        async move {
            if is_api_path(req.uri().path()) {
                return Ok(not_found(req.uri().path()));
            }
            Ok(assets
                .oneshot(req)
                .await
                .map(|res| res.map(Body::new))
                .into_response())
        }
    })
}

fn not_found(path: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        axum::Json(serde_json::json!({
            "error": "not_found",
            "error_description":
                format!("no route serves {path}. See /api/openapi.json for the console API, \
                         and /.well-known/oauth-protected-resource for the MCP endpoint and the \
                         authorization server (the otto platform) that issues its tokens."),
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A setting that `of-server` reads but never passes on is invisible: the
    /// console runs on `of_web::Config`'s default and calls it fact.
    #[test]
    fn every_deployment_setting_reaches_of_web() {
        let mut config = Config::for_test();
        config.github_app_webhook_secret = Some("webhook-secret".into());
        config.github_app_slug = Some("otto-factory".into());
        config.github_app_client_id = Some("gh-client".into());
        config.github_app_client_secret = Some("gh-secret".into());
        config.jira_client_id = Some("jira-client".into());
        config.jira_client_secret = Some("jira-secret".into());
        config.enforce_quotas = true;

        let web = web_config(&config);

        assert_eq!(
            web.github_app_webhook_secret.as_deref(),
            Some("webhook-secret")
        );
        assert_eq!(web.platform_url, config.platform_url);
        assert_eq!(web.platform_webhook_secret, config.platform_webhook_secret);
        assert!(
            web.github_tracker_configured() && web.jira_tracker_configured(),
            "the tracker console cannot connect a provider it was never told the credentials for"
        );
        assert_eq!(web.public_url, config.public_url);
        assert_eq!(web.resource_uri, config.resource_uri);
    }

    #[test]
    fn every_tracker_setting_reaches_of_mcp() {
        let mut config = Config::for_test();
        config.github_app_id = Some(42);
        config.github_app_private_key = Some("pem".into());
        config.jira_client_id = Some("jira-client".into());
        config.jira_client_secret = Some("jira-secret".into());

        let platform = platform_client(&config).expect("platform client");
        let mcp = mcp_config(platform, &config);

        assert_eq!(mcp.github_app_id, Some(42));
        assert_eq!(mcp.github_app_private_key.as_deref(), Some("pem"));
        assert_eq!(mcp.jira_client_id.as_deref(), Some("jira-client"));
        assert_eq!(mcp.jira_client_secret.as_deref(), Some("jira-secret"));
        assert_eq!(mcp.encryption_key.as_deref(), Some("k"));
    }

    #[test]
    fn api_prefixes_do_not_match_by_string_prefix_alone() {
        assert!(is_api_path("/api"));
        assert!(is_api_path("/api/orgs/acme"));
        assert!(is_api_path("/mcp"));
        assert!(is_api_path("/.well-known/oauth-protected-resource"));
        assert!(is_api_path("/platform/webhooks"));
        assert!(is_api_path("/webhooks/github"));

        // An org slug or a page name that merely starts with the same letters
        // belongs to the console. `/apiary` is a legal org route and must not
        // answer a JSON 404 that the SPA never gets to render.
        assert!(!is_api_path("/apiary"));
        assert!(!is_api_path("/mcp-guide"));
        assert!(!is_api_path("/o/acme/queue"));
        assert!(!is_api_path("/"));
    }
}
