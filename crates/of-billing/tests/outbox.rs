//! The usage outbox against a real Postgres and a mock platform.
//!
//! The properties that matter: a call's record commits or rolls back with the
//! call; an outage loses nothing; and everything recorded reaches the platform
//! exactly once, however many times or from however many shippers it is tried.

use std::time::Duration;

use of_billing::outbox::{self, ShipperConfig};
use of_billing::{BillingError, Meter};
use of_testkit::MockPlatform;
use otto_tenant::ids::{OrgId, UserId};
use otto_tenant::Db;
use sqlx::PgPool;

fn cfg() -> ShipperConfig {
    ShipperConfig {
        // Short lease and backoff so a test can wait out a retry.
        claim_lease: Duration::from_millis(50),
        base_backoff: Duration::from_millis(20),
        max_backoff: Duration::from_millis(200),
        poll: Duration::from_millis(10),
        ..ShipperConfig::default()
    }
}

async fn record(db: &Db, org: OrgId, user: UserId, meter: &Meter, tool: &str) {
    let mut tx = db.begin(org).await.unwrap();
    meter.charge(&mut tx, user, tool).await.unwrap();
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn a_failed_call_is_not_recorded(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let meter = Meter::new(platform.client(), false, "https://x.test/billing");
    let (org, user) = (OrgId::new(), UserId::new());

    let mut tx = db.begin(org).await.unwrap();
    meter.charge(&mut tx, user, "add_job").await.unwrap();
    // The tool fails after charging: its transaction rolls back, meter and all.
    tx.rollback().await.unwrap();

    assert_eq!(outbox::pending(&db).await.unwrap(), 0);
}

#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn a_recorded_call_ships_once_and_leaves_the_outbox(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let meter = Meter::new(platform.client(), false, "https://x.test/billing");
    let (org, user) = (OrgId::new(), UserId::new());

    record(&db, org, user, &meter, "add_job").await;
    record(&db, org, user, &meter, "list_jobs").await;
    assert_eq!(outbox::pending(&db).await.unwrap(), 2);

    let report = outbox::ship_once(&db, &platform.client(), &cfg())
        .await
        .unwrap();
    assert_eq!((report.claimed, report.accepted), (2, 2));

    assert_eq!(outbox::pending(&db).await.unwrap(), 0);
    let counted = platform.counted_usage();
    assert_eq!(counted.len(), 2);
    let billable: Vec<_> = counted
        .iter()
        .map(|e| (e.tool.as_str(), e.billable))
        .collect();
    assert!(billable.contains(&("add_job", true)));
    assert!(
        billable.contains(&("list_jobs", false)),
        "free calls are recorded too"
    );
    assert!(counted
        .iter()
        .all(|e| e.org_id == org.as_uuid() && e.user_id == Some(user.as_uuid())));

    // Nothing left to ship.
    let report = outbox::ship_once(&db, &platform.client(), &cfg())
        .await
        .unwrap();
    assert_eq!(report.claimed, 0);
    assert_eq!(platform.usage_calls(), 1);
}

#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn an_outage_loses_nothing_and_the_backlog_ships_later_exactly_once(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let client = platform.client();
    let meter = Meter::new(client.clone(), false, "https://x.test/billing");
    let (org, user) = (OrgId::new(), UserId::new());

    platform.set_down(true);
    // The tools keep working and keep recording while the platform is away.
    for _ in 0..5 {
        record(&db, org, user, &meter, "add_job").await;
    }
    for _ in 0..3 {
        assert!(outbox::ship_once(&db, &client, &cfg()).await.is_err());
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    assert_eq!(
        outbox::pending(&db).await.unwrap(),
        5,
        "an outage must not drop rows"
    );
    assert!(platform.counted_usage().is_empty());
    let (attempts, last_error): (i32, Option<String>) =
        sqlx::query_as("SELECT max(attempts), max(last_error) FROM usage_outbox")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert!(attempts >= 1);
    assert!(
        last_error.is_some(),
        "the failure is recorded for an operator"
    );

    // The platform comes back; the backlog drains.
    platform.set_down(false);
    tokio::time::sleep(Duration::from_millis(250)).await;
    let report = outbox::ship_once(&db, &client, &cfg()).await.unwrap();
    assert_eq!((report.claimed, report.accepted), (5, 5));
    assert_eq!(outbox::pending(&db).await.unwrap(), 0);
    assert_eq!(platform.counted_usage().len(), 5);
}

