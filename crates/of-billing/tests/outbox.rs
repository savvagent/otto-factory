//! The usage outbox against a real Postgres and a mock platform.
//!
//! The properties that matter: a call's record commits or rolls back with the
//! call; an outage loses nothing; and everything recorded reaches the platform
//! exactly once, however many times or from however many shippers it is tried.

use std::time::Duration;

use of_billing::outbox::{self, ShipperConfig};
use of_billing::{BillingError, Meter, Metered, METER_JOBS_COMPLETED};
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

/// The billable twin of [`record`]: a completed job, the one metered call.
async fn record_metered(db: &Db, org: OrgId, user: UserId, meter: &Meter) {
    let mut tx = db.begin(org).await.unwrap();
    meter
        .charge_metered(&mut tx, user, "complete_job", &Metered::jobs_completed())
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn a_failed_call_is_not_recorded(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let meter = Meter::new(platform.client(), "https://x.test/billing");
    let (org, user) = (OrgId::new(), UserId::new());

    let mut tx = db.begin(org).await.unwrap();
    meter
        .charge_metered(&mut tx, user, "complete_job", &Metered::jobs_completed())
        .await
        .unwrap();
    // The tool fails after charging: its transaction rolls back, meter and all.
    tx.rollback().await.unwrap();

    assert_eq!(outbox::pending(&db).await.unwrap(), 0);
}

#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn a_recorded_call_ships_once_and_leaves_the_outbox(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let meter = Meter::new(platform.client(), "https://x.test/billing");
    let (org, user) = (OrgId::new(), UserId::new());

    record_metered(&db, org, user, &meter).await;
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
    assert!(billable.contains(&("complete_job", true)));
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
    let meter = Meter::new(client.clone(), "https://x.test/billing");
    let (org, user) = (OrgId::new(), UserId::new());

    platform.set_down(true);
    // The tools keep working and keep recording while the platform is away.
    for _ in 0..5 {
        record_metered(&db, org, user, &meter).await;
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
    let meter = Meter::new(client.clone(), "https://x.test/billing");
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
    let meter = Meter::new(client.clone(), "https://x.test/billing");
    let (org, user) = (OrgId::new(), UserId::new());
    for _ in 0..40 {
        record_metered(&db, org, user, &meter).await;
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
    let meter = Meter::new(client.clone(), "https://x.test/billing");
    let (gone, user) = (OrgId::new(), UserId::new());
    platform.reject_usage_for(gone.as_uuid());
    record_metered(&db, gone, user, &meter).await;

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
    assert_eq!((tool.as_str(), billable), ("complete_job", true));
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
    let meter = Meter::new(client.clone(), "https://x.test/billing");
    let (org, user) = (OrgId::new(), UserId::new());
    for _ in 0..3 {
        record_metered(&db, org, user, &meter).await;
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
    let meter = Meter::new(client.clone(), "https://x.test/billing");
    let (org, user) = (OrgId::new(), UserId::new());
    platform.set_down(true);
    for _ in 0..3 {
        record_metered(&db, org, user, &meter).await;
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

// ------------------------------------------------------------- enforcement

const FIX_URL: &str = "https://otto.test";

fn meter(platform: &MockPlatform) -> Meter {
    Meter::new(platform.client(), FIX_URL)
}

/// The tools recorded in the outbox, oldest first.
async fn recorded_tools(db: &Db) -> Vec<String> {
    sqlx::query_scalar("SELECT tool FROM usage_outbox ORDER BY id")
        .fetch_all(db.pool())
        .await
        .unwrap()
}

/// A budget this service's hard stop has tripped, as the platform reports it.
fn tripped_budget() -> otto_resource::BudgetStatus {
    otto_resource::BudgetStatus {
        monthly_budget_cents: 1000,
        spend_cents: Some(1250),
        percent_used: Some(125),
        thresholds_reached: vec![50, 80, 100],
        hard_stop: true,
        blocked: true,
        reason: Some(otto_resource::BUDGET_EXCEEDED.into()),
    }
}

/// Try one completed job, committing whatever it wrote; the error if it was
/// refused.
async fn try_complete(db: &Db, org: OrgId, user: UserId, meter: &Meter) -> Option<BillingError> {
    let mut tx = db.begin(org).await.unwrap();
    let res = meter
        .charge_metered(&mut tx, user, "complete_job", &Metered::jobs_completed())
        .await;
    tx.commit().await.unwrap();
    res.err()
}

/// The platform's blocked reason is the whole rule: a completed job is refused
/// with that reason as its code, nothing is written for it, and a free call in
/// the same transaction still goes through.
#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn a_budget_block_refuses_a_completed_job_and_records_nothing(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let meter = meter(&platform);
    let (org, user) = (OrgId::new(), UserId::new());
    platform.add_org(org.as_uuid(), "acme", "Acme");
    platform.set_blocked_reason(org.as_uuid(), Some(otto_resource::BUDGET_EXCEEDED));

    let mut tx = db.begin(org).await.unwrap();
    let err = meter
        .charge_metered(&mut tx, user, "complete_job", &Metered::jobs_completed())
        .await
        .unwrap_err();
    assert!(matches!(err, BillingError::Blocked { .. }), "{err:?}");
    assert_eq!(err.code(), "budget_exceeded");
    assert!(!err.retriable());
    assert!(err.to_string().contains(FIX_URL), "{err}");
    // Free calls are never refused.
    meter.charge(&mut tx, user, "list_jobs").await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(recorded_tools(&db).await, vec!["list_jobs".to_string()]);
}

#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn a_payment_block_refuses_a_completed_job_as_payment_required(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let meter = meter(&platform);
    let (org, user) = (OrgId::new(), UserId::new());
    platform.add_org(org.as_uuid(), "acme", "Acme");
    platform.set_blocked_reason(org.as_uuid(), Some(otto_resource::PAYMENT_REQUIRED));

    let err = try_complete(&db, org, user, &meter).await.expect("refused");
    assert_eq!(err.code(), "payment_required");
    assert!(!err.retriable());
    assert!(recorded_tools(&db).await.is_empty());
}

/// A platform that reports the budget's hard stop but no `blocked_reason` (one
/// that predates the field) still blocks, and the refusal has to carry a code
/// an agent can branch on: `budget_exceeded`, which is what tripped.
#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn a_tripped_budget_with_no_reason_refuses_as_budget_exceeded(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let meter = meter(&platform);
    let (org, user) = (OrgId::new(), UserId::new());
    platform.add_org(org.as_uuid(), "acme", "Acme");
    platform.set_budget(org.as_uuid(), Some(tripped_budget()));

    let err = try_complete(&db, org, user, &meter).await.expect("refused");
    assert_eq!(err.code(), "budget_exceeded");
    assert!(recorded_tools(&db).await.is_empty());
}

/// The legacy family-wide bucket no longer decides anything: an org past it on
/// a hard-stop plan, with no blocked reason, completes jobs. (The old rule
/// refused here; now the platform says when to refuse.)
#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn a_spent_legacy_bucket_no_longer_refuses(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let meter = meter(&platform);
    let (org, user) = (OrgId::new(), UserId::new());
    platform.add_org(org.as_uuid(), "acme", "Acme");
    platform.set_usage(org.as_uuid(), 9_999, 500, true);
    // A budget under its limit does not block either.
    platform.set_budget(
        org.as_uuid(),
        Some(otto_resource::BudgetStatus {
            spend_cents: Some(10),
            percent_used: Some(1),
            thresholds_reached: vec![],
            blocked: false,
            reason: None,
            ..tripped_budget()
        }),
    );

    assert!(try_complete(&db, org, user, &meter).await.is_none());
    assert_eq!(recorded_tools(&db).await, vec!["complete_job".to_string()]);
}

/// Free calls are never refused, so they never ask: a free call that waited on
/// the platform would make every coordination call as slow as its worst
/// lookup, for an answer it ignores.
#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn free_calls_never_ask_the_platform(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let meter = meter(&platform);
    let (org, user) = (OrgId::new(), UserId::new());
    platform.add_org(org.as_uuid(), "acme", "Acme");
    platform.set_blocked_reason(org.as_uuid(), Some(otto_resource::BUDGET_EXCEEDED));

    record(&db, org, user, &meter, "add_job").await;
    record(&db, org, user, &meter, "claim_jobs").await;
    let mut tx = db.begin(org).await.unwrap();
    meter.record_replay(&mut tx, user, "add_job").await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(platform.status_calls(), 0);
    assert_eq!(outbox::pending(&db).await.unwrap(), 3);
}

