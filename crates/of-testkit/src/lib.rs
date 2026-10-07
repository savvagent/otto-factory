//! An in-process stand-in for the otto platform's resource-server API.
//!
//! Integration tests that exercise introspection, the internal lookups, usage
//! shipping, or the lifecycle webhooks run against [`MockPlatform`], a real HTTP
//! server on a loopback port speaking the same wire format as
//! `otto-platform-server` (it reuses `otto-resource`'s own wire types, so the two
//! cannot drift silently). Hand a test the real [`PlatformClient`] from
//! [`MockPlatform::client`] and the code under test cannot tell the difference.
//!
//! What it models, because the tests depend on it:
//!
//! - **Audience.** A token is minted for one `resource_uri`; the mock introspects
//!   it as active only for a caller whose HTTP Basic client id is that URI, like
//!   the platform does. [`MockPlatform::issue_for`] mints one for a *different*
//!   resource server to prove that is refused.
//! - **Outages.** [`MockPlatform::set_down`] makes every endpoint answer `503`,
//!   which is how "platform down means 503, never 401" is tested.
//! - **Usage dedupe.** `POST /internal/usage` counts an `event_id` once and
//!   reports repeats as duplicates, so "ships exactly once" is checkable by
//!   counting what [`MockPlatform::counted_usage`] holds.
//! - **Webhook signing.** [`MockPlatform::webhook`] produces a signed delivery
//!   (header + raw body) for a given event.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use base64::Engine;
use chrono::{DateTime, NaiveDate, Utc};
use otto_resource::{
    ClientConfig, IntrospectionResponse, MemberInfo, OrgInfo, PlatformClient, RejectedEvent, Role,
    TeamInfo, TokenKind, UsageBatch, UsageEvent, UsageReceipt, UsageStatus, UserInfo,
};
use serde::Deserialize;
use uuid::Uuid;

pub use otto_resource;

/// The `resource_uri` the mock registers its default resource server under.
pub const RESOURCE_URI: &str = "https://factory.test/mcp";
/// The introspection credential the mock expects.
pub const SECRET: &str = "otto_rs_test_secret";
/// The key lifecycle webhooks are signed with.
pub const WEBHOOK_SECRET: &str = "otto_whsec_test_secret";

#[derive(Clone)]
struct TokenRecord {
    jti: Uuid,
    /// `None` for a [`MockPlatform::issue_floating`] token: it opens whichever
    /// org its user joined most recently, resolved at introspection time.
    org: Option<Uuid>,
    user: Uuid,
    scopes: Vec<String>,
    resource: String,
    exp: i64,
    kind: TokenKind,
}

#[derive(Default)]
struct Data {
    tokens: HashMap<String, TokenRecord>,
    orgs: HashMap<Uuid, OrgInfo>,
    /// (org, user) -> (info, role, join order)
    members: HashMap<(Uuid, Uuid), (UserInfo, Role, u64)>,
    joins: u64,
    teams: HashMap<(Uuid, Uuid), TeamInfo>,
    usage: HashMap<Uuid, UsageStatus>,
    counted: Vec<UsageEvent>,
    seen: HashSet<Uuid>,
    /// Org ids whose usage the mock refuses, like a deleted org.
    unknown_orgs_for_usage: HashSet<Uuid>,
}

struct Inner {
    data: Mutex<Data>,
    down: AtomicBool,
    /// Fail the next N usage POSTs with 503, then recover.
    fail_usage: AtomicUsize,
    introspect_calls: AtomicUsize,
    usage_calls: AtomicUsize,
    lookup_calls: AtomicUsize,
}

pub struct MockPlatform {
    pub url: String,
    inner: Arc<Inner>,
    _task: tokio::task::JoinHandle<()>,
}