#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn a_partial_failure_retries_and_still_counts_each_call_once(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let client = platform.client();
    let meter = Meter::new(client.clone(), false, "https://x.test/billing");
    let (org, user) = (OrgId::new(), UserId::new());
    for _ in 0..4 {
        record(&db, org, user, &meter, "claim_jobs").await;
    }

    // The first POST fails, the retry succeeds.
    platform.fail_next_usage_posts(1);
    assert!(outbox::ship_once(&db, &client, &cfg()).await.is_err());
    tokio::time::sleep(Duration::from_millis(100)).await;
    outbox::ship_once(&db, &client, &cfg()).await.unwrap();
    assert_eq!(platform.counted_usage().len(), 4);

    // A lost acknowledgement: the platform counted the batch but we never heard,
    // so the same rows go again. The platform calls them duplicates.
    let counted = platform.counted_usage();
    sqlx::query(
        "INSERT INTO usage_outbox (event_id, org_id, user_id, tool, billable) \
         SELECT $1, $2, $3, $4, true",
    )
    .bind(counted[0].event_id)
    .bind(org)
    .bind(user)
    .bind("claim_jobs")
    .execute(db.pool())
    .await
    .unwrap();
    let report = outbox::ship_once(&db, &client, &cfg()).await.unwrap();
    assert_eq!((report.accepted, report.duplicates), (0, 1));
    assert_eq!(
        platform.counted_usage().len(),
        4,
        "a resend was counted twice"
    );
    assert_eq!(outbox::pending(&db).await.unwrap(), 0);
}

#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn concurrent_shippers_never_double_count(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let client = platform.client();
    let meter = Meter::new(client.clone(), false, "https://x.test/billing");
    let (org, user) = (OrgId::new(), UserId::new());
    for _ in 0..40 {
        record(&db, org, user, &meter, "add_job").await;
    }

    let slow = ShipperConfig {
        batch: 10,
        claim_lease: Duration::from_secs(30),
        ..cfg()
    };
    let (a, b) = tokio::join!(
        outbox::ship_once(&db, &client, &slow),
        outbox::ship_once(&db, &client, &slow)
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(
        a.claimed + b.claimed,
        20,
        "two shippers claimed the same rows"
    );
    assert_eq!(a.duplicates + b.duplicates, 0);

    while outbox::pending(&db).await.unwrap() > 0 {
        outbox::ship_once(&db, &client, &slow).await.unwrap();
    }
    assert_eq!(platform.counted_usage().len(), 40);
}

#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn a_rejected_event_is_kept_in_a_dead_letter_table_not_dropped(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let client = platform.client();
    let meter = Meter::new(client.clone(), false, "https://x.test/billing");
    let (gone, user) = (OrgId::new(), UserId::new());
    platform.reject_usage_for(gone.as_uuid());
    record(&db, gone, user, &meter, "add_job").await;

    let report = outbox::ship_once(&db, &client, &cfg()).await.unwrap();
    assert_eq!(report.rejected, 1);
    assert_eq!(
        outbox::pending(&db).await.unwrap(),
        0,
        "not retried forever"
    );

    let (tool, billable, reason): (String, bool, String) =
        sqlx::query_as("SELECT tool, billable, reason FROM usage_outbox_rejected")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!((tool.as_str(), billable), ("add_job", true));
    assert!(
        reason.contains("does not exist"),
        "the platform's reason is kept: {reason}"
    );
}

/// A receipt that does not account for every event cannot be trusted to have
/// covered the batch: keep everything and ask again.
#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn a_receipt_that_does_not_add_up_deletes_nothing(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let client = platform.client();
    let meter = Meter::new(client.clone(), false, "https://x.test/billing");
    let (org, user) = (OrgId::new(), UserId::new());
    for _ in 0..3 {
        record(&db, org, user, &meter, "add_job").await;
    }

    platform.short_receipts(1);
    assert!(outbox::ship_once(&db, &client, &cfg()).await.is_err());
    assert_eq!(
        outbox::pending(&db).await.unwrap(),
        3,
        "rows were deleted on an unreliable receipt"
    );

    tokio::time::sleep(Duration::from_millis(250)).await;
    let report = outbox::ship_once(&db, &client, &cfg()).await.unwrap();
    assert_eq!(
        (report.claimed, report.duplicates),
        (3, 3),
        "the retry is all duplicates"
    );
    assert_eq!(
        platform.counted_usage().len(),
        3,
        "and nothing was counted twice"
    );
    assert_eq!(outbox::pending(&db).await.unwrap(), 0);
}

