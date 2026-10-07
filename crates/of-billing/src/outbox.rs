//! The usage outbox: how a metered call reaches the platform.
//!
//! A call is recorded by [`enqueue`], inside the tool's own transaction, as a
//! row in `usage_outbox` with a freshly minted `event_id`. [`run`] is the
//! background task that drains the table: it claims due rows, posts them to the
//! platform with `PlatformClient::ship_usage`, and deletes what the platform has
//! accounted for.
//!
//! ## Why this is exactly-once without being clever
//!
//! The platform dedupes on `(resource server, event_id)`, and `event_id` is fixed
//! when the row is written. So the shipper may retry anything, any number of
//! times, from any number of replicas, and the platform counts each call once.
//! The shipper's only job is to never *lose* a row, which it guarantees by
//! deleting only rows the platform has answered for: every event in a successful
//! response is exactly one of accepted, duplicate, or rejected, and all three are
//! final. A failed or unreadable response deletes nothing.
//!
//! ## Backoff and replicas
//!
//! `next_attempt_at` is both the retry backoff and a claim lease. Claiming a row
//! pushes it into the future (so a second replica skips it, via `FOR UPDATE SKIP
//! LOCKED`); success deletes it; failure re-schedules it with exponential
//! backoff. A replica that dies mid-ship leaves rows that simply become due
//! again when their lease lapses. Running the shipper in every replica is safe.
//!
//! ## Outages
//!
//! While the platform is down rows accumulate and nothing else is affected: the
//! tools keep working and keep recording. When it returns they drain, oldest
//! first.

use std::time::Duration;

use otto_resource::{PlatformClient, UsageEvent};
use otto_tenant::ids::UserId;
use otto_tenant::{Db, Tx};
use sqlx::FromRow;
use tokio::sync::watch;
use uuid::Uuid;

use crate::error::Result;

/// Record one metered call in the caller's transaction.
///
/// Returns the call's `event_id`. The row commits or rolls back with `tx`, which
/// is what makes "a failed call is not billed" true.
pub async fn enqueue(
    tx: &mut Tx<'_>,
    user: Option<UserId>,
    tool: &str,
    billable: bool,
) -> Result<Uuid> {
    let org = tx.org();
    let event_id: Uuid = sqlx::query_scalar(
        "INSERT INTO usage_outbox (org_id, user_id, tool, billable) \
         VALUES ($1, $2, $3, $4) RETURNING event_id",
    )
    .bind(org)
    .bind(user)
    .bind(tool)
    .bind(billable)
    .fetch_one(tx.conn())
    .await?;
    Ok(event_id)
}

/// Shipper tuning. [`Default`] is right for production; tests shorten it.
#[derive(Debug, Clone)]
pub struct ShipperConfig {
    /// Rows per request. Never more than the platform accepts per batch.
    pub batch: usize,
    /// How long to sleep when there is nothing to ship.
    pub poll: Duration,
    /// How long a claim holds a row before another replica may take it.
    pub claim_lease: Duration,
    /// First retry delay for a failed row; doubles per attempt.
    pub base_backoff: Duration,
    /// Ceiling for the per-row retry delay.
    pub max_backoff: Duration,
}

impl Default for ShipperConfig {
    fn default() -> Self {
        Self {
            batch: otto_resource::MAX_USAGE_BATCH,
            poll: Duration::from_secs(5),
            claim_lease: Duration::from_secs(120),
            base_backoff: Duration::from_secs(5),
            max_backoff: Duration::from_secs(15 * 60),
        }
    }
}

/// What one pass did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ShipReport {
    pub claimed: usize,
    pub accepted: u32,
    pub duplicates: u32,
    pub rejected: usize,
}

#[derive(Debug, FromRow)]
struct Row {
    id: i64,
    event_id: Uuid,
    org_id: Uuid,
    user_id: Option<Uuid>,
    tool: String,
    billable: bool,
    occurred_at: chrono::DateTime<chrono::Utc>,
}

