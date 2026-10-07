//! The platform's lifecycle webhooks, applied.
//!
//! Each test drives `of_core::platform_events::apply` with a verified event, the
//! way `POST /platform/webhooks` does, against a real Postgres.

mod common;

use common::{db, job, tenant, Member};
use of_core::jobs::{JobsExt, Status};
use of_core::leases::LeasesExt;
use of_core::messages::{MessagesExt, NewMessage};
use of_core::platform_events::{apply, Outcome};
use of_core::repos::{NewRepo, ReposExt};
use of_core::teams::VerifiedTeam;
use of_core::trackers::{upsert_binding, upsert_connection, Provider};
use of_testkit::MockPlatform;
use otto_resource::webhook::{self, WebhookEvent};
use otto_tenant::ids::{OrgId, TeamId};
use sqlx::PgPool;
use uuid::Uuid;

fn event(kind: &str, data: serde_json::Value) -> WebhookEvent {
    let (header, body) = MockPlatform::webhook(kind, data);
    webhook::verify(of_testkit::WEBHOOK_SECRET, &header, &body).expect("a verified event")
}

async fn count(db: &otto_tenant::Db, org: OrgId, table: &str) -> i64 {
    let mut tx = db.begin(org).await.unwrap();
    let n = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE org_id = $1"))
        .bind(org)
        .fetch_one(tx.conn())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    n
}

