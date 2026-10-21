use of_core::jobs::JobsExt;
use of_core::repos::ReposExt;

mod common;

use axum::body::Body;
use common::Harness;
use hmac::{Hmac, Mac};
use http::{Request, StatusCode};
use of_core::jobs::Tracker;
use of_core::repos::NewRepo;
use of_core::trackers::{upsert_binding, upsert_connection, Provider};

use otto_tenant::ids::{OrgId, UserId};
use sha2::Sha256;
use sqlx::PgPool;
use tower::ServiceExt;

type HmacSha256 = Hmac<Sha256>;

const GITHUB_SECRET: &str = "github-secret";
const JIRA_SECRET: &str = "jira-secret";
const GITHUB_ISSUES_FIXTURE: &[u8] =
    include_bytes!("../../of-trackers/tests/fixtures/github-issues.json");
const JIRA_FIXTURE: &[u8] = include_bytes!("../../of-trackers/tests/fixtures/jira-automation.json");
const UNKNOWN_GITHUB_INSTALLATION_FIXTURE: &[u8] = br#"{
  "action":"opened",
  "installation":{"id":999999},
  "repository":{"full_name":"acme/api"},
  "issue":{"id":7001,"number":17,"title":"Implement webhook ingest","body":"Wire webhook verification into the tracker sync flow.","state":"open","labels":[{"name":"bug"},{"name":"trackers"}]}
}"#;

/// An id standing in for a platform record (orgs and users are the platform's).
struct Fx<T> {
    id: T,
}

async fn harness(pool: PgPool) -> Harness {
    common::harness_with(pool, |config| {
        config.github_app_webhook_secret = Some(GITHUB_SECRET.into());
    })
    .await
}

