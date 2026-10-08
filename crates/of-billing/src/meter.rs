//! Charging a tool call against an org's bucket.
//!
//! ## Two halves, two systems
//!
//! The org's *standing* (plan, included operations, this month's count) lives in
//! the platform, which is the system of record for billing. A call's *record*
//! lives here, in the factory's own database, as a row in `usage_outbox`:
//!
//! - **Recording** is local and atomic. [`Meter::charge`] inserts the outbox row
//!   in the tool's own transaction, so a failed call rolls its record back with
//!   everything else (never billed) and a successful one is recorded even if the
//!   platform is down at that instant (never lost). A background shipper
//!   ([`crate::outbox`]) delivers the rows later, exactly once from the
//!   platform's point of view because it dedupes on the row's `event_id`.
//! - **Enforcing** reads the platform's last answer to "how much has this org
//!   used", cached for a minute by `PlatformClient::usage_status`. The cross-
//!   database atomicity the old in-transaction counter gave is gone, so a
//!   hard-stop org can overrun its bucket by whatever it does inside that window
//!   plus whatever is still in the outbox. That bound is the price of the split
//!   and is deliberately accepted for a usage bucket (savvagent/otto-factory#192,
//!   workstream 3).
//!
//! ## Why charging happens *before* the work
//!
//! [`Meter::charge`] is called immediately after the tool opens its
//! transaction, before it does anything. That looks wrong for the rule "a
//! failed call is not billed" — and it is exactly what makes the rule true: the
//! outbox row is in the tool's own transaction, so a tool that fails rolls the
//! meter back with it. Charging afterwards would need a second transaction,
//! which can fail on its own, be retried, or be forgotten. Doing it first also
//! puts the quota check where a refusal costs nothing.
//!
//! ## What enforcement means
//!
//! Enforcement is off by default behind a flag. When it is on, only **billable**
//! tools on a **hard-stop** plan are refused, and only past the bucket. Reads keep
//! working in every case, so an org that hits its limit mid-task can still see
//! the state of its queue and go and upgrade.
//!
//! **If the platform cannot answer the quota question, the call is allowed**
//! (and logged). The alternative is refusing paying customers' work because a
//! lookup failed; the call is still recorded in the outbox, so nothing is
//! under-billed. Authentication already requires the platform, so this only
//! matters in the window where a cached token outlives an outage. The decision
//! is remembered per org for [`FAIL_OPEN_FOR`], so an outage costs one bounded
//! lookup per org per few seconds, not one per call.
//!
//! **The lookup happens before the transaction opens.** [`Meter::warm`] (called
//! by `Factory::tx`) makes the platform call first, so a slow platform never
//! holds a pooled database connection; the check inside the transaction then
//! reads the client's cache.
//!
//! **Unshipped usage counts.** The platform's total lags by whatever is still in
//! the outbox, so the check adds this org's unshipped billable rows to the
//! platform's count before comparing it to the limit.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use otto_resource::{PlatformClient, UsageStatus};
use otto_tenant::ids::{OrgId, UserId};
use otto_tenant::Tx;
use serde::Serialize;

use crate::classify;
use crate::error::{BillingError, Result};
use crate::outbox;

/// How long the quota check waits on the platform before allowing the call.
///
/// The check runs inside the tool's transaction, so a slow platform would hold a
/// pooled database connection for as long as it dawdles; the client's own 10 s
/// timeout is far too long for that. A cache hit (the normal case) costs nothing
/// and a miss gets this long, after which the call goes ahead and is recorded
/// like any other — the same outcome as the platform being down.
const QUOTA_LOOKUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// How long a failed quota lookup is remembered, per org, as "allow".
const FAIL_OPEN_FOR: std::time::Duration = std::time::Duration::from_secs(5);

/// Fraction of the bucket at which a caller starts being warned.
///
/// Eighty percent is early enough that a team has time to do something about it
/// before work starts being refused, and late enough that it is not noise.
pub const WARN_AT: f64 = 0.8;

/// Configuration for the meter.
#[derive(Clone)]
pub struct Meter {
    platform: Arc<PlatformClient>,
    /// Orgs whose last quota lookup failed, and until when to skip looking.
    fail_open: Arc<Mutex<HashMap<OrgId, Instant>>>,
    /// Refuse billable calls past a hard-stop bucket. Off by default.
    pub enforce: bool,
    /// Where a caller who has run out is told to go. Named in the refusal,
    /// because an error that says "upgrade" without saying where is a dead end
    /// for an agent and an annoyance for a human.
    pub upgrade_url: String,
}

impl std::fmt::Debug for Meter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Meter")
            .field("enforce", &self.enforce)
            .field("upgrade_url", &self.upgrade_url)
            .finish_non_exhaustive()
    }
}