/// Claim up to a batch of due rows, ship them, and delete what the platform
/// answered for.
///
/// On any shipping error nothing is deleted: the rows are rescheduled with
/// backoff and the error is returned for the caller's loop to pace itself on.
pub async fn ship_once(
    db: &Db,
    platform: &PlatformClient,
    cfg: &ShipperConfig,
) -> std::result::Result<ShipReport, ShipError> {
    // Claim, on the pool, unpinned: the shipper serves every org. The table's
    // policy lets an unpinned statement see all orgs' rows and no pinned one see
    // another's (see the baseline migration).
    let rows: Vec<Row> = sqlx::query_as(
        "UPDATE usage_outbox SET attempts = attempts + 1, \
                next_attempt_at = now() + make_interval(secs => $2) \
         WHERE id IN ( \
             SELECT id FROM usage_outbox WHERE next_attempt_at <= now() \
             ORDER BY id LIMIT $1 FOR UPDATE SKIP LOCKED) \
         RETURNING id, event_id, org_id, user_id, tool, billable, occurred_at",
    )
    .bind(cfg.batch as i64)
    .bind(cfg.claim_lease.as_secs_f64())
    .fetch_all(db.pool())
    .await?;

    if rows.is_empty() {
        return Ok(ShipReport::default());
    }

    let mut ordered = rows;
    ordered.sort_by_key(|r| r.id);
    let ids: Vec<i64> = ordered.iter().map(|r| r.id).collect();
    let events: Vec<UsageEvent> = ordered
        .iter()
        .map(|r| UsageEvent {
            event_id: r.event_id,
            org_id: r.org_id,
            user_id: r.user_id,
            tool: r.tool.clone(),
            billable: r.billable,
            occurred_at: r.occurred_at,
        })
        .collect();

    match platform.ship_usage(&events).await {
        Ok(receipt) => {
            // Accepted, duplicate, and rejected are all final: delete the lot.
            for r in &receipt.rejected {
                tracing::warn!(
                    event_id = %r.event_id,
                    reason = %r.reason,
                    "the platform refused a usage event; dropping it"
                );
            }
            sqlx::query("DELETE FROM usage_outbox WHERE id = ANY($1)")
                .bind(&ids)
                .execute(db.pool())
                .await?;
            Ok(ShipReport {
                claimed: ids.len(),
                accepted: receipt.accepted,
                duplicates: receipt.duplicates,
                rejected: receipt.rejected.len(),
            })
        }
        Err(e) => {
            // Reschedule exactly these rows. `attempts` was already bumped by
            // the claim, so the delay doubles per consecutive failure.
            let reschedule = sqlx::query(
                "UPDATE usage_outbox SET last_error = $2, \
                        next_attempt_at = now() + make_interval(secs => \
                            LEAST($3::float8 * power(2, LEAST(attempts - 1, 20)), $4::float8)) \
                 WHERE id = ANY($1)",
            )
            .bind(&ids)
            .bind(truncate(&e.to_string(), 500))
            .bind(cfg.base_backoff.as_secs_f64())
            .bind(cfg.max_backoff.as_secs_f64())
            .execute(db.pool())
            .await;
            if let Err(re) = reschedule {
                // The claim lease still holds the rows back, so this only
                // delays the retry; it cannot lose anything.
                tracing::warn!(error = %re, "could not record a usage shipping failure");
            }
            Err(ShipError::Platform(e))
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Why a shipping pass failed.
#[derive(Debug, thiserror::Error)]
pub enum ShipError {
    #[error("the platform did not accept the batch: {0}")]
    Platform(#[from] otto_resource::Error),
    #[error("outbox database error: {0}")]
    Db(#[from] sqlx::Error),
}

/// Rows waiting to be shipped (due or backing off).
pub async fn pending(db: &Db) -> std::result::Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT count(*) FROM usage_outbox")
        .fetch_one(db.pool())
        .await
}

/// Drain the outbox until `shutdown` flips to `true`.
///
/// A final pass runs on the way out so a clean shutdown does not strand rows
/// that were one request from delivered.
pub async fn run(
    db: Db,
    platform: std::sync::Arc<PlatformClient>,
    cfg: ShipperConfig,
    mut shutdown: watch::Receiver<bool>,
) {
    tracing::info!("usage shipper started");
    let mut failures: u32 = 0;
    loop {
        let wait = match ship_once(&db, &platform, &cfg).await {
            Ok(report) => {
                failures = 0;
                if report.claimed > 0 {
                    tracing::debug!(?report, "shipped usage");
                }
                // A full batch means there is probably more waiting.
                if report.claimed >= cfg.batch {
                    Duration::ZERO
                } else {
                    cfg.poll
                }
            }
            Err(e) => {
                failures = failures.saturating_add(1);
                let wait = cfg
                    .poll
                    .saturating_mul(1u32 << failures.min(6))
                    .min(Duration::from_secs(60));
                match &e {
                    ShipError::Platform(otto_resource::Error::Unauthorized) => tracing::error!(
                        "the platform rejected this resource server's credential; usage is \
                         accumulating in the outbox. Check OTTO_INTROSPECTION_SECRET"
                    ),
                    other => {
                        tracing::warn!(error = %other, retry_in = ?wait, "usage shipping failed")
                    }
                }
                wait
            }
        };

        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = shutdown.changed() => {
                if let Err(e) = ship_once(&db, &platform, &cfg).await {
                    tracing::warn!(error = %e, "final usage flush failed; rows stay in the outbox");
                }
                tracing::info!("usage shipper stopped");
                return;
            }
        }
    }
}