/// Put something in every table an org can own.
async fn populate(db: &otto_tenant::Db, t: &common::Tenant) {
    let mut tx = db.begin(t.org).await.unwrap();
    let j1 = tx.add_job(job(t, "one")).await.unwrap();
    let j2 = tx.add_job(job(t, "two")).await.unwrap();
    tx.set_dependencies(&j2.id, std::slice::from_ref(&j1.id), &[])
        .await
        .unwrap();
    tx.acquire_lease(t.repo, "branch:main", t.user, Some("agent"), None, None)
        .await
        .unwrap();
    tx.send_message(
        t.user,
        NewMessage {
            body: "hello".into(),
            job_id: Some(j1.id.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    tx.ack_messages(t.user, 1).await.unwrap();
    let conn = upsert_connection(
        &mut tx,
        Provider::Github,
        &format!("install-{}", t.org),
        None,
        None,
    )
    .await
    .unwrap();
    upsert_binding(
        &mut tx,
        t.repo,
        Some(conn.id),
        Provider::Github,
        "acme/api",
        "otto-factory",
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO usage_outbox (org_id, user_id, tool, billable) VALUES ($1, $2, 'add_job', true)")
        .bind(t.org)
        .bind(t.user)
        .execute(tx.conn())
        .await
        .unwrap();
    tx.audit(otto_tenant::audit::Entry::new("repo.registered"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

const TABLES: [&str; 13] = [
    "repos",
    "repo_remotes",
    "repo_leases",
    "jobs",
    "job_dependencies",
    "messages",
    "message_cursors",
    "tracker_connections",
    "tracker_bindings",
    "tracker_connection_index",
    "org_counters",
    "usage_outbox",
    "audit_events",
];

#[sqlx::test]
async fn org_deleted_purges_that_org_and_only_that_org(pool: PgPool) {
    let db = db(pool);
    let a = tenant(&db, "acme", "git@github.com:acme/api.git").await;
    let b = tenant(&db, "globex", "git@github.com:globex/api.git").await;
    populate(&db, &a).await;
    populate(&db, &b).await;
    for table in TABLES {
        assert!(
            count(&db, a.org, table).await > 0,
            "{table} was not populated"
        );
    }
    let b_before: Vec<i64> = {
        let mut v = vec![];
        for table in TABLES {
            v.push(count(&db, b.org, table).await);
        }
        v
    };

    let ev = event(
        "org.deleted",
        serde_json::json!({ "org_id": a.org.as_uuid() }),
    );
    let outcome = apply(&db, &ev).await.unwrap();
    assert!(matches!(outcome, Outcome::Applied { .. }), "{outcome:?}");

    for table in TABLES {
        assert_eq!(
            count(&db, a.org, table).await,
            0,
            "{table} survived org.deleted"
        );
    }
    // The other tenant is untouched, row for row.
    for (table, before) in TABLES.iter().zip(b_before) {
        assert_eq!(
            count(&db, b.org, table).await,
            before,
            "{table} of another org changed"
        );
    }
    // The global connection index has no RLS: make sure the purge was scoped.
    let global: i64 = sqlx::query_scalar("SELECT count(*) FROM tracker_connection_index")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(
        global, 1,
        "org.deleted touched another org's connection index"
    );
}

#[sqlx::test]
async fn a_redelivered_event_is_a_no_op(pool: PgPool) {
    let db = db(pool);
    let a = tenant(&db, "acme", "git@github.com:acme/api.git").await;
    let id = Uuid::new_v4();
    let (header, body) = MockPlatform::webhook_with_id(
        id,
        "org.deleted",
        serde_json::json!({ "org_id": a.org.as_uuid() }),
    );
    let ev = webhook::verify(of_testkit::WEBHOOK_SECRET, &header, &body).unwrap();

    assert!(matches!(
        apply(&db, &ev).await.unwrap(),
        Outcome::Applied { .. }
    ));
    assert_eq!(apply(&db, &ev).await.unwrap(), Outcome::Duplicate);

    // New rows after the purge (an org id reused by a test, say) are not wiped
    // by a replay: the marker outlived the purge.
    let mut tx = db.begin(a.org).await.unwrap();
    tx.register_repo(NewRepo {
        slug: "later".into(),
        remotes: vec!["git@github.com:acme/later.git".into()],
        ..Default::default()
    })
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(apply(&db, &ev).await.unwrap(), Outcome::Duplicate);
    assert_eq!(count(&db, a.org, "repos").await, 1);
}

#[sqlx::test]
async fn team_deleted_keeps_the_scope_and_never_widens_it(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = db(pool);
    let a = tenant(&db, "acme", "git@github.com:acme/api.git").await;
    let team = TeamId::new();
    platform.add_org(a.org.as_uuid(), "acme", "Acme");
    platform.add_team(a.org.as_uuid(), team.as_uuid(), "platform", "Platform");

    let verified = VerifiedTeam::verify(&platform.client(), a.org, team)
        .await
        .unwrap();
    let mut tx = db.begin(a.org).await.unwrap();
    let repo = tx
        .register_repo(NewRepo {
            slug: "scoped".into(),
            remotes: vec!["git@github.com:acme/scoped.git".into()],
            team_id: Some(verified),
            ..Default::default()
        })
        .await
        .unwrap();
    let j = tx
        .add_job(of_core::jobs::NewJob {
            repo_id: repo.id,
            title: "scoped work".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(j.team_id, Some(team), "a job inherits its repo's team");
    tx.commit().await.unwrap();

    let ev = event(
        "team.deleted",
        serde_json::json!({ "org_id": a.org.as_uuid(), "team_id": team.as_uuid() }),
    );
    let Outcome::Applied { detail } = apply(&db, &ev).await.unwrap() else {
        panic!("expected the event to apply");
    };
    assert_eq!(detail["repos"], 1);
    assert_eq!(detail["jobs"], 1);

    // Nothing was nulled: a null team_id means org-wide.
    let mut tx = db.begin(a.org).await.unwrap();
    let repo = tx.get_repo(repo.id).await.unwrap().unwrap();
    let job = tx.get_job(&j.id).await.unwrap();
    let audit = tx.audit_trail(Some("platform."), 10).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        repo.team_id,
        Some(team),
        "team.deleted widened a repo to org-wide"
    );
    assert_eq!(
        job.team_id,
        Some(team),
        "team.deleted widened a job to org-wide"
    );
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].action, "platform.team.deleted");

    // And the dead id cannot be attached to anything new: the platform no longer
    // knows it.
    platform.remove_team(a.org.as_uuid(), team.as_uuid());
    let err = VerifiedTeam::verify(&platform.client(), a.org, team).await;
    assert!(
        matches!(err, Err(of_core::Error::TeamNotFound { .. })),
        "{err:?}"
    );

    assert_eq!(apply(&db, &ev).await.unwrap(), Outcome::Duplicate);
}

#[sqlx::test]
async fn member_removed_releases_what_they_held(pool: PgPool) {
    let db = db(pool);
    let a = tenant(&db, "acme", "git@github.com:acme/api.git").await;
    let bob = Member::new().id;

    let mut tx = db.begin(a.org).await.unwrap();
    let held = tx.add_job(job(&a, "bobs")).await.unwrap();
    let mine = tx.add_job(job(&a, "mine")).await.unwrap();
    let done = tx.add_job(job(&a, "done")).await.unwrap();
    tx.claim_jobs(std::slice::from_ref(&held.id), bob, Some("bob-agent"), None)
        .await
        .unwrap();
    tx.claim_jobs(std::slice::from_ref(&mine.id), a.user, None, None)
        .await
        .unwrap();
    tx.claim_jobs(std::slice::from_ref(&done.id), bob, None, None)
        .await
        .unwrap();
    tx.complete_job(&done.id, bob, Some("ok"), None)
        .await
        .unwrap();
    tx.acquire_lease(a.repo, "branch:bob", bob, Some("bob-agent"), None, None)
        .await
        .unwrap();
    tx.acquire_lease(a.repo, "branch:mine", a.user, None, None, None)
        .await
        .unwrap();
    tx.send_message(
        bob,
        NewMessage {
            body: "bob was here".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    tx.ack_messages(bob, 1).await.unwrap();
    tx.commit().await.unwrap();

    let ev = event(
        "member.removed",
        serde_json::json!({ "org_id": a.org.as_uuid(), "user_id": bob.as_uuid() }),
    );
    let Outcome::Applied { detail } = apply(&db, &ev).await.unwrap() else {
        panic!("expected the event to apply");
    };
    assert_eq!(detail["leases_released"], 1);
    assert_eq!(detail["claims_released"], 1);
    assert_eq!(detail["cursors_dropped"], 1);

    let mut tx = db.begin(a.org).await.unwrap();
    let held = tx.get_job(&held.id).await.unwrap();
    let mine = tx.get_job(&mine.id).await.unwrap();
    let done = tx.get_job(&done.id).await.unwrap();
    let leases = tx.list_leases(None).await.unwrap();
    let messages = tx
        .inbox(a.user, &of_core::messages::InboxQuery::default())
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        held.status,
        Status::Pending,
        "bob's in-flight job was not released"
    );
    assert_eq!(held.claimed_by, None);
    assert_eq!(
        mine.status,
        Status::InProgress,
        "someone else's claim was touched"
    );
    assert_eq!(
        done.status,
        Status::Completed,
        "finished history was rewritten"
    );
    assert_eq!(leases.len(), 1);
    assert_eq!(leases[0].resource, "branch:mine");
    assert_eq!(messages.len(), 1, "what bob wrote is history and stays");

    assert_eq!(apply(&db, &ev).await.unwrap(), Outcome::Duplicate);
}

#[sqlx::test]
async fn an_event_for_one_org_cannot_touch_another(pool: PgPool) {
    let db = db(pool);
    let a = tenant(&db, "acme", "git@github.com:acme/api.git").await;
    let b = tenant(&db, "globex", "git@github.com:globex/api.git").await;

    // The same user id in both orgs (a person in two companies).
    let mut tx = db.begin(b.org).await.unwrap();
    tx.acquire_lease(b.repo, "branch:main", a.user, None, None, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let ev = event(
        "member.removed",
        serde_json::json!({ "org_id": a.org.as_uuid(), "user_id": a.user.as_uuid() }),
    );
    apply(&db, &ev).await.unwrap();

    let mut tx = db.begin(b.org).await.unwrap();
    assert_eq!(
        tx.list_leases(None).await.unwrap().len(),
        1,
        "removal in one org released a lease in another"
    );
    tx.commit().await.unwrap();
}

#[sqlx::test]
async fn an_unknown_event_type_is_acknowledged_not_failed(pool: PgPool) {
    let db = db(pool);
    let ev = event("plan.changed", serde_json::json!({ "anything": true }));
    assert_eq!(apply(&db, &ev).await.unwrap(), Outcome::Ignored);
}

// ------------------------------------------------------------- VerifiedTeam

#[tokio::test]
async fn an_unknown_team_fails_closed() {
    let platform = MockPlatform::start().await;
    let org = OrgId::new();
    platform.add_org(org.as_uuid(), "acme", "Acme");

    let res = VerifiedTeam::verify(&platform.client(), org, TeamId::new()).await;
    assert!(
        matches!(res, Err(of_core::Error::TeamNotFound { .. })),
        "{res:?}"
    );
}

#[tokio::test]
async fn a_team_from_another_org_is_not_found() {
    let platform = MockPlatform::start().await;
    let (mine, theirs) = (OrgId::new(), OrgId::new());
    let team = TeamId::new();
    platform.add_org(mine.as_uuid(), "mine", "Mine");
    platform.add_org(theirs.as_uuid(), "theirs", "Theirs");
    platform.add_team(theirs.as_uuid(), team.as_uuid(), "secret", "Secret");

    let res = VerifiedTeam::verify(&platform.client(), mine, team).await;
    assert!(
        matches!(res, Err(of_core::Error::TeamNotFound { .. })),
        "{res:?}"
    );
}

#[tokio::test]
async fn an_unreachable_platform_refuses_instead_of_assuming() {
    let platform = MockPlatform::start().await;
    let org = OrgId::new();
    let team = TeamId::new();
    platform.add_org(org.as_uuid(), "acme", "Acme");
    platform.add_team(org.as_uuid(), team.as_uuid(), "platform", "Platform");
    platform.set_down(true);

    let res = VerifiedTeam::verify(&platform.client(), org, team).await;
    let Err(e) = res else {
        panic!("a team was verified while the platform was down")
    };
    assert!(matches!(e, of_core::Error::Platform(_)), "{e:?}");
    assert!(e.retriable(), "an outage is worth retrying");
}

#[sqlx::test]
async fn a_team_verified_for_another_org_cannot_be_attached_here(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = db(pool);
    let a = tenant(&db, "acme", "git@github.com:acme/api.git").await;
    let b = tenant(&db, "globex", "git@github.com:globex/api.git").await;
    let team = TeamId::new();
    platform.add_org(b.org.as_uuid(), "globex", "Globex");
    platform.add_team(b.org.as_uuid(), team.as_uuid(), "theirs", "Theirs");
    let theirs = VerifiedTeam::verify(&platform.client(), b.org, team)
        .await
        .unwrap();

    let mut tx = db.begin(a.org).await.unwrap();
    let res = tx
        .register_repo(NewRepo {
            slug: "sneaky".into(),
            remotes: vec!["git@github.com:acme/sneaky.git".into()],
            team_id: Some(theirs),
            ..Default::default()
        })
        .await;
    let _ = tx.rollback().await;
    assert!(
        matches!(res, Err(of_core::Error::TeamNotFound { .. })),
        "{res:?}"
    );
}