#[sqlx::test(migrations = "../of-core/migrations")]
async fn github_webhooks_are_public_and_acknowledged_when_verified(pool: PgPool) {
    let h = harness(pool).await;
    let org = Fx {
        id: otto_tenant::ids::OrgId::new(),
    };

    let mut tx = h.db.begin(org.id).await.unwrap();
    let repo = tx
        .register_repo(NewRepo {
            slug: "api".into(),
            name: Some("Acme API".into()),
            remotes: vec!["git@github.com:acme/api.git".into()],
            created_by: Some(otto_tenant::ids::UserId::new()),
            ..Default::default()
        })
        .await
        .unwrap();
    let connection = upsert_connection(&mut tx, Provider::Github, "123456", None, None)
        .await
        .unwrap();
    upsert_binding(
        &mut tx,
        repo.id,
        Some(connection.id),
        Provider::Github,
        "acme/api",
        "trackers",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let reply = post_webhook(
        &h,
        "/webhooks/github",
        GITHUB_ISSUES_FIXTURE,
        &[
            ("x-github-event", "issues"),
            (
                "x-hub-signature-256",
                &format!("sha256={}", github_signature(GITHUB_ISSUES_FIXTURE)),
            ),
        ],
    )
    .await;

    assert_eq!(reply.status(), StatusCode::OK);
    assert_eq!(body_json(reply).await["ok"], serde_json::Value::Bool(true));
}

#[sqlx::test(migrations = "../of-core/migrations")]
async fn github_rejection_and_unknown_connection_share_the_same_public_404(pool: PgPool) {
    let h = harness(pool).await;
    let org = Fx {
        id: otto_tenant::ids::OrgId::new(),
    };

    let mut tx = h.db.begin(org.id).await.unwrap();
    upsert_connection(&mut tx, Provider::Github, "123456", None, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let invalid_signature = post_webhook(
        &h,
        "/webhooks/github",
        GITHUB_ISSUES_FIXTURE,
        &[
            ("x-github-event", "issues"),
            ("x-hub-signature-256", "sha256=deadbeef"),
        ],
    )
    .await;
    assert_eq!(invalid_signature.status(), StatusCode::NOT_FOUND);

    let unknown_installation = post_webhook(
        &h,
        "/webhooks/github",
        UNKNOWN_GITHUB_INSTALLATION_FIXTURE,
        &[
            ("x-github-event", "issues"),
            (
                "x-hub-signature-256",
                &format!(
                    "sha256={}",
                    github_signature(UNKNOWN_GITHUB_INSTALLATION_FIXTURE)
                ),
            ),
        ],
    )
    .await;

    assert_eq!(unknown_installation.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        body_text(invalid_signature).await,
        body_text(unknown_installation).await
    );
}

#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_labelled_issue_webhook_creates_one_job_and_a_stale_redelivery_does_not_duplicate_it(
    pool: PgPool,
) {
    let h = harness(pool).await;
    let org = Fx {
        id: otto_tenant::ids::OrgId::new(),
    };

    let mut tx = h.db.begin(org.id).await.unwrap();
    let repo = tx
        .register_repo(NewRepo {
            slug: "api".into(),
            name: Some("Acme API".into()),
            remotes: vec!["git@github.com:acme/api.git".into()],
            created_by: Some(otto_tenant::ids::UserId::new()),
            ..Default::default()
        })
        .await
        .unwrap();
    let connection = upsert_connection(&mut tx, Provider::Github, "123456", None, None)
        .await
        .unwrap();
    upsert_binding(
        &mut tx,
        repo.id,
        Some(connection.id),
        Provider::Github,
        "acme/api",
        "trackers",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    for _ in 0..2 {
        let reply = post_webhook(
            &h,
            "/webhooks/github",
            GITHUB_ISSUES_FIXTURE,
            &[
                ("x-github-event", "issues"),
                (
                    "x-hub-signature-256",
                    &format!("sha256={}", github_signature(GITHUB_ISSUES_FIXTURE)),
                ),
            ],
        )
        .await;
        assert_eq!(reply.status(), StatusCode::OK);
    }

    let mut tx = h.db.begin(org.id).await.unwrap();
    let job = tx
        .get_job_by_ticket_for_repo(repo.id, Tracker::Github, "acme/api#17")
        .await
        .unwrap()
        .expect("synced job");
    let jobs = tx
        .list_jobs(&of_core::jobs::JobFilter {
            repo_id: Some(repo.id),
            ..Default::default()
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(job.title, "Implement webhook ingest");
    assert_eq!(job.remote_revision.as_deref(), Some("2026-09-03T18:00:00Z"));
    assert_eq!(jobs.len(), 1);
}

/// The fixture GitHub installation, and a second one for a second org.
const INSTALLATION: u64 = 123456;
const OTHER_INSTALLATION: u64 = 654321;

/// Register a repo in `org` bound to `installation`'s `full_name` with the
/// `trackers` label, so an opened issue there syncs in as a pending job.
async fn bind_github_repo(h: &Harness, org: OrgId, installation: u64, full_name: &str) {
    let mut tx = h.db.begin(org).await.unwrap();
    let repo = tx
        .register_repo(NewRepo {
            slug: "api".into(),
            name: Some("Acme API".into()),
            remotes: vec![format!("git@github.com:{full_name}.git")],
            created_by: Some(UserId::new()),
            ..Default::default()
        })
        .await
        .unwrap();
    let connection = upsert_connection(
        &mut tx,
        Provider::Github,
        &installation.to_string(),
        None,
        None,
    )
    .await
    .unwrap();
    upsert_binding(
        &mut tx,
        repo.id,
        Some(connection.id),
        Provider::Github,
        full_name,
        "trackers",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

/// A GitHub `issues` event for issue `number` in `full_name`.
fn github_issue(
    installation: u64,
    full_name: &str,
    number: u64,
    action: &str,
    state_reason: Option<&str>,
    updated_at: &str,
) -> Vec<u8> {
    let state = if action == "closed" { "closed" } else { "open" };
    let state_reason = state_reason.map_or("null".to_string(), |r| format!("\"{r}\""));
    format!(
        r#"{{
  "action":"{action}",
  "installation":{{"id":{installation}}},
  "repository":{{"full_name":"{full_name}"}},
  "issue":{{"id":{id},"number":{number},"title":"Issue {number}","body":"x","state":"{state}","updated_at":"{updated_at}","state_reason":{state_reason},"labels":[{{"name":"trackers"}}]}}
}}"#,
        id = 7000 + number,
    )
    .into_bytes()
}

async fn post_github_issue(h: &Harness, body: &[u8]) -> http::Response<Body> {
    post_webhook(
        h,
        "/webhooks/github",
        body,
        &[
            ("x-github-event", "issues"),
            (
                "x-hub-signature-256",
                &format!("sha256={}", github_signature(body)),
            ),
        ],
    )
    .await
}

/// Open issue `number` in `full_name`, which syncs it in as a pending job, and
/// return that job.
async fn open_issue(
    h: &Harness,
    org: OrgId,
    installation: u64,
    full_name: &str,
    number: u64,
) -> of_core::jobs::Job {
    let opened = github_issue(
        installation,
        full_name,
        number,
        "opened",
        None,
        "2026-09-03T18:00:00Z",
    );
    assert_eq!(post_github_issue(h, &opened).await.status(), StatusCode::OK);
    job_for(h, org, full_name, number).await
}

/// Close issue `number` as `state_reason`, asserting the delivery succeeded.
async fn close_issue(h: &Harness, installation: u64, full_name: &str, number: u64, reason: &str) {
    let closed = github_issue(
        installation,
        full_name,
        number,
        "closed",
        Some(reason),
        "2026-09-04T09:00:00Z",
    );
    assert_eq!(post_github_issue(h, &closed).await.status(), StatusCode::OK);
}

async fn job_for(h: &Harness, org: OrgId, full_name: &str, number: u64) -> of_core::jobs::Job {
    let mut tx = h.db.begin(org).await.unwrap();
    let repo = tx.get_repo_by_slug("api").await.unwrap().expect("repo");
    let job = tx
        .get_job_by_ticket_for_repo(repo.id, Tracker::Github, &format!("{full_name}#{number}"))
        .await
        .unwrap()
        .expect("job");
    tx.commit().await.unwrap();
    job
}

/// Claim `job` for `agent`, as `claim_jobs` would.
async fn claim(h: &Harness, org: OrgId, job: &of_core::jobs::Job, agent: UserId) {
    let mut tx = h.db.begin(org).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&job.id), agent, None, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

/// One usage row recording a completed job.
#[derive(Debug, PartialEq, sqlx::FromRow)]
struct CompletionRow {
    tool: String,
    billable: bool,
    meter: Option<String>,
    quantity: Option<i64>,
    user_id: Option<uuid::Uuid>,
}

/// Every usage row recording a completed job, in the order written.
async fn completion_rows(pool: &PgPool) -> Vec<CompletionRow> {
    sqlx::query_as(
        "SELECT tool, billable, meter, quantity, user_id FROM usage_outbox \
         WHERE tool IN ('complete_job', 'inbound_ticket_close') ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

fn billed_close() -> CompletionRow {
    CompletionRow {
        tool: "inbound_ticket_close".into(),
        billable: true,
        meter: Some("otto-factory/jobs_completed".into()),
        quantity: Some(1),
        user_id: None,
    }
}

fn unbilled_close() -> CompletionRow {
    CompletionRow {
        tool: "inbound_ticket_close".into(),
        billable: false,
        meter: None,
        quantity: None,
        user_id: None,
    }
}

/// An agent claimed the job, and the ticket closed as done: that is a
/// completed job, billed once however often the close is delivered.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn an_inbound_close_of_a_claimed_job_bills_it_exactly_once(pool: PgPool) {
    let h = harness(pool.clone()).await;
    let org = OrgId::new();
    bind_github_repo(&h, org, INSTALLATION, "acme/api").await;
    let job = open_issue(&h, org, INSTALLATION, "acme/api", 17).await;
    claim(&h, org, &job, UserId::new()).await;

    // The redelivery is dropped as stale, so the close records once.
    for _ in 0..2 {
        close_issue(&h, INSTALLATION, "acme/api", 17, "completed").await;
    }

    assert_eq!(
        job_for(&h, org, "acme/api", 17).await.status,
        of_core::jobs::Status::Completed
    );
    assert_eq!(completion_rows(&pool).await, vec![billed_close()]);
}

/// Billing follows claim history, not the status at the close: a job an
/// agent claimed and then handed back (`repend_job`, or a claim left to
/// expire) was worked, so closing its ticket from `pending` is still billed.
/// Otherwise an agent could repend and close to complete work unbilled.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn an_inbound_close_of_a_job_claimed_then_repended_is_billed(pool: PgPool) {
    let h = harness(pool.clone()).await;
    let org = OrgId::new();
    bind_github_repo(&h, org, INSTALLATION, "acme/api").await;
    let job = open_issue(&h, org, INSTALLATION, "acme/api", 17).await;
    claim(&h, org, &job, UserId::new()).await;
    let mut tx = h.db.begin(org).await.unwrap();
    tx.repend_job(&job.id).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        job_for(&h, org, "acme/api", 17).await.status,
        of_core::jobs::Status::Pending
    );

    close_issue(&h, INSTALLATION, "acme/api", 17, "completed").await;

    assert_eq!(
        job_for(&h, org, "acme/api", 17).await.status,
        of_core::jobs::Status::Completed
    );
    assert_eq!(completion_rows(&pool).await, vec![billed_close()]);
}

/// Nobody ever claimed a pending job (`attempts` is 0), so closing its
/// ticket (by anyone, as anything but `not_planned`) completes it without
/// billing the org.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn an_inbound_close_of_a_never_claimed_job_completes_it_unbilled(pool: PgPool) {
    let h = harness(pool.clone()).await;
    let org = OrgId::new();
    bind_github_repo(&h, org, INSTALLATION, "acme/api").await;
    open_issue(&h, org, INSTALLATION, "acme/api", 17).await;

    for _ in 0..2 {
        close_issue(&h, INSTALLATION, "acme/api", 17, "completed").await;
    }

    assert_eq!(
        job_for(&h, org, "acme/api", 17).await.status,
        of_core::jobs::Status::Completed
    );
    assert_eq!(completion_rows(&pool).await, vec![unbilled_close()]);
}

/// The budget's hard stop holds on the tracker path too: an agent refused at
/// `complete_job` cannot get the job billed by closing its ticket instead.
/// The close still applies (the ticket is closed); it is recorded unbilled.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_blocked_orgs_inbound_close_completes_the_job_unbilled(pool: PgPool) {
    let h = harness(pool.clone()).await;
    let org = OrgId::new();
    h.platform
        .set_budget(org.as_uuid(), Some(of_testkit::tripped_budget()));
    bind_github_repo(&h, org, INSTALLATION, "acme/api").await;
    let job = open_issue(&h, org, INSTALLATION, "acme/api", 17).await;
    claim(&h, org, &job, UserId::new()).await;

    close_issue(&h, INSTALLATION, "acme/api", 17, "completed").await;

    assert_eq!(
        job_for(&h, org, "acme/api", 17).await.status,
        of_core::jobs::Status::Completed
    );
    assert_eq!(completion_rows(&pool).await, vec![unbilled_close()]);
}

#[sqlx::test(migrations = "../of-core/migrations")]
async fn an_inbound_close_as_not_planned_fails_the_job_and_records_no_completion(pool: PgPool) {
    let h = harness(pool.clone()).await;
    let org = OrgId::new();
    bind_github_repo(&h, org, INSTALLATION, "acme/api").await;
    let job = open_issue(&h, org, INSTALLATION, "acme/api", 17).await;
    claim(&h, org, &job, UserId::new()).await;

    close_issue(&h, INSTALLATION, "acme/api", 17, "not_planned").await;

    assert_eq!(
        job_for(&h, org, "acme/api", 17).await.status,
        of_core::jobs::Status::Failed
    );
    assert!(completion_rows(&pool).await.is_empty());
}

/// Complete `job` the way `complete_job` does: warm the org's standing, then
/// charge and complete in one transaction. `false` if it was refused (and so
/// wrote nothing).
async fn complete_as_agent(
    h: &Harness,
    meter: &of_billing::Meter,
    org: OrgId,
    job: &of_core::jobs::Job,
    agent: UserId,
) -> bool {
    let warmed = meter.warm(org).await;
    let mut tx = h.db.begin(org).await.unwrap();
    if meter
        .charge_completion(&mut tx, agent, &warmed)
        .await
        .is_err()
    {
        return false;
    }
    tx.complete_job(&job.id, agent, None, None).await.unwrap();
    tx.commit().await.unwrap();
    true
}

/// A job already completed through `complete_job` was recorded by that call.
/// A later close event (newer `updated_at`, so not stale) finds a terminal
/// job, is a no-op, and must not write a second row.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn a_later_close_of_a_job_completed_by_complete_job_records_nothing_more(pool: PgPool) {
    let h = harness(pool.clone()).await;
    let org = OrgId::new();
    let meter = of_billing::Meter::new(h.platform.client(), "https://x.test");
    bind_github_repo(&h, org, INSTALLATION, "acme/api").await;
    let job = open_issue(&h, org, INSTALLATION, "acme/api", 17).await;
    let agent = UserId::new();
    claim(&h, org, &job, agent).await;
    assert!(complete_as_agent(&h, &meter, org, &job, agent).await);

    close_issue(&h, INSTALLATION, "acme/api", 17, "completed").await;

    let rows = completion_rows(&pool).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].tool, "complete_job");
    assert!(rows[0].billable);
}