impl Meter {
    pub fn new(
        platform: Arc<PlatformClient>,
        enforce: bool,
        upgrade_url: impl Into<String>,
    ) -> Self {
        Self {
            platform,
            fail_open: Arc::default(),
            enforce,
            upgrade_url: upgrade_url.into(),
        }
    }

    /// Look the org's quota standing up now, so the check inside the tool's
    /// transaction finds it cached. Called before the transaction opens; a
    /// no-op when enforcement is off. Never fails: see the module docs.
    pub async fn warm(&self, org: OrgId) {
        if self.enforce {
            let _ = self.status_for(org).await;
        }
    }

    /// Charge one tool call, or refuse it.
    ///
    /// On success the call is recorded in the outbox inside `tx`; on refusal
    /// nothing is written.
    pub async fn charge(&self, tx: &mut Tx<'_>, user: UserId, tool: &str) -> Result<Charge> {
        let class = classify::classify(tool);
        if self.enforce && class.is_billable() {
            if let Some(status) = self.status_for(tx.org()).await {
                let unshipped = unshipped_billable(tx).await?;
                self.refuse_if_spent(status, unshipped, tool)?;
            }
        }
        outbox::enqueue(tx, Some(user), tool, class.is_billable()).await?;
        Ok(Charge {
            billable: class.is_billable(),
        })
    }

    /// Record an idempotent replay of `tool` — always as `Free`, regardless
    /// of `tool`'s own classification, and with no quota check.
    ///
    /// "Both classes are recorded regardless" still has to hold for a replay: it
    /// is a real call that reached the server and was served, just not new work.
    /// Charging it again would double-bill the caller for one logical
    /// job/message; not recording it at all would leave a gap in the history a
    /// future repricing decision needs. Never enforced — a replay creates
    /// nothing, so refusing it on a hard-stop plan would refuse a call that is,
    /// from the bucket's perspective, free to serve.
    pub async fn record_replay(&self, tx: &mut Tx<'_>, user: UserId, tool: &str) -> Result<()> {
        outbox::enqueue(tx, Some(user), tool, false).await?;
        Ok(())
    }

    /// Check whether a call would be refused, without recording any usage.
    ///
    /// Used by `sync_ticket`, whose "work" is an outbound tracker write that has
    /// already happened by the time `charge` runs — without this, an org on a
    /// hard-stop plan already over its bucket gets that write posted anyway, only
    /// to be told afterwards that it wasn't billed. Short-circuits when
    /// enforcement is off or the tool isn't billable, so it costs nothing in the
    /// default configuration.
    ///
    /// Takes the caller's `tx` so it counts unshipped usage exactly as `charge`
    /// does. Judged against the platform's figure alone, an org whose bucket is
    /// only spent once the outbox is added would pass here, have its tracker
    /// write posted, and then be refused by the `charge` that follows — the
    /// very outcome this check exists to prevent.
    pub async fn would_refuse(&self, tx: &mut Tx<'_>, tool: &str) -> Result<()> {
        let class = classify::classify(tool);
        if !self.enforce || !class.is_billable() {
            return Ok(());
        }
        match self.status_for(tx.org()).await {
            Some(status) => {
                let unshipped = unshipped_billable(tx).await?;
                self.refuse_if_spent(status, unshipped, tool)
            }
            None => Ok(()),
        }
    }

    /// The enforcement/hard-stop/bucket comparison shared by [`Self::charge`]
    /// and [`Self::would_refuse`].
    ///
    /// It compares the count *before* this call is added, so the call that
    /// lands exactly on the limit is allowed and the next one is not. An
    /// off-by-one here is the difference between a plan advertised as 500
    /// operations delivering 500 or 499.
    fn refuse_if_spent(&self, mut status: UsageStatus, unshipped: i64, tool: &str) -> Result<()> {
        status.billable_count += unshipped;
        if status.is_blocked() {
            return Err(BillingError::QuotaExceeded {
                tool: tool.to_string(),
                used: status.billable_count,
                included: status.included_ops,
                plan: plan_name(&status.plan),
                upgrade_url: self.upgrade_url.clone(),
            });
        }
        Ok(())
    }