impl MockPlatform {
    /// Start the mock on a free loopback port.
    pub async fn start() -> Self {
        let inner = Arc::new(Inner {
            data: Mutex::new(Data::default()),
            down: AtomicBool::new(false),
            fail_usage: AtomicUsize::new(0),
            introspect_calls: AtomicUsize::new(0),
            usage_calls: AtomicUsize::new(0),
            lookup_calls: AtomicUsize::new(0),
        });

        let app = Router::new()
            .route("/oauth/introspect", post(introspect))
            .route("/internal/usage", post(ingest_usage))
            .route("/internal/orgs/{org}/usage-status", get(usage_status))
            .route("/internal/orgs/{org}/members/by-email", get(member_by_email))
            .route("/internal/orgs/{org}/members/{user}", get(member))
            .route("/internal/orgs/{org}/teams/by-slug/{slug}", get(team_by_slug))
            .route("/internal/orgs/{org}/teams/{team}", get(team))
            .with_state(inner.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the mock platform");
        let addr: SocketAddr = listener.local_addr().expect("local addr");
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("mock platform");
        });

        Self {
            url: format!("http://{addr}"),
            inner,
            _task: task,
        }
    }

    /// A real [`PlatformClient`] pointed at this mock, authenticating as the
    /// default resource server.
    pub fn client(&self) -> Arc<PlatformClient> {
        self.client_with(ClientConfig::new(&self.url, RESOURCE_URI, SECRET))
    }

    pub fn client_with(&self, cfg: ClientConfig) -> Arc<PlatformClient> {
        Arc::new(PlatformClient::new(cfg).expect("build the platform client"))
    }

    // ----------------------------------------------------------------- world

    /// Register an org (and make it known to usage and lookups).
    pub fn add_org(&self, org: Uuid, slug: &str, name: &str) {
        let mut d = self.inner.data.lock().unwrap();
        d.orgs.insert(
            org,
            OrgInfo {
                id: org,
                slug: slug.into(),
                name: name.into(),
                plan: "free".into(),
            },
        );
        d.usage.entry(org).or_insert_with(|| UsageStatus {
            org_id: org,
            plan: "free".into(),
            period_start: NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
            billable_count: 0,
            total_count: 0,
            included_ops: 500,
            hard_stop: true,
        });
    }

    pub fn add_member(&self, org: Uuid, user: Uuid, email: &str, role: Role) {
        let mut d = self.inner.data.lock().unwrap();
        d.joins += 1;
        let order = d.joins;
        d.members.insert(
            (org, user),
            (
                UserInfo {
                    id: user,
                    email: Some(email.into()),
                    name: Some(email.split('@').next().unwrap_or(email).into()),
                },
                role,
                order,
            ),
        );
    }

    /// Change a member's role.
    pub fn set_role(&self, org: Uuid, user: Uuid, role: Role) {
        if let Some(m) = self.inner.data.lock().unwrap().members.get_mut(&(org, user)) {
            m.1 = role;
        }
    }

    pub fn remove_member(&self, org: Uuid, user: Uuid) {
        self.inner.data.lock().unwrap().members.remove(&(org, user));
    }

    pub fn add_team(&self, org: Uuid, team: Uuid, slug: &str, name: &str) {
        self.inner.data.lock().unwrap().teams.insert(
            (org, team),
            TeamInfo {
                id: team,
                org_id: org,
                slug: slug.into(),
                name: name.into(),
            },
        );
    }

    pub fn remove_team(&self, org: Uuid, team: Uuid) {
        self.inner.data.lock().unwrap().teams.remove(&(org, team));
    }

    /// Set an org's usage standing.
    pub fn set_usage(&self, org: Uuid, billable: i64, included: i64, hard_stop: bool) {
        let mut d = self.inner.data.lock().unwrap();
        let entry = d.usage.entry(org).or_insert_with(|| UsageStatus {
            org_id: org,
            plan: "free".into(),
            period_start: NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
            billable_count: 0,
            total_count: 0,
            included_ops: 0,
            hard_stop,
        });
        entry.billable_count = billable;
        entry.total_count = billable;
        entry.included_ops = included;
        entry.hard_stop = hard_stop;
    }

    /// Make the platform refuse usage for an org the way it refuses a deleted one.
    pub fn reject_usage_for(&self, org: Uuid) {
        self.inner
            .data
            .lock()
            .unwrap()
            .unknown_orgs_for_usage
            .insert(org);
    }

    // ---------------------------------------------------------------- tokens

    /// Mint an OAuth token for the default resource server that opens `org` as
    /// `user`. Also registers the membership (with `role`) and the org if the
    /// mock has not seen them, so one call gives a working caller.
    pub fn issue(&self, org: Uuid, user: Uuid, role: Role, scopes: &[&str]) -> String {
        self.issue_for(RESOURCE_URI, org, user, role, scopes)
    }

    /// As [`Self::issue`], for an arbitrary resource server's audience.
    pub fn issue_for(
        &self,
        resource: &str,
        org: Uuid,
        user: Uuid,
        role: Role,
        scopes: &[&str],
    ) -> String {
        {
            let mut d = self.inner.data.lock().unwrap();
            d.orgs.entry(org).or_insert_with(|| OrgInfo {
                id: org,
                slug: format!("org-{}", &org.to_string()[..8]),
                name: "Test Org".into(),
                plan: "free".into(),
            });
            if !d.members.contains_key(&(org, user)) {
                d.joins += 1;
                let order = d.joins;
                d.members.insert(
                    (org, user),
                    (
                        UserInfo {
                            id: user,
                            email: Some(format!("{}@test.example", &user.to_string()[..8])),
                            name: None,
                        },
                        role,
                        order,
                    ),
                );
            }
            d.members.get_mut(&(org, user)).unwrap().1 = role;
            d.usage.entry(org).or_insert_with(|| UsageStatus {
                org_id: org,
                plan: "free".into(),
                period_start: NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
                billable_count: 0,
                total_count: 0,
                included_ops: 500,
                hard_stop: true,
            });
        }
        let token = new_token();
        self.inner.data.lock().unwrap().tokens.insert(
            token.clone(),
            TokenRecord {
                jti: Uuid::new_v4(),
                org: Some(org),
                user,
                scopes: scopes.iter().map(|s| s.to_string()).collect(),
                resource: resource.into(),
                exp: (Utc::now() + chrono::Duration::hours(1)).timestamp(),
                kind: TokenKind::Oauth,
            },
        );
        token
    }

    /// A token for `user` that opens whichever org they joined most recently,
    /// resolved when it is introspected. **Not how the real platform works** (a
    /// real token is fixed to one org when it is issued); it exists so a fixture
    /// can mint one credential per person and then place them in orgs, the way
    /// the console tests were written. Tests about *which org a token opens* use
    /// [`Self::issue`], which is faithful.
    pub fn issue_floating(&self, user: Uuid, scopes: &[&str]) -> String {
        let token = new_token();
        self.inner.data.lock().unwrap().tokens.insert(
            token.clone(),
            TokenRecord {
                jti: Uuid::new_v4(),
                org: None,
                user,
                scopes: scopes.iter().map(|s| s.to_string()).collect(),
                resource: RESOURCE_URI.into(),
                exp: (Utc::now() + chrono::Duration::hours(1)).timestamp(),
                kind: TokenKind::Oauth,
            },
        );
        token
    }

    /// Revoke a token at the platform (its next introspection is inactive).
    pub fn revoke(&self, token: &str) {
        self.inner.data.lock().unwrap().tokens.remove(token);
    }

    // --------------------------------------------------------------- failures

    /// While down, every endpoint answers `503`.
    pub fn set_down(&self, down: bool) {
        self.inner.down.store(down, Ordering::SeqCst);
    }

    /// Fail the next `n` usage POSTs with `503`, then recover.
    pub fn fail_next_usage_posts(&self, n: usize) {
        self.inner.fail_usage.store(n, Ordering::SeqCst);
    }

    // ------------------------------------------------------------ observation

    /// Usage events the platform has counted, each `event_id` once.
    pub fn counted_usage(&self) -> Vec<UsageEvent> {
        self.inner.data.lock().unwrap().counted.clone()
    }

    pub fn introspect_calls(&self) -> usize {
        self.inner.introspect_calls.load(Ordering::SeqCst)
    }

    pub fn usage_calls(&self) -> usize {
        self.inner.usage_calls.load(Ordering::SeqCst)
    }

    /// Calls to the `/internal/orgs/...` lookup endpoints.
    pub fn lookup_calls(&self) -> usize {
        self.inner.lookup_calls.load(Ordering::SeqCst)
    }

    // --------------------------------------------------------------- webhooks

    /// A signed delivery for `kind` with `data`: `(Otto-Signature header, raw
    /// body)`. Signed now, with [`WEBHOOK_SECRET`].
    pub fn webhook(kind: &str, data: serde_json::Value) -> (String, Vec<u8>) {
        Self::webhook_with_id(Uuid::new_v4(), kind, data)
    }

    /// As [`Self::webhook`], with a chosen event id, for redelivery tests.
    pub fn webhook_with_id(id: Uuid, kind: &str, data: serde_json::Value) -> (String, Vec<u8>) {
        let body = otto_resource::webhook::body(id, kind, Utc::now(), &data);
        let header = otto_resource::webhook::sign(WEBHOOK_SECRET, Utc::now().timestamp(), &body);
        (header, body)
    }
}