/// The parity guard: however a job reaches `completed`, it writes exactly one
/// completion row, so `org_counters.jobs_completed_total` always equals the
/// number of completion rows. A path that completes a job without recording
/// it (or records one twice) breaks this.
#[sqlx::test(migrations = "../of-core/migrations")]
async fn every_completion_writes_exactly_one_completion_row(pool: PgPool) {
    let h = harness(pool.clone()).await;
    let meter = of_billing::Meter::new(h.platform.client(), "https://x.test");
    let agent = UserId::new();

    let open = OrgId::new();
    bind_github_repo(&h, open, INSTALLATION, "acme/api").await;
    // complete_job, then a later close of the same ticket: one billed row.
    let done = open_issue(&h, open, INSTALLATION, "acme/api", 1).await;
    claim(&h, open, &done, agent).await;
    assert!(complete_as_agent(&h, &meter, open, &done, agent).await);
    close_issue(&h, INSTALLATION, "acme/api", 1, "completed").await;
    // A claimed job closed as done: billed.
    let claimed = open_issue(&h, open, INSTALLATION, "acme/api", 2).await;
    claim(&h, open, &claimed, agent).await;
    close_issue(&h, INSTALLATION, "acme/api", 2, "completed").await;
    // A never-claimed job closed: completed, unbilled.
    open_issue(&h, open, INSTALLATION, "acme/api", 3).await;
    close_issue(&h, INSTALLATION, "acme/api", 3, "completed").await;
    // Closed as not planned: failed, no completion.
    let dropped = open_issue(&h, open, INSTALLATION, "acme/api", 4).await;
    claim(&h, open, &dropped, agent).await;
    close_issue(&h, INSTALLATION, "acme/api", 4, "not_planned").await;

    let blocked = OrgId::new();
    h.platform
        .set_budget(blocked.as_uuid(), Some(of_testkit::tripped_budget()));
    bind_github_repo(&h, blocked, OTHER_INSTALLATION, "acme/web").await;
    // complete_job is refused outright: nothing completes, nothing written.
    let refused = open_issue(&h, blocked, OTHER_INSTALLATION, "acme/web", 5).await;
    claim(&h, blocked, &refused, agent).await;
    assert!(!complete_as_agent(&h, &meter, blocked, &refused, agent).await);
    // The same job closed in the tracker instead: completed, unbilled.
    close_issue(&h, OTHER_INSTALLATION, "acme/web", 5, "completed").await;

    let mut completed = 0;
    for org in [open, blocked] {
        let mut tx = h.db.begin(org).await.unwrap();
        completed += tx.stats(None).await.unwrap().completed;
        tx.commit().await.unwrap();
    }
    let rows = completion_rows(&pool).await;
    assert_eq!(completed, 4);
    assert_eq!(rows.len() as i64, completed, "{rows:?}");
    assert_eq!(
        rows.iter().filter(|r| r.billable).count(),
        2,
        "only complete_job and the claimed close are billed: {rows:?}"
    );
    assert!(
        rows.iter()
            .all(|r| r.billable == (r.meter.as_deref() == Some("otto-factory/jobs_completed"))),
        "a row is billed exactly when it carries the meter: {rows:?}"
    );
}