#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn the_background_task_drains_the_outbox_and_stops_on_shutdown(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let client = platform.client();
    let meter = Meter::new(client.clone(), false, "https://x.test/billing");
    let (org, user) = (OrgId::new(), UserId::new());
    platform.set_down(true);
    for _ in 0..3 {
        record(&db, org, user, &meter, "add_job").await;
    }

    let (tx, rx) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(outbox::run(db.clone(), client, cfg(), rx));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(outbox::pending(&db).await.unwrap(), 3);

    platform.set_down(false);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while outbox::pending(&db).await.unwrap() > 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the shipper never recovered"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(platform.counted_usage().len(), 3);

    tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
}

// ------------------------------------------------------------------- quota

#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn a_spent_hard_stop_bucket_refuses_billable_calls_and_records_nothing(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let meter = Meter::new(platform.client(), true, "https://x.test/billing");
    let (org, user) = (OrgId::new(), UserId::new());
    platform.add_org(org.as_uuid(), "acme", "Acme");
    platform.set_usage(org.as_uuid(), 500, 500, true);

    let mut tx = db.begin(org).await.unwrap();
    let err = meter.charge(&mut tx, user, "add_job").await.unwrap_err();
    assert!(
        matches!(err, BillingError::QuotaExceeded { included: 500, .. }),
        "{err:?}"
    );
    assert!(!err.retriable());
    // Reads still work.
    meter.charge(&mut tx, user, "list_jobs").await.unwrap();
    tx.commit().await.unwrap();

    let tools: Vec<String> = sqlx::query_scalar("SELECT tool FROM usage_outbox")
        .fetch_all(db.pool())
        .await
        .unwrap();
    assert_eq!(tools, vec!["list_jobs".to_string()]);
}

#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn the_call_that_lands_on_the_limit_is_allowed(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let meter = Meter::new(platform.client(), true, "https://x.test/billing");
    let (org, user) = (OrgId::new(), UserId::new());
    platform.add_org(org.as_uuid(), "acme", "Acme");
    platform.set_usage(org.as_uuid(), 499, 500, true);
    record(&db, org, user, &meter, "add_job").await;
}

#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn an_overage_plan_is_never_refused(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let meter = Meter::new(platform.client(), true, "https://x.test/billing");
    let (org, user) = (OrgId::new(), UserId::new());
    platform.add_org(org.as_uuid(), "acme", "Acme");
    platform.set_usage(org.as_uuid(), 9_999, 500, false);
    record(&db, org, user, &meter, "add_job").await;
}

#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn with_enforcement_off_the_platform_is_never_asked(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let meter = Meter::new(platform.client(), false, "https://x.test/billing");
    let (org, user) = (OrgId::new(), UserId::new());
    platform.add_org(org.as_uuid(), "acme", "Acme");
    platform.set_usage(org.as_uuid(), 9_999, 500, true);
    record(&db, org, user, &meter, "add_job").await;
    assert_eq!(platform.lookup_calls(), 0);
}

/// Refusing paying customers' work because a lookup failed is worse than
/// letting it through: the call is still recorded, so nothing is under-billed.
#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn a_platform_outage_does_not_block_work_it_only_defers_the_bill(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let meter = Meter::new(platform.client(), true, "https://x.test/billing");
    let (org, user) = (OrgId::new(), UserId::new());
    platform.set_down(true);
    record(&db, org, user, &meter, "add_job").await;
    assert_eq!(outbox::pending(&db).await.unwrap(), 1);
}