/// A token string with the shape the client accepts (`otto_at_` + 43 chars).
pub fn new_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!(
        "otto_at_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    )
}

// ------------------------------------------------------------------ handlers

type S = State<Arc<Inner>>;

fn unavailable() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, "mock platform is down").into_response()
}

/// Returns the calling resource server's `resource_uri`, or the response to send.
#[allow(clippy::result_large_err)]
fn caller(inner: &Inner, headers: &HeaderMap) -> Result<String, Response> {
    if inner.down.load(Ordering::SeqCst) {
        return Err(unavailable());
    }
    let unauth = || (StatusCode::UNAUTHORIZED, "bad credential").into_response();
    let raw = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(unauth)?;
    let b64 = raw.strip_prefix("Basic ").ok_or_else(unauth)?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|_| unauth())?;
    let decoded = String::from_utf8(decoded).map_err(|_| unauth())?;
    let (user, secret) = decoded.split_once(':').ok_or_else(unauth)?;
    let uri = percent_encoding::percent_decode_str(user)
        .decode_utf8()
        .map_err(|_| unauth())?
        .into_owned();
    if secret != SECRET {
        return Err(unauth());
    }
    Ok(uri)
}

#[derive(Deserialize)]
struct IntrospectForm {
    token: Option<String>,
}