#[sqlx::test(migrations = "../of-core/migrations")]
async fn jira_webhooks_require_the_site_and_secret_but_acknowledge_a_valid_request(pool: PgPool) {
    let h = harness(pool).await;
    let org = Fx {
        id: otto_tenant::ids::OrgId::new(),
    };
    let sealed_secret = h.cipher.seal(JIRA_SECRET.as_bytes()).unwrap();

    let mut tx = h.db.begin(org.id).await.unwrap();
    let repo = tx
        .register_repo(NewRepo {
            slug: "api".into(),
            name: Some("Globex API".into()),
            remotes: vec!["git@github.com:globex/api.git".into()],
            created_by: Some(otto_tenant::ids::UserId::new()),
            ..Default::default()
        })
        .await
        .unwrap();
    let connection = upsert_connection(
        &mut tx,
        Provider::Jira,
        "cloud-123",
        None,
        Some(&sealed_secret),
    )
    .await
    .unwrap();
    upsert_binding(
        &mut tx,
        repo.id,
        Some(connection.id),
        Provider::Jira,
        "DF",
        "trackers",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let ok = post_webhook(
        &h,
        "/webhooks/jira?site=cloud-123",
        JIRA_FIXTURE,
        &[("x-of-webhook-secret", JIRA_SECRET)],
    )
    .await;
    assert_eq!(ok.status(), StatusCode::OK);

    let wrong_secret = post_webhook(
        &h,
        "/webhooks/jira?site=cloud-123",
        JIRA_FIXTURE,
        &[("x-of-webhook-secret", "wrong-secret")],
    )
    .await;
    let unknown_site = post_webhook(
        &h,
        "/webhooks/jira?site=cloud-999",
        JIRA_FIXTURE,
        &[("x-of-webhook-secret", JIRA_SECRET)],
    )
    .await;
    let missing_site = post_webhook(
        &h,
        "/webhooks/jira",
        JIRA_FIXTURE,
        &[("x-of-webhook-secret", JIRA_SECRET)],
    )
    .await;

    assert_eq!(wrong_secret.status(), StatusCode::NOT_FOUND);
    assert_eq!(unknown_site.status(), StatusCode::NOT_FOUND);
    assert_eq!(missing_site.status(), StatusCode::NOT_FOUND);

    let wrong_body = body_text(wrong_secret).await;
    assert_eq!(wrong_body, body_text(unknown_site).await);
    assert_eq!(wrong_body, body_text(missing_site).await);
}

#[sqlx::test(migrations = "../of-core/migrations")]
async fn the_openapi_document_mentions_the_webhook_route(pool: PgPool) {
    let h = harness(pool).await;
    let doc = common::Call::get("/api/openapi.json").send(&h.router).await;
    doc.expect(StatusCode::OK);
    assert!(doc.body["paths"]["/webhooks/{provider}"]["post"].is_object());
}

async fn post_webhook(
    h: &Harness,
    uri: &str,
    body: &[u8],
    headers: &[(&str, &str)],
) -> http::Response<Body> {
    let mut builder = Request::builder().method("POST").uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    h.router
        .clone()
        .oneshot(builder.body(Body::from(body.to_vec())).unwrap())
        .await
        .unwrap()
}

async fn body_json(response: http::Response<Body>) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), 256 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn body_text(response: http::Response<Body>) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), 256 * 1024)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn github_signature(body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(GITHUB_SECRET.as_bytes()).unwrap();
    mac.update(body);
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