    /// The platform's usage status for `org`, or `None` if it could not be read
    /// in time (the caller then allows the call). Bounded by
    /// [`QUOTA_LOOKUP_TIMEOUT`], and a failure is remembered for
    /// [`FAIL_OPEN_FOR`] so an outage does not add that wait to every call.
    async fn status_for(&self, org: OrgId) -> Option<UsageStatus> {
        if let Some(until) = self
            .fail_open
            .lock()
            .ok()
            .and_then(|m| m.get(&org).copied())
        {
            if until > Instant::now() {
                return None;
            }
        }
        let failure = match tokio::time::timeout(
            QUOTA_LOOKUP_TIMEOUT,
            self.platform.usage_status(org.as_uuid()),
        )
        .await
        {
            Ok(Ok(status)) => {
                if let Ok(mut m) = self.fail_open.lock() {
                    m.remove(&org);
                }
                return Some(status);
            }
            Ok(Err(e)) => e.to_string(),
            Err(_) => "timed out".to_string(),
        };
        tracing::warn!(
            org = %org,
            reason = %failure,
            "could not read usage from the platform in time; allowing billable calls for {FAIL_OPEN_FOR:?} (they are still recorded)"
        );
        if let Ok(mut m) = self.fail_open.lock() {
            m.insert(org, Instant::now() + FAIL_OPEN_FOR);
        }
        None
    }

    /// Report an org's standing without charging for anything.
    ///
    /// Used by the `usage` and `whoami` tools, which are themselves free — a
    /// caller must never have to spend an operation to find out how many it has
    /// left. Unlike the quota check this surfaces a platform failure, since an
    /// answer was asked for and a made-up one would be worse than an error.
    pub async fn report(&self, org: OrgId) -> Result<Status> {
        let usage = self.platform.usage_status(org.as_uuid()).await?;
        Ok(Status::new(usage, self.enforce))
    }
}

/// Billable calls this org has recorded that the platform has not been told
/// about yet.
async fn unshipped_billable(tx: &mut Tx<'_>) -> Result<i64> {
    let org = tx.org();
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM usage_outbox WHERE org_id = $1 AND billable")
            .bind(org)
            .fetch_one(tx.conn())
            .await?,
    )
}

/// `free` -> `Free`.
fn plan_name(plan: &str) -> String {
    let mut chars = plan.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Where an org stands against its bucket.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub plan: String,
    /// Billable operations included this month, after any negotiated override.
    pub included_ops: i64,
    pub billable_used: i64,
    /// Never negative: an org past its bucket has none left, not a debt.
    pub remaining: i64,
    /// Every recorded call this month, billable or not.
    pub total_calls: i64,
    pub period_start: chrono::NaiveDate,
    /// True past [`WARN_AT`] of the bucket. Worth surfacing to a human.
    pub warning: bool,
    /// Whether exceeding the bucket stops billable work rather than metering
    /// overage.
    pub hard_stop: bool,
    /// Whether the server is currently refusing calls over the bucket at all.
    pub enforced: bool,
}

impl Status {
    fn new(usage: UsageStatus, enforced: bool) -> Self {
        Self {
            plan: plan_name(&usage.plan),
            included_ops: usage.included_ops,
            billable_used: usage.billable_count,
            remaining: remaining(usage.billable_count, usage.included_ops),
            total_calls: usage.total_count,
            period_start: usage.period_start,
            warning: warning(usage.billable_count, usage.included_ops),
            hard_stop: usage.hard_stop,
            enforced,
        }
    }
}

/// The outcome of charging one call.
#[derive(Debug, Clone, PartialEq)]
pub struct Charge {
    pub billable: bool,
}

/// Operations left in the bucket, floored at zero.
fn remaining(used: i64, included: i64) -> i64 {
    included.saturating_sub(used).max(0)
}

/// Whether an org has crossed the warning threshold.
///
/// A bucket of zero counts as exhausted rather than as a division by zero — an
/// org with no included operations is over its limit from the first call.
fn warning(used: i64, included: i64) -> bool {
    if included <= 0 {
        return true;
    }
    used as f64 >= included as f64 * WARN_AT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaining_never_goes_negative() {
        assert_eq!(remaining(0, 500), 500);
        assert_eq!(remaining(499, 500), 1);
        assert_eq!(remaining(500, 500), 0);
        assert_eq!(
            remaining(900, 500),
            0,
            "an org past its bucket has none left, not a debt"
        );
    }

    /// The boundary customers actually notice: a plan sold as 500 operations
    /// has to deliver 500, not 499.
    #[test]
    fn the_warning_starts_at_four_fifths() {
        assert!(!warning(399, 500));
        assert!(warning(400, 500));
        assert!(warning(500, 500));
    }

    #[test]
    fn an_empty_bucket_is_always_over() {
        assert!(warning(0, 0));
        assert_eq!(remaining(0, 0), 0);
    }

    #[test]
    fn plans_are_shown_capitalised() {
        assert_eq!(plan_name("free"), "Free");
        assert_eq!(plan_name("enterprise"), "Enterprise");
        assert_eq!(plan_name(""), "");
    }
}