async fn introspect(
    State(inner): S,
    headers: HeaderMap,
    Form(form): Form<IntrospectForm>,
) -> Response {
    let resource = match caller(&inner, &headers) {
        Ok(r) => r,
        Err(res) => return res,
    };
    inner.introspect_calls.fetch_add(1, Ordering::SeqCst);

    let d = inner.data.lock().unwrap();
    let token = form.token.unwrap_or_default();
    let body = match d.tokens.get(token.trim()) {
        // Audience: a token for another resource server is just inactive.
        Some(t) if t.resource == resource && t.exp > Utc::now().timestamp() => {
            let org = t.org.or_else(|| {
                d.members
                    .iter()
                    .filter(|((_, u), _)| *u == t.user)
                    .max_by_key(|(_, (_, _, order))| *order)
                    .map(|((o, _), _)| *o)
            });
            // Active also means the user is still a member; the role is today's.
            match org.and_then(|o| d.members.get(&(o, t.user)).map(|m| (o, m))) {
                Some((org, (_, role, _))) => IntrospectionResponse {
                    active: true,
                    sub: Some(t.user),
                    org_id: Some(org),
                    role: Some(*role),
                    scope: Some(t.scopes.join(" ")),
                    aud: Some(t.resource.clone()),
                    exp: Some(t.exp),
                    client_id: Some("otto_client_test".into()),
                    token_type: Some("Bearer".into()),
                    token_kind: Some(t.kind),
                    jti: Some(t.jti),
                },
                None => IntrospectionResponse::inactive(),
            }
        }
        _ => IntrospectionResponse::inactive(),
    };
    Json(body).into_response()
}