#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn a_replay_is_recorded_as_free_and_never_refused(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let meter = Meter::new(platform.client(), true, "https://x.test/billing");
    let (org, user) = (OrgId::new(), UserId::new());
    platform.add_org(org.as_uuid(), "acme", "Acme");
    platform.set_usage(org.as_uuid(), 500, 500, true);

    let mut tx = db.begin(org).await.unwrap();
    meter.record_replay(&mut tx, user, "add_job").await.unwrap();
    tx.commit().await.unwrap();

    let billable: bool = sqlx::query_scalar("SELECT billable FROM usage_outbox")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert!(!billable);
}

#[tokio::test]
async fn the_report_surfaces_a_platform_failure_instead_of_inventing_numbers() {
    let platform = MockPlatform::start().await;
    let meter = Meter::new(platform.client(), true, "https://x.test/billing");
    let org = OrgId::new();
    platform.add_org(org.as_uuid(), "acme", "Acme");
    platform.set_usage(org.as_uuid(), 450, 500, true);

    let status = meter.report(org).await.unwrap();
    assert_eq!(
        (status.billable_used, status.remaining, status.warning),
        (450, 50, true)
    );
    assert_eq!(status.plan, "Free");
    assert!(status.enforced);

    // A different org the platform has never heard of: an error, not zeros.
    assert!(meter.report(OrgId::new()).await.is_err());
}

/// The platform's count lags by whatever is still in the outbox, so the check
/// adds the org's unshipped billable usage before comparing to the limit.
#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn unshipped_usage_counts_against_the_bucket(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let meter = Meter::new(platform.client(), true, "https://x.test/billing");
    let (org, user) = (OrgId::new(), UserId::new());
    platform.add_org(org.as_uuid(), "acme", "Acme");
    platform.set_usage(org.as_uuid(), 498, 500, true);

    // Two calls fit (498 -> 500), and nothing has been shipped.
    record(&db, org, user, &meter, "add_job").await;
    record(&db, org, user, &meter, "add_job").await;

    // The platform still says 498, but the outbox holds two more.
    let mut tx = db.begin(org).await.unwrap();
    let err = meter.charge(&mut tx, user, "add_job").await.unwrap_err();
    assert!(
        matches!(err, BillingError::QuotaExceeded { used: 500, .. }),
        "{err:?}"
    );
    // Free calls are still fine.
    meter.charge(&mut tx, user, "list_jobs").await.unwrap();
    tx.commit().await.unwrap();
}

/// An outage adds one bounded lookup, not one per call.
#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn an_outage_is_remembered_so_it_does_not_slow_every_call(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let mut c = otto_resource::ClientConfig::new(
        &platform.url,
        of_testkit::RESOURCE_URI,
        of_testkit::SECRET,
    );
    c.usage_status_ttl = Duration::from_millis(1);
    let meter = Meter::new(platform.client_with(c), true, "https://x.test/billing");
    let (org, user) = (OrgId::new(), UserId::new());
    platform.add_org(org.as_uuid(), "acme", "Acme");
    platform.set_down(true);

    for _ in 0..5 {
        record(&db, org, user, &meter, "add_job").await;
    }
    assert_eq!(
        platform.lookup_calls(),
        0,
        "a down platform counts no lookups"
    );
    // (set_down answers 503 before counting; what matters is the next check.)
    platform.set_down(false);
    platform.set_usage(org.as_uuid(), 500, 500, true);
    // Still inside the remembered window: allowed without asking.
    record(&db, org, user, &meter, "add_job").await;
    assert_eq!(
        platform.lookup_calls(),
        0,
        "the failed lookup was not remembered"
    );
}

/// `warm` makes the network call before a transaction exists.
#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn warm_looks_the_org_up_before_any_transaction(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let meter = Meter::new(platform.client(), true, "https://x.test/billing");
    let (org, user) = (OrgId::new(), UserId::new());
    platform.add_org(org.as_uuid(), "acme", "Acme");

    meter.warm(org).await;
    assert_eq!(platform.lookup_calls(), 1);
    record(&db, org, user, &meter, "add_job").await;
    assert_eq!(
        platform.lookup_calls(),
        1,
        "the in-transaction check hit the cache"
    );

    // Off, it never asks.
    let off = Meter::new(platform.client(), false, "https://x.test/billing");
    off.warm(OrgId::new()).await;
    assert_eq!(platform.lookup_calls(), 1);
}