/// Refusing paying customers' work because a lookup failed is worse than
/// letting it through: the call is still recorded, so nothing is under-billed.
#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn a_platform_outage_does_not_block_work_it_only_defers_the_bill(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let meter = meter(&platform);
    let (org, user) = (OrgId::new(), UserId::new());
    platform.add_org(org.as_uuid(), "acme", "Acme");
    platform.set_blocked_reason(org.as_uuid(), Some(otto_resource::BUDGET_EXCEEDED));
    platform.set_down(true);

    assert!(try_complete(&db, org, user, &meter).await.is_none());

    let (billable, meter_key): (bool, Option<String>) =
        sqlx::query_as("SELECT billable, meter FROM usage_outbox")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert!(billable);
    assert_eq!(meter_key.as_deref(), Some(METER_JOBS_COMPLETED));
}

#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn a_replay_is_recorded_as_free_and_never_refused(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let meter = meter(&platform);
    let (org, user) = (OrgId::new(), UserId::new());
    platform.add_org(org.as_uuid(), "acme", "Acme");
    platform.set_blocked_reason(org.as_uuid(), Some(otto_resource::BUDGET_EXCEEDED));

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
    let meter = meter(&platform);
    let org = OrgId::new();
    platform.add_org(org.as_uuid(), "acme", "Acme");
    platform.set_usage(org.as_uuid(), 450, 500, true);

    let status = meter.report(org).await.unwrap();
    assert_eq!(
        (status.billable_used, status.remaining, status.warning),
        (450, 50, true)
    );
    assert_eq!(status.plan, "Free");
    assert!(
        status.enforced,
        "the platform's blocked reason is always enforced"
    );

    // A different org the platform has never heard of: an error, not zeros.
    assert!(meter.report(OrgId::new()).await.is_err());
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
    let meter = Meter::new(platform.client_with(c), FIX_URL);
    let (org, user) = (OrgId::new(), UserId::new());
    platform.add_org(org.as_uuid(), "acme", "Acme");
    platform.set_status_down(true);

    for _ in 0..5 {
        assert!(try_complete(&db, org, user, &meter).await.is_none());
    }
    assert_eq!(
        platform.status_calls(),
        1,
        "the first failure is remembered, so the next four never ask"
    );
    platform.set_status_down(false);
    platform.set_blocked_reason(org.as_uuid(), Some(otto_resource::BUDGET_EXCEEDED));
    // Still inside the remembered window: allowed without asking.
    assert!(try_complete(&db, org, user, &meter).await.is_none());
    assert_eq!(
        platform.status_calls(),
        1,
        "the failed lookup was not remembered"
    );
    assert_eq!(outbox::pending(&db).await.unwrap(), 6);
}