async fn ingest_usage(State(inner): S, headers: HeaderMap, Json(batch): Json<UsageBatch>) -> Response {
    if let Err(res) = caller(&inner, &headers) {
        return res;
    }
    inner.usage_calls.fetch_add(1, Ordering::SeqCst);
    if inner
        .fail_usage
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
        .is_ok()
    {
        return unavailable();
    }

    let mut d = inner.data.lock().unwrap();
    let mut receipt = UsageReceipt::default();
    for ev in batch.events {
        if d.unknown_orgs_for_usage.contains(&ev.org_id) {
            receipt.rejected.push(RejectedEvent {
                event_id: ev.event_id,
                reason: format!("org {} does not exist", ev.org_id),
            });
        } else if d.seen.insert(ev.event_id) {
            receipt.accepted += 1;
            d.counted.push(ev);
        } else {
            receipt.duplicates += 1;
        }
    }
    Json(receipt).into_response()
}

async fn usage_status(State(inner): S, headers: HeaderMap, Path(org): Path<Uuid>) -> Response {
    if let Err(res) = caller(&inner, &headers) {
        return res;
    }
    inner.lookup_calls.fetch_add(1, Ordering::SeqCst);
    let d = inner.data.lock().unwrap();
    match d.usage.get(&org) {
        Some(u) => {
            // The platform's own count includes what was shipped to it.
            let mut u = u.clone();
            let shipped_billable = d
                .counted
                .iter()
                .filter(|e| e.org_id == org && e.billable)
                .count() as i64;
            let shipped_total = d.counted.iter().filter(|e| e.org_id == org).count() as i64;
            u.billable_count += shipped_billable;
            u.total_count += shipped_total;
            Json(u).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn member_info(d: &Data, org: Uuid, user: Uuid) -> Option<MemberInfo> {
    let (u, role, _) = d.members.get(&(org, user))?;
    Some(MemberInfo {
        user: u.clone(),
        org: d.orgs.get(&org)?.clone(),
        role: *role,
    })
}

async fn member(
    State(inner): S,
    headers: HeaderMap,
    Path((org, user)): Path<(Uuid, Uuid)>,
) -> Response {
    if let Err(res) = caller(&inner, &headers) {
        return res;
    }
    inner.lookup_calls.fetch_add(1, Ordering::SeqCst);
    let d = inner.data.lock().unwrap();
    match member_info(&d, org, user) {
        Some(m) => Json(m).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Deserialize)]
struct ByEmail {
    email: String,
}

async fn member_by_email(
    State(inner): S,
    headers: HeaderMap,
    Path(org): Path<Uuid>,
    Query(q): Query<ByEmail>,
) -> Response {
    if let Err(res) = caller(&inner, &headers) {
        return res;
    }
    inner.lookup_calls.fetch_add(1, Ordering::SeqCst);
    let d = inner.data.lock().unwrap();
    let found = d
        .members
        .iter()
        .filter(|((o, _), _)| *o == org)
        .find(|(_, (u, _, _))| {
            u.email
                .as_deref()
                .is_some_and(|e| e.eq_ignore_ascii_case(q.email.trim()))
        })
        .and_then(|((o, u), _)| member_info(&d, *o, *u));
    match found {
        Some(m) => Json(m).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn team(
    State(inner): S,
    headers: HeaderMap,
    Path((org, team)): Path<(Uuid, Uuid)>,
) -> Response {
    if let Err(res) = caller(&inner, &headers) {
        return res;
    }
    inner.lookup_calls.fetch_add(1, Ordering::SeqCst);
    let d = inner.data.lock().unwrap();
    match d.teams.get(&(org, team)) {
        Some(t) => Json(t.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn team_by_slug(
    State(inner): S,
    headers: HeaderMap,
    Path((org, slug)): Path<(Uuid, String)>,
) -> Response {
    if let Err(res) = caller(&inner, &headers) {
        return res;
    }
    inner.lookup_calls.fetch_add(1, Ordering::SeqCst);
    let d = inner.data.lock().unwrap();
    match d
        .teams
        .iter()
        .find(|((o, _), t)| *o == org && t.slug == slug)
    {
        Some((_, t)) => Json(t.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Parsed `DateTime` re-export so tests need not depend on chrono directly for it.
pub type Timestamp = DateTime<Utc>;