/// `warm` makes the network call before a transaction exists, so the check
/// inside the transaction reads the cache.
#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn warm_looks_the_org_up_before_any_transaction(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let meter = meter(&platform);
    let (org, user) = (OrgId::new(), UserId::new());
    platform.add_org(org.as_uuid(), "acme", "Acme");

    meter.warm(org).await;
    assert_eq!(platform.status_calls(), 1);
    record_metered(&db, org, user, &meter).await;
    assert_eq!(
        platform.status_calls(),
        1,
        "the in-transaction check hit the cache"
    );
}

// ------------------------------------------------------------------- meters

async fn enqueue_metered(db: &Db, org: OrgId, user: UserId, metered: Option<&Metered>) {
    let mut tx = db.begin(org).await.unwrap();
    outbox::enqueue(&mut tx, Some(user), "complete_job", true, metered)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn a_metered_row_ships_its_meter_and_quantity(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let (org, user) = (OrgId::new(), UserId::new());
    enqueue_metered(&db, org, user, Some(&Metered::jobs_completed())).await;
    enqueue_metered(&db, org, user, None).await;

    outbox::ship_once(&db, &platform.client(), &cfg())
        .await
        .unwrap();

    let counted = platform.counted_usage();
    assert_eq!(counted.len(), 2);
    let metered: Vec<_> = counted.iter().filter(|e| e.meter.is_some()).collect();
    assert_eq!(metered.len(), 1);
    assert_eq!(
        metered[0].meter.as_deref(),
        Some("otto-factory/jobs_completed")
    );
    assert_eq!(metered[0].quantity, Some(1));
    let plain = counted.iter().find(|e| e.meter.is_none()).unwrap();
    assert_eq!(plain.quantity, None, "an unmetered row carries no quantity");
}

#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn a_rejected_metered_row_keeps_its_meter_and_quantity(pool: PgPool) {
    let platform = MockPlatform::start().await;
    let db = Db::from_pool(pool);
    let (gone, user) = (OrgId::new(), UserId::new());
    platform.reject_usage_for(gone.as_uuid());
    enqueue_metered(&db, gone, user, Some(&Metered::jobs_completed())).await;

    let report = outbox::ship_once(&db, &platform.client(), &cfg())
        .await
        .unwrap();
    assert_eq!(report.rejected, 1);

    let (meter, quantity): (Option<String>, Option<i64>) =
        sqlx::query_as("SELECT meter, quantity FROM usage_outbox_rejected")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(meter.as_deref(), Some(METER_JOBS_COMPLETED));
    assert_eq!(quantity, Some(1));
}

/// A meter without a quantity (or the reverse) would ship a half-formed event;
/// a zero or negative quantity would not be a count. The schema refuses both.
#[sqlx::test(migrator = "of_core::MIGRATOR")]
async fn the_schema_refuses_a_half_metered_or_non_positive_row(pool: PgPool) {
    for (meter, quantity) in [
        (Some("otto-factory/jobs_completed"), None),
        (None, Some(1_i64)),
        (Some("otto-factory/jobs_completed"), Some(0)),
        (Some("otto-factory/jobs_completed"), Some(-1)),
    ] {
        for table in ["usage_outbox", "usage_outbox_rejected"] {
            let sql = if table == "usage_outbox" {
                "INSERT INTO usage_outbox (org_id, tool, billable, meter, quantity) \
                 VALUES ($1, 't', true, $2, $3)"
            } else {
                "INSERT INTO usage_outbox_rejected \
                   (event_id, org_id, tool, billable, occurred_at, reason, meter, quantity) \
                 VALUES (gen_random_uuid(), $1, 't', true, now(), 'r', $2, $3)"
            };
            let res = sqlx::query(sql)
                .bind(OrgId::new().as_uuid())
                .bind(meter)
                .bind(quantity)
                .execute(&pool)
                .await;
            assert!(res.is_err(), "{table} accepted {meter:?}/{quantity:?}");
        }
    }
}
